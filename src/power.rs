//! `veridict power`: pre-experiment sample-size estimation for `compare --metric winrate/
//! sign-test/elo`. The counterpart to `verdict::estimate_additional_trials` (which asks "how many
//! *more* trials would likely resolve an *already-run* inconclusive result") but *before* any
//! trial has run: "how many trials would I need for a target probability of reaching a passing
//! verdict at all."
//!
//! **Why this needs two effect values, not one.** `compare`'s real decision rule
//! (`verdict::decide`) is: pass iff a confidence interval's lower bound clears `pass_above`. If
//! power were evaluated with the *true* effect set equal to that same pass bar, the answer would
//! be the interval's own miscoverage at the boundary it was built against (`≈ 1 - confidence`,
//! flat, never climbing toward a target power no matter how large `n` gets) - not a useful power
//! number. A real power calculation needs the pass bar (`min_effect`, exactly what `--min-effect`/
//! `--pass-above` mean elsewhere in this project) and a *separate*, strictly larger, assumed true
//! effect (`assume_effect`) to power for - the standard distinction between "the smallest effect
//! worth caring about" and "the effect you actually expect/hope for," present in every real power
//! analysis. `estimate_trials` rejects `assume_effect <= min_effect` as a hard error rather than
//! silently returning a number that means something other than what it looks like.
//!
//! **Why exact binomial summation, not a textbook power formula.** Same rationale
//! `estimate_additional_trials` already gives for its own exact branch: `winrate`/`sign-test`/
//! `elo` already have a real, already-tested CI function (`wilson`/`exact`/`jeffreys`) - searching
//! against that function directly, rather than a generic normal-approximation power formula,
//! means the answer is exact for the *actual* decision rule `compare` uses, not an approximation
//! of it. The normal-approximation formula is used only to seed the search (a fast starting
//! bracket), never as the reported answer.
//!
//! **The "sawtooth" caveat.** Exact power for a discrete CI method (Wilson/Clopper-Pearson/
//! Jeffreys) is not perfectly monotonic in `n` - a documented property of exact discrete methods,
//! not a bug here (Chernick, M.R. & Liu, C.Y. (2002), "The Saw-Toothed Behavior of Power Versus
//! Sample Size and Software Solutions: Single Binomial Proportion Using Exact Methods," *The
//! American Statistician* 56(2):149-155). A naive "first `n` a binary search lands on" could
//! report a lucky local spike that dips back below target at `n+1`. The search below confirms a
//! candidate `n` holds for a stability window of subsequent `n` before accepting it - see
//! `tests/calibration/power_calibration.rs` for the Monte Carlo check that the final answer's
//! *empirical* pass rate actually matches `target_power`, the real proof this matters, not just
//! that the algorithm compiles.
//!
//! **Why the output is a design estimate, not a guarantee.** `estimated_trials` assumes the true
//! effect is *exactly* `assume_effect`. A smaller real effect needs more trials than this number
//! says - this is not a corner case to special-case away, it is the entire reason
//! `--assume-effect` must be a real, separate, user-supplied assumption rather than something this
//! tool infers. Consistent with this project's "a false pass is worse than an inconclusive result"
//! bias: `power` is a design aid for choosing how much data to collect, never a substitute for the
//! real confidence interval `compare` computes from the data that's actually observed.
//!
//! **Why `mean-diff` isn't supported here.** No closed-form CI-width-at-n function exists for a
//! bootstrap CI without real resampled data (same reason `estimate_additional_trials` falls back
//! to an `O(1/sqrt(n))` approximation for it) - but *unlike* that post-hoc case, there is no
//! fallback available pre-experiment either, since there's no existing sample to approximate a
//! variance from. `PowerMetric::new` rejects `MetricKind::MeanDiff` with a real error (not
//! `unreachable!()`) since it's a `pub` constructor a library caller could reach directly, even
//! though the CLI's own `PowerMetricArg` already excludes it at the flag-parsing level.
//!
//! **`--sprt` mode is a structurally different question, not a variant of the search above.**
//! Wald's SPRT guarantees its `alpha`/`beta` error rates by construction, regardless of `n` - there
//! is no "target power" to search a sample size for the way the CI-crossing mode above does. What's
//! useful instead is the *expected* number of trials to a decision (Wald's own "Average Sample
//! Number") under each hypothesis, via the classical ASN approximation:
//! `E[N|H] ≈ [alpha'(H)*ln(A) + (1-alpha'(H))*ln(B)] / E[Z|H]`, where `alpha'(H)` is the
//! probability of stopping at the *upper* boundary `ln(A)` under hypothesis `H` (source: Wald
//! (1947), *Sequential Analysis*) - reusing `stats::sprt::{bounds, score_from_elo, llr_delta}`
//! directly, the exact same functions `sprt::run`'s own Wald loop uses for its real stopping
//! boundaries, not re-derived math. This is a known *approximation*: it ignores "overshoot" (the
//! LLR's excess past a boundary at the moment of stopping), so the true expected sample size runs
//! somewhat higher in practice - `tests/calibration/sprt_asn_calibration.rs` measures the real bias
//! empirically rather than leaving it as an unquantified caveat.

use serde::Serialize;

use crate::error::VeridictError;
use crate::sprt::SprtConfig;
use crate::stats::sprt::{bounds, llr_delta, score_from_elo};
use crate::stats::{exact, jeffreys, wilson};
use crate::{CiMethod, MetricKind};

/// Upper bound on trials the search will ever evaluate. ponytail: this is a safety net against
/// forever-widening searches when the requested effect gap is tiny relative to
/// confidence/target_power, not a claim that 5,000,000 trials is ever a practical experiment size
/// - raise it only if a genuine use case needs to *express* (not achieve) a larger number.
const MAX_TRIALS: u64 = 5_000_000;

/// How many consecutive `n` must all clear `target_power` before the search accepts the first of
/// them as the answer - see the module doc's "sawtooth" section.
const STABILITY_WINDOW: u64 = 20;

/// How far the local stability scan looks around the coarse bracket search's crossing point.
/// Generous relative to the sawtooth perturbations reported in the literature (single-digit to
/// low-double-digit trial counts, not hundreds) without re-scanning from `n = 1` every time.
const LOCAL_SCAN_RADIUS: u64 = 500;

/// Which knob(s) a metric actually uses for power estimation - the same "carry only what a
/// variant reads" idiom `MetricConfig` uses, for the same reason: `Elo` never reads `ci_method`
/// (always Wilson - see [`PowerMetric::new`]), so a mismatched pairing is a compile-time-adjacent
/// impossibility rather than a runtime check repeated at every call site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerMetric {
    WinRate { ci_method: CiMethod },
    SignTest { ci_method: CiMethod },
    Elo,
}

impl PowerMetric {
    pub fn kind(&self) -> MetricKind {
        match self {
            Self::WinRate { .. } => MetricKind::WinRate,
            Self::SignTest { .. } => MetricKind::SignTest,
            Self::Elo => MetricKind::Elo,
        }
    }

    /// Builds a validated `PowerMetric` from flat inputs - mirrors `MetricConfig::new`'s own
    /// compatibility check, reusing the same `IncompatibleCiMethod` error: `compare --metric elo`
    /// itself never accepts anything but Wilson, so a `power` estimate using a CI method `compare`
    /// would refuse to run with would be answering a question `compare` can't actually pose.
    pub fn new(kind: MetricKind, ci_method: CiMethod) -> Result<Self, VeridictError> {
        match kind {
            MetricKind::WinRate => Ok(Self::WinRate { ci_method }),
            MetricKind::SignTest => Ok(Self::SignTest { ci_method }),
            MetricKind::Elo if ci_method != CiMethod::Wilson => {
                Err(VeridictError::IncompatibleCiMethod {
                    method: crate::metrics::ci_method_label(ci_method),
                    metric: crate::metrics::metric_label(kind),
                })
            }
            MetricKind::Elo => Ok(Self::Elo),
            MetricKind::MeanDiff => Err(VeridictError::UnsupportedPowerMetric {
                metric: crate::metrics::metric_label(kind),
            }),
        }
    }

    fn ci_method(&self) -> CiMethod {
        match self {
            Self::WinRate { ci_method } | Self::SignTest { ci_method } => *ci_method,
            Self::Elo => CiMethod::Wilson,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct PowerReport {
    pub schema_version: u32,
    pub metric: MetricKind,
    pub ci_method: &'static str,
    pub min_effect: f64,
    pub assume_effect: f64,
    pub confidence: f64,
    pub target_power: f64,
    pub estimated_trials: u64,
    /// The real exact power *at* `estimated_trials` (always `>= target_power`) - reported
    /// alongside the count since the "sawtooth" property (see module doc) means the achieved
    /// value can overshoot the target by a nontrivial margin, not just graze it.
    pub achieved_power: f64,
    pub method: &'static str,
    pub notes: Vec<String>,
}

/// `p0`/`p1` in proportion space: `winrate`/`sign-test`'s effect is centered on 0.5 (deviation
/// from a 50/50 split, matching those metrics' own report convention); `elo`'s effect is
/// logistic-Elo points, converted via `stats::sprt::score_from_elo` - the same named function
/// `sprt`'s own hypothesis handling uses, rather than re-deriving the transform inline a third
/// time (`estimate_additional_trials` already inlines it once; this reuses the real function
/// instead of inlining a second copy).
fn effect_to_proportion(kind: MetricKind, effect: f64) -> f64 {
    match kind {
        MetricKind::Elo => score_from_elo(effect),
        _ => 0.5 + effect,
    }
}

#[allow(clippy::too_many_arguments)]
pub fn estimate_trials(
    metric: PowerMetric,
    min_effect: f64,
    assume_effect: f64,
    confidence: f64,
    target_power: f64,
    paired_by_id: bool,
) -> Result<PowerReport, VeridictError> {
    if !min_effect.is_finite() || !assume_effect.is_finite() {
        return Err(VeridictError::InvalidThreshold(
            "min_effect/assume_effect must be finite".to_string(),
        ));
    }
    if assume_effect <= min_effect {
        return Err(VeridictError::PowerRequiresEffectGap {
            min_effect,
            assume_effect,
        });
    }
    if !confidence.is_finite() || confidence <= 0.0 || confidence >= 1.0 {
        return Err(VeridictError::InvalidConfidence(confidence));
    }
    if !target_power.is_finite() || target_power <= 0.0 || target_power >= 1.0 {
        return Err(VeridictError::InvalidTargetPower(target_power));
    }

    let kind = metric.kind();
    let ci_method = metric.ci_method();
    let p0 = effect_to_proportion(kind, min_effect);
    let p1 = effect_to_proportion(kind, assume_effect);
    if !(0.0..1.0).contains(&p0) || !(0.0..1.0).contains(&p1) {
        return Err(VeridictError::InvalidThreshold(format!(
            "min_effect/assume_effect must keep the implied proportion strictly inside (0, 1); \
             got p0={p0}, p1={p1} - for winrate/sign-test this means effects strictly inside \
             (-0.5, 0.5)"
        )));
    }

    let (estimated_trials, achieved_power) = estimate_smallest_n(
        p0,
        p1,
        ci_method,
        confidence,
        target_power,
        min_effect,
        assume_effect,
    )?;

    let mut notes = vec![
        "Assumes the true effect is exactly assume_effect; a smaller real effect needs more \
         trials than this number, not fewer - this is a design estimate for how much data to \
         collect, not a guarantee about what a real run will show."
            .to_string(),
    ];
    if paired_by_id {
        notes.push(
            "--paired-by-id was set but does not change this estimate: pairing reduces \
             testcase/opening variance in practice, but the actual reduction depends on the \
             data's within-pair correlation and isn't modeled here - treat this number as a \
             conservative (unpaired) upper bound on trials needed under pairing."
                .to_string(),
        );
    }

    Ok(PowerReport {
        schema_version: crate::report::REPORT_SCHEMA_VERSION,
        metric: kind,
        ci_method: crate::metrics::ci_method_label(ci_method),
        min_effect,
        assume_effect,
        confidence,
        target_power,
        estimated_trials,
        achieved_power,
        method: "exact_binomial_search",
        notes,
    })
}

/// Exact probability that an `n`-trial experiment's `--ci-method` CI lower bound clears `p0`,
/// given the true proportion is `p1`: `sum_{k=0}^{n} Binomial_pmf(n, p1, k) * [CI_lower(k,n) >=
/// p0]`. The pmf is tracked in *log* space (`log_pmf(k+1) = log_pmf(k) + ln(n-k) - ln(k+1) +
/// ln(p1/(1-p1))`, the standard iterative binomial-pmf update, in log form) rather than as a raw
/// probability - `pmf(0) = (1-p1)^n` underflows to exactly `0.0` in `f64` for even moderate `n`
/// whenever `p1` isn't small (e.g. `p1=0.9`, `n>~324`), which would corrupt every later term
/// computed by multiplying forward from it, well before `n` gets anywhere near where this
/// function is actually asked to evaluate. Only exponentiated (`.exp()`) at the point of adding a
/// term into the sum, where a term that's genuinely negligible correctly contributes `~0.0`
/// without having corrupted the terms that aren't.
fn power_at_n(
    n: u64,
    p0: f64,
    p1: f64,
    ci_method: CiMethod,
    confidence: f64,
) -> Result<f64, VeridictError> {
    let log_odds = (p1 / (1.0 - p1)).ln();
    let mut log_pmf = n as f64 * (1.0 - p1).ln();
    let mut power = 0.0;
    for k in 0..=n {
        let (ci_low, _) = match ci_method {
            CiMethod::Wilson => wilson::wilson_ci(k, n, confidence)?,
            CiMethod::Exact => exact::clopper_pearson_ci(k, n, confidence)?,
            CiMethod::Jeffreys => jeffreys::jeffreys_ci(k, n, confidence)?,
        };
        if ci_low >= p0 {
            power += log_pmf.exp();
        }
        if k < n {
            log_pmf += ((n - k) as f64).ln() - ((k + 1) as f64).ln() + log_odds;
        }
    }
    Ok(power.clamp(0.0, 1.0))
}

/// Finds the smallest `n` where `power_at_n` holds `>= target_power` for a stability window of
/// consecutive `n` (see module doc's "sawtooth" section) - not a plain binary search, which would
/// assume strict monotonicity the real function doesn't have. Two phases: a coarse bracket search
/// (doubling then bisection, treating `power_at_n` as monotone *for bracketing purposes only* -
/// its large-scale trend is monotone even though it isn't locally), then a bounded local scan
/// around that bracket to confirm the true smallest stable `n` rather than trusting whichever
/// point bisection happened to land on.
#[allow(clippy::too_many_arguments)]
fn estimate_smallest_n(
    p0: f64,
    p1: f64,
    ci_method: CiMethod,
    confidence: f64,
    target_power: f64,
    min_effect: f64,
    assume_effect: f64,
) -> Result<(u64, f64), VeridictError> {
    let cap_error = || VeridictError::PowerSearchExceededCap {
        cap: MAX_TRIALS,
        min_effect,
        assume_effect,
        target_power,
    };

    // Closed-form normal-approximation seed (unpooled variance under p0/p1, standard one-
    // proportion-vs-fixed-null formula) - a fast starting bracket only, never the reported
    // answer; refined below against the real CI function.
    let z_conf = wilson::inverse_normal_cdf(1.0 - (1.0 - confidence) / 2.0);
    let z_pow = wilson::inverse_normal_cdf(target_power);
    let numerator = z_conf * (p0 * (1.0 - p0)).sqrt() + z_pow * (p1 * (1.0 - p1)).sqrt();
    let seed = ((numerator / (p1 - p0)).powi(2))
        .ceil()
        .clamp(1.0, MAX_TRIALS as f64) as u64;

    let mut hi = seed;
    while power_at_n(hi, p0, p1, ci_method, confidence)? < target_power {
        if hi >= MAX_TRIALS {
            return Err(cap_error());
        }
        hi = (hi.saturating_mul(2)).clamp(hi + 1, MAX_TRIALS);
    }

    let mut lo = 1u64;
    while hi - lo > 1 {
        let mid = lo + (hi - lo) / 2;
        if power_at_n(mid, p0, p1, ci_method, confidence)? >= target_power {
            hi = mid;
        } else {
            lo = mid;
        }
    }

    let scan_start = hi.saturating_sub(LOCAL_SCAN_RADIUS).max(1);
    let scan_end = (hi + LOCAL_SCAN_RADIUS).min(MAX_TRIALS);
    let mut stable_run_start: Option<u64> = None;
    for n in scan_start..=scan_end {
        let power_here = power_at_n(n, p0, p1, ci_method, confidence)?;
        if power_here >= target_power {
            let start = *stable_run_start.get_or_insert(n);
            if n - start + 1 >= STABILITY_WINDOW {
                return Ok((start, power_at_n(start, p0, p1, ci_method, confidence)?));
            }
        } else {
            stable_run_start = None;
        }
    }
    // The bounded local scan didn't confirm a full stability window (the coarse bracket's own
    // crossing point should already be very close to it in practice) - report the coarse
    // crossing itself rather than erroring, since it's still a real n where power holds exactly
    // there; just not confirmed stable across the whole scan window.
    Ok((hi, power_at_n(hi, p0, p1, ci_method, confidence)?))
}

impl PowerReport {
    pub fn to_json_pretty(&self) -> String {
        serde_json::to_string_pretty(self).expect(
            "PowerReport contains only finite fields and strings; serialization cannot fail",
        )
    }

    pub fn to_markdown(&self) -> String {
        let metric = match self.metric {
            MetricKind::WinRate => "winrate",
            MetricKind::SignTest => "sign-test",
            MetricKind::Elo => "elo",
            MetricKind::MeanDiff => "mean-diff",
        };
        let mut out = String::from("# Veridict Power\n\n");
        out.push_str(&format!(
            "Estimated **{}** trials for **{:.0}%** power to reach a passing `{metric}` verdict \
             (`--ci-method {}`), assuming the true effect is exactly **{}** against a pass bar \
             of **{}**, at {:.0}% confidence.\n\n",
            self.estimated_trials,
            self.target_power * 100.0,
            self.ci_method,
            self.assume_effect,
            self.min_effect,
            self.confidence * 100.0,
        ));
        out.push_str(&format!(
            "Achieved power at that trial count: {:.4}.\n\n",
            self.achieved_power
        ));
        if !self.notes.is_empty() {
            out.push_str("Notes:\n\n");
            for note in &self.notes {
                out.push_str(&format!("- {note}\n"));
            }
        }
        out
    }
}

#[derive(Debug, Serialize)]
pub struct SprtPowerReport {
    pub schema_version: u32,
    pub elo0: f64,
    pub elo1: f64,
    pub alpha: f64,
    pub beta: f64,
    pub expected_trials_under_h0: u64,
    pub expected_trials_under_h1: u64,
    pub method: &'static str,
    pub notes: Vec<String>,
}

/// Wald's classical ASN approximation - see the module doc's `--sprt` section for the formula and
/// its overshoot caveat. `SprtConfig::new` is reused verbatim for validation (not re-derived) so a
/// bad `elo0`/`elo1`/`alpha`/`beta` here produces the exact same error `veridict sprt` itself
/// would for the same inputs.
pub fn estimate_sprt_expected_trials(
    elo0: f64,
    elo1: f64,
    alpha: f64,
    beta: f64,
) -> Result<SprtPowerReport, VeridictError> {
    let config = SprtConfig::new(elo0, elo1, alpha, beta)?;
    let b = bounds(config.alpha, config.beta);
    let p0 = score_from_elo(config.elo0);
    let p1 = score_from_elo(config.elo1);

    let expected_trials = |true_p: f64, alpha_prime: f64| -> Result<u64, VeridictError> {
        let e_z = true_p * llr_delta(true, p0, p1) + (1.0 - true_p) * llr_delta(false, p0, p1);
        let e_n = (alpha_prime * b.upper + (1.0 - alpha_prime) * b.lower) / e_z;
        if !e_n.is_finite() || e_n < 0.0 {
            return Err(VeridictError::InvalidThreshold(format!(
                "SPRT ASN computation produced a non-physical expected sample size ({e_n}) for \
                 elo0={elo0}, elo1={elo1}, alpha={alpha}, beta={beta} - this shouldn't happen for \
                 valid elo0 < elo1, please report this as a bug"
            )));
        }
        Ok(e_n.ceil() as u64)
    };

    let expected_trials_under_h0 = expected_trials(p0, config.alpha)?;
    let expected_trials_under_h1 = expected_trials(p1, 1.0 - config.beta)?;

    Ok(SprtPowerReport {
        schema_version: crate::report::REPORT_SCHEMA_VERSION,
        elo0: config.elo0,
        elo1: config.elo1,
        alpha: config.alpha,
        beta: config.beta,
        expected_trials_under_h0,
        expected_trials_under_h1,
        method: "wald_asn_approximation",
        notes: vec![
            "expected_trials_under_h0/h1 are the two endpoint cases (the true strength sitting \
             exactly at elo0 or elo1) - a real candidate whose true strength lies between elo0 \
             and elo1, the common case since you're running SPRT precisely because that strength \
             is unknown, needs substantially more trials than either endpoint: a Wald SPRT's \
             expected sample size peaks near the midpoint between the two hypotheses, not at \
             either one. Budget above these two numbers, not at them, when the candidate's true \
             strength is genuinely uncertain."
                .to_string(),
            "Wald's classical Average Sample Number approximation - ignores \"overshoot\" (the \
             LLR's excess past a boundary at the moment of stopping), so a real run typically \
             needs somewhat more trials than this number in practice."
                .to_string(),
            "Counts decisive trials only (same as --sprt-variant wald itself) - a draw-heavy \
             testcase needs more real games than this number, since draws don't move the LLR at \
             all. Use --sprt-variant trinomial/pentanomial for draw-heavy testing."
                .to_string(),
        ],
    })
}

impl SprtPowerReport {
    pub fn to_json_pretty(&self) -> String {
        serde_json::to_string_pretty(self).expect(
            "SprtPowerReport contains only finite fields and strings; serialization cannot fail",
        )
    }

    pub fn to_markdown(&self) -> String {
        let mut out = String::from("# Veridict Power (SPRT)\n\n");
        out.push_str(&format!(
            "Wald SPRT with elo0={}, elo1={}, alpha={}, beta={}: expected **{}** trials under H0, \
             expected **{}** trials under H1 (Wald's ASN approximation).\n\n",
            self.elo0,
            self.elo1,
            self.alpha,
            self.beta,
            self.expected_trials_under_h0,
            self.expected_trials_under_h1,
        ));
        if !self.notes.is_empty() {
            out.push_str("Notes:\n\n");
            for note in &self.notes {
                out.push_str(&format!("- {note}\n"));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn power_metric_rejects_non_wilson_for_elo() {
        assert!(matches!(
            PowerMetric::new(MetricKind::Elo, CiMethod::Exact),
            Err(VeridictError::IncompatibleCiMethod { .. })
        ));
        assert!(PowerMetric::new(MetricKind::Elo, CiMethod::Wilson).is_ok());
    }

    #[test]
    fn power_metric_rejects_mean_diff() {
        assert!(matches!(
            PowerMetric::new(MetricKind::MeanDiff, CiMethod::Wilson),
            Err(VeridictError::UnsupportedPowerMetric { .. })
        ));
    }

    #[test]
    fn estimate_trials_requires_assume_effect_greater_than_min_effect() {
        let metric = PowerMetric::WinRate {
            ci_method: CiMethod::Wilson,
        };
        assert!(matches!(
            estimate_trials(metric, 0.05, 0.05, 0.95, 0.8, false),
            Err(VeridictError::PowerRequiresEffectGap { .. })
        ));
        assert!(matches!(
            estimate_trials(metric, 0.05, 0.03, 0.95, 0.8, false),
            Err(VeridictError::PowerRequiresEffectGap { .. })
        ));
    }

    #[test]
    fn estimate_trials_achieved_power_meets_target() {
        let metric = PowerMetric::WinRate {
            ci_method: CiMethod::Wilson,
        };
        let report = estimate_trials(metric, 0.02, 0.10, 0.95, 0.8, false).unwrap();
        assert!(report.achieved_power >= 0.8);
        assert!(report.estimated_trials > 0);
    }

    #[test]
    fn a_larger_effect_gap_needs_fewer_trials() {
        let metric = PowerMetric::Elo;
        let small_gap = estimate_trials(metric, 10.0, 20.0, 0.95, 0.8, false).unwrap();
        let large_gap = estimate_trials(metric, 10.0, 60.0, 0.95, 0.8, false).unwrap();
        assert!(
            large_gap.estimated_trials < small_gap.estimated_trials,
            "large gap {} should need fewer trials than small gap {}",
            large_gap.estimated_trials,
            small_gap.estimated_trials
        );
    }

    #[test]
    fn tiny_effect_gap_hits_the_cap_with_a_clear_error() {
        let metric = PowerMetric::WinRate {
            ci_method: CiMethod::Wilson,
        };
        assert!(matches!(
            estimate_trials(metric, 0.0001, 0.00011, 0.95, 0.999, false),
            Err(VeridictError::PowerSearchExceededCap { .. })
        ));
    }

    #[test]
    fn notes_mention_paired_by_id_only_when_set() {
        let metric = PowerMetric::WinRate {
            ci_method: CiMethod::Wilson,
        };
        let without = estimate_trials(metric, 0.02, 0.10, 0.95, 0.8, false).unwrap();
        let with = estimate_trials(metric, 0.02, 0.10, 0.95, 0.8, true).unwrap();
        assert_eq!(without.notes.len(), 1);
        assert_eq!(with.notes.len(), 2);
        assert!(with.notes[1].contains("paired-by-id"));
    }

    // Hand-checkable sanity anchor: at n found by the search, the reported achieved_power must
    // match an independently-recomputed exact sum over the same n (proves `estimate_smallest_n`
    // and `power_at_n` agree, not just that each individually runs without panicking).
    #[test]
    fn achieved_power_matches_an_independent_recomputation_at_the_same_n() {
        let metric = PowerMetric::WinRate {
            ci_method: CiMethod::Wilson,
        };
        let report = estimate_trials(metric, 0.02, 0.08, 0.95, 0.8, false).unwrap();
        let p0 = 0.5 + 0.02;
        let p1 = 0.5 + 0.08;
        let recomputed =
            power_at_n(report.estimated_trials, p0, p1, CiMethod::Wilson, 0.95).unwrap();
        assert!((recomputed - report.achieved_power).abs() < 1e-9);
    }

    #[test]
    fn sprt_asn_is_positive_and_finite_for_a_normal_config() {
        let report = estimate_sprt_expected_trials(0.0, 20.0, 0.05, 0.05).unwrap();
        assert!(report.expected_trials_under_h0 > 0);
        assert!(report.expected_trials_under_h1 > 0);
    }

    #[test]
    fn sprt_asn_reuses_sprt_config_validation() {
        assert!(matches!(
            estimate_sprt_expected_trials(20.0, 0.0, 0.05, 0.05),
            Err(VeridictError::InvalidThreshold(_))
        ));
        assert!(matches!(
            estimate_sprt_expected_trials(0.0, 20.0, 1.5, 0.05),
            Err(VeridictError::InvalidThreshold(_))
        ));
    }

    #[test]
    fn sprt_asn_a_larger_elo_gap_needs_fewer_expected_trials() {
        let small_gap = estimate_sprt_expected_trials(0.0, 10.0, 0.05, 0.05).unwrap();
        let large_gap = estimate_sprt_expected_trials(0.0, 60.0, 0.05, 0.05).unwrap();
        assert!(large_gap.expected_trials_under_h0 < small_gap.expected_trials_under_h0);
        assert!(large_gap.expected_trials_under_h1 < small_gap.expected_trials_under_h1);
    }
}
