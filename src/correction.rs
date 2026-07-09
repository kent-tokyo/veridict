//! Multiple-comparison correction for a `compare --metric` run's metric family (`--correction
//! bonferroni|holm`).
//!
//! **The problem.** `compare --metric elo --metric winrate --metric sign-test` already lets a
//! user run several metrics against the same candidate in one call, combined via
//! `verdict::aggregate`. But each metric's own pass/fail decision is made independently at the
//! stated `--confidence` - run enough metrics (or, in a broader campaign, enough candidates) and
//! the chance that *something* clears its bar by luck alone climbs. That directly undermines this
//! project's own "a false pass is worse than an inconclusive result" bias (see `verdict`'s doc):
//! an uncorrected multi-metric family is more likely to produce a lucky pass than any single
//! metric run alone would be.
//!
//! **The family-error target.** `compare`'s pass rule already reads a *two-sided*
//! `(1-confidence)` CI's *lower* bound as a one-sided pass signal - a two-sided interval splits
//! its error budget evenly between both tails, so a single, uncorrected metric today already has
//! a one-sided false-pass rate of `alpha/2` (e.g. 0.025 at the default 95% confidence), not the
//! nominal `alpha`. The natural, and only defensible, correction target is therefore "running `m`
//! metrics together is no more dangerous than running one" - i.e. keep the *family's* one-sided
//! false-pass rate at that same `alpha/2`, not the nominal `alpha` itself (which would let a
//! "corrected" multi-metric family tolerate a *higher* false-pass rate than a single uncorrected
//! metric already has today - backwards for this project). This falls straight out of standard
//! textbook Bonferroni simultaneous confidence intervals (Dunn 1961; Miller, *Simultaneous
//! Statistical Inference*, 1966): recompute each test's ordinary, symmetric, two-sided CI at
//! confidence `1 - alpha/m` (the same `alpha = 1-confidence` the tool already uses, split across
//! the family - no extra factor of anything) and re-run the *existing, unchanged* `verdict::decide`
//! against it. Because a valid CI always has `ci_low <= ci_high`, and this only ever *widens* an
//! already-`Pass` report's interval (`ci_low >= pass_above > fail_below` at the narrower, original
//! confidence), the widened `ci_high` can never newly satisfy `ci_high <= fail_below` - correction
//! can only ever move an unadjusted `Pass` to `Inconclusive`, never manufacture a `Fail`. That
//! asymmetry isn't a special case implemented below; it's a direct consequence of widening plus
//! `decide`'s existing logic, and it's exactly what this project's bias calls for.
//!
//! **`achieved_alpha`: the p-value-equivalent both corrections share.** Rather than literally
//! recomputing a full CI and re-running `decide` for every candidate confidence level, this module
//! binary-searches for the smallest `gamma` (same units as `1 - confidence`) at which a report's
//! own CI, built from the same observed data at confidence `1 - gamma`, would still have its lower
//! bound clear `pass_above`. This is the standard CI-test duality (a `1-gamma` CI is exactly the
//! set of null values not rejected by a `gamma`-level test) - rigorous for Clopper-Pearson (exact
//! binomial tail test), Wilson (score test), and Jeffreys (a monotone Beta-posterior family), all
//! three provably monotone in `gamma`, so bisection is safe. Bonferroni then compares
//! `achieved_alpha <= alpha/m` directly; Holm compares each rank's achieved alpha to
//! `alpha/(m-k+1)` in ascending order, stopping at the first failure. Both read straight off the
//! CI functions `compare` already uses (`stats::wilson`/`stats::exact`/`stats::jeffreys`) - no
//! separate math for Bonferroni vs. Holm, one search drives both.
//!
//! **`mean-diff` is excluded from its own correction but still counts toward `family_size`.**
//! There is no closed-form CI-at-a-hypothetical-confidence function for a bootstrap CI without
//! real resampled data (same reason `verdict::estimate_additional_trials`/`power` both special-
//! case it) - a mean-diff report's own verdict is left unadjusted. Excluding it from
//! `family_size` entirely would under-count the real multiplicity risk the *other* metrics in the
//! same run are actually exposed to, so it still counts - the conservative choice, consistent with
//! "false pass worse than inconclusive."

use crate::report::Report;
use crate::stats::elo;
use crate::stats::{exact, jeffreys, wilson};
use crate::{CiMethod, MetricConfig, MetricKind, Verdict};

/// Which multiple-comparison correction (if any) `compare --correction` applies to a multi-metric
/// family. `None` is the default - `apply_correction` returns immediately without touching any
/// report, so an unrequested run's JSON stays byte-identical to before this existed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Correction {
    None,
    Bonferroni,
    Holm,
}

impl Correction {
    fn label(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Bonferroni => "bonferroni",
            Self::Holm => "holm",
        }
    }

    /// Sentence-initial form of `label`, for appending a new sentence to `Report.reason`.
    fn title_label(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::Bonferroni => "Bonferroni",
            Self::Holm => "Holm",
        }
    }
}

/// Bisection stops here rather than searching all the way to `gamma = 0` - past this point the
/// implied CI is already wider than any real analysis would use, and 60 halvings of the starting
/// bracket already converge far past `f64` precision.
const ACHIEVED_ALPHA_FLOOR: f64 = 1e-12;
const BISECTION_ITERATIONS: u32 = 60;

/// Smallest `gamma` (same units as `1 - confidence`) at which this report's own CI, recomputed at
/// confidence `1 - gamma` from the same observed data, would still have its lower bound (in the
/// metric's native "effect" units - a proportion offset from zero for winrate/sign-test, Elo
/// points for elo) clear `pass_above`. `None` for `mean-diff` (no closed-form CI-at-a-hypothetical-
/// confidence function exists - see module doc) - callers must leave such a report's verdict
/// unadjusted. Only ever called for a report whose verdict is already `Pass` at `confidence`, so
/// `gamma_orig = 1 - confidence` is a known-passing search bracket, not re-verified here.
fn achieved_alpha(
    metric: MetricKind,
    ci_method: CiMethod,
    successes: u64,
    paired_count: u64,
    effect: f64,
    pass_above: f64,
    confidence: f64,
) -> Option<f64> {
    if metric == MetricKind::MeanDiff {
        return None;
    }

    // Recovers the same proportion `estimate_additional_trials` recovers from `effect` for
    // exactly the same reason: elo's `effect` has already been through `elo_from_score`'s
    // logistic transform, so this is that transform's inverse.
    let p_hat = if metric == MetricKind::Elo {
        1.0 / (1.0 + 10f64.powf(-effect / 400.0))
    } else {
        effect + 0.5
    };

    let ci_lower_effect_at = |gamma: f64| -> Option<f64> {
        let candidate_confidence = 1.0 - gamma;
        if metric == MetricKind::Elo {
            let (lo, _) =
                wilson::wilson_ci_from_proportion(p_hat, paired_count as f64, candidate_confidence)
                    .ok()?;
            Some(elo::elo_from_score(lo))
        } else {
            let (lo, _) = match ci_method {
                CiMethod::Wilson => {
                    wilson::wilson_ci(successes, paired_count, candidate_confidence).ok()?
                }
                CiMethod::Jeffreys => {
                    jeffreys::jeffreys_ci(successes, paired_count, candidate_confidence).ok()?
                }
                CiMethod::Exact => {
                    exact::clopper_pearson_ci(successes, paired_count, candidate_confidence).ok()?
                }
            };
            Some(lo - 0.5)
        }
    };

    let gamma_orig = 1.0 - confidence;
    let mut lo = ACHIEVED_ALPHA_FLOOR;
    let mut hi = gamma_orig;
    for _ in 0..BISECTION_ITERATIONS {
        let mid = lo + (hi - lo) / 2.0;
        match ci_lower_effect_at(mid) {
            Some(lower) if lower >= pass_above => hi = mid,
            _ => lo = mid,
        }
    }
    Some(hi)
}

/// Holm's step-down rejection: sort correctable reports (those with a `Some` achieved alpha) by
/// achieved alpha ascending, compare each rank `k` (1-based) to `alpha/(family_size-k+1)`, and
/// **stop rejecting at the first failure** - a report past an early failure is held back
/// regardless of its own achieved alpha, because Holm's family-wise error guarantee (Holm 1979)
/// depends on the sequential stop, not independent per-test comparisons. Returns, indexed the same
/// as `achieved`: whether each report was rejected (`Some(true)`/`Some(false)`, `None` if
/// uncorrectable) and the per-rank threshold it was actually compared against.
fn holm_reject(
    achieved: &[Option<f64>],
    alpha: f64,
    family_size: usize,
) -> (Vec<Option<bool>>, Vec<Option<f64>>) {
    let mut order: Vec<usize> = (0..achieved.len())
        .filter(|&i| achieved[i].is_some())
        .collect();
    order.sort_by(|&a, &b| achieved[a].unwrap().total_cmp(&achieved[b].unwrap()));

    let mut rejected = vec![None; achieved.len()];
    let mut thresholds = vec![None; achieved.len()];
    let mut still_rejecting = true;
    for (rank, &idx) in order.iter().enumerate() {
        let k = rank + 1;
        let threshold = alpha / (family_size - k + 1) as f64;
        thresholds[idx] = Some(threshold);
        still_rejecting = still_rejecting && achieved[idx].unwrap() <= threshold;
        rejected[idx] = Some(still_rejecting);
    }
    (rejected, thresholds)
}

/// Applies `correction` to `reports` in place: `reports`/`configs` must be the same length and
/// index-aligned (one `MetricConfig` per `Report`, needed to recover each report's `ci_method` -
/// not itself stored on `Report`). Only ever adjusts a report whose *unadjusted* verdict is
/// `Pass` (correction exists to catch a lucky pass among many attempts, not to second-guess a fail
/// or inconclusive). `family_size = reports.len()` always - a single-metric run degenerates both
/// Bonferroni and Holm to a no-op (`alpha/1 = alpha`, exactly the report's own existing pass
/// condition), so callers don't need to special-case it away.
pub fn apply_correction(
    reports: &mut [Report],
    configs: &[MetricConfig],
    correction: Correction,
    confidence: f64,
) {
    if correction == Correction::None {
        return;
    }
    let family_size = reports.len();
    let alpha = 1.0 - confidence;

    let achieved: Vec<Option<f64>> = reports
        .iter()
        .zip(configs)
        .map(|(report, config)| {
            if report.verdict != Verdict::Pass {
                return None;
            }
            achieved_alpha(
                report.metric,
                config.ci_method(),
                report.candidate_count,
                report.paired_count,
                report.effect,
                report.pass_above,
                confidence,
            )
        })
        .collect();

    let (rejected, thresholds): (Vec<Option<bool>>, Vec<Option<f64>>) = match correction {
        Correction::None => unreachable!("returned above"),
        Correction::Bonferroni => {
            let uniform = alpha / family_size as f64;
            (
                achieved.iter().map(|a| a.map(|v| v <= uniform)).collect(),
                achieved.iter().map(|a| a.map(|_| uniform)).collect(),
            )
        }
        Correction::Holm => holm_reject(&achieved, alpha, family_size),
    };

    for (i, report) in reports.iter_mut().enumerate() {
        let unadjusted_verdict = report.verdict;

        if report.metric == MetricKind::MeanDiff && unadjusted_verdict == Verdict::Pass {
            report.warnings.push(format!(
                "excluded from its own {method} multiple-comparison correction (mean-diff has no \
                 closed-form CI at an adjusted confidence level); still counts toward \
                 family_size={family_size}",
                method = correction.label(),
            ));
        }

        if let Some(reject) = rejected[i] {
            let a = achieved[i].expect("rejected[i] is Some only when achieved[i] is Some");
            let t = thresholds[i].expect("thresholds[i] is Some whenever rejected[i] is Some");
            let method = correction.title_label();
            if reject {
                report.reason.push_str(&format!(
                    ". {method} correction (family_size={family_size}) confirms: achieved \
                     significance {a:.6} <= the corrected threshold {t:.6}."
                ));
            } else if a <= t {
                report.verdict = Verdict::Inconclusive;
                report.reason.push_str(&format!(
                    ". {method} correction (family_size={family_size}): achieved significance \
                     {a:.6} would clear its own threshold {t:.6}, but an earlier-ranked test in \
                     the family failed, so Holm's sequential rule holds this one back too - \
                     downgraded from pass to inconclusive."
                ));
            } else {
                report.verdict = Verdict::Inconclusive;
                report.reason.push_str(&format!(
                    ". {method} correction (family_size={family_size}): achieved significance \
                     {a:.6} exceeds the corrected threshold {t:.6} - downgraded from pass to \
                     inconclusive."
                ));
            }
        }

        report.correction_method = Some(correction.label());
        report.family_size = Some(family_size);
        report.achieved_alpha = achieved[i];
        report.adjusted_alpha_threshold = thresholds[i];
        report.unadjusted_verdict = Some(unadjusted_verdict);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::{DataQuality, REPORT_SCHEMA_VERSION};
    use crate::stats::wilson;

    #[allow(clippy::too_many_arguments)]
    fn report_with(
        metric: MetricKind,
        verdict: Verdict,
        candidate_count: u64,
        paired_count: u64,
        effect: f64,
        ci_low: f64,
        ci_high: f64,
        pass_above: f64,
        fail_below: f64,
    ) -> Report {
        Report {
            schema_version: REPORT_SCHEMA_VERSION,
            verdict,
            metric,
            baseline_count: paired_count - candidate_count,
            candidate_count,
            paired_count,
            effect,
            confidence: 0.95,
            ci_low,
            ci_high,
            pass_above,
            fail_below,
            timeouts: 0,
            crashes: 0,
            invalid: 0,
            failure_breakdown: crate::metrics::FailureBreakdown::default(),
            reason: "test fixture".to_string(),
            estimated_additional_trials: None,
            warnings: Vec::new(),
            data_quality: DataQuality::default(),
            correction_method: None,
            family_size: None,
            achieved_alpha: None,
            adjusted_alpha_threshold: None,
            unadjusted_verdict: None,
        }
    }

    fn winrate_pass(candidate_wins: u64, n: u64, pass_above: f64) -> Report {
        let (lo, hi) = wilson::wilson_ci(candidate_wins, n, 0.95).unwrap();
        let p_hat = candidate_wins as f64 / n as f64;
        report_with(
            MetricKind::WinRate,
            Verdict::Pass,
            candidate_wins,
            n,
            p_hat - 0.5,
            lo - 0.5,
            hi - 0.5,
            pass_above,
            -pass_above,
        )
    }

    fn winrate_config() -> MetricConfig {
        MetricConfig::WinRate {
            ci_method: CiMethod::Wilson,
            failure_policy: crate::FailurePolicy::ReportOnly,
        }
    }

    #[test]
    fn stronger_effect_has_smaller_achieved_alpha() {
        let weak = achieved_alpha(
            MetricKind::WinRate,
            CiMethod::Wilson,
            58,
            100,
            0.08,
            0.02,
            0.95,
        )
        .unwrap();
        let strong = achieved_alpha(
            MetricKind::WinRate,
            CiMethod::Wilson,
            80,
            100,
            0.30,
            0.02,
            0.95,
        )
        .unwrap();
        assert!(
            strong < weak,
            "a larger, cleaner effect should need a smaller achieved alpha to still pass: \
             strong={strong}, weak={weak}"
        );
    }

    #[test]
    fn achieved_alpha_self_consistency_across_all_ci_methods() {
        // CI-test duality check (mirrors verdict.rs's
        // winrate_wilson_search_matches_a_direct_wilson_recompute): achieved_alpha's own crossing
        // point must reproduce ci_low == pass_above when the CI is recomputed at confidence
        // (1 - achieved_alpha) via the SAME function - proves the search actually lands on the
        // right value, not just "smaller for stronger effects" (the test above). Every other test
        // in this module uses the default Wilson method - this is the only coverage for the
        // Exact/Jeffreys branches of `achieved_alpha`'s internal closure.
        // 64/100 verified (via a real `compare` run) to be a genuine pass at 95% confidence under
        // all three CI methods - Exact is always at least as wide as Wilson, so a smaller margin
        // passes under Wilson/Jeffreys but not Exact, which would violate achieved_alpha's own
        // "already passing at confidence" precondition.
        let successes = 64u64;
        let n = 100u64;
        let effect = successes as f64 / n as f64 - 0.5;
        let pass_above = 0.02;

        for ci_method in [CiMethod::Wilson, CiMethod::Exact, CiMethod::Jeffreys] {
            let gamma = achieved_alpha(
                MetricKind::WinRate,
                ci_method,
                successes,
                n,
                effect,
                pass_above,
                0.95,
            )
            .unwrap();
            let confidence = 1.0 - gamma;
            let (lo, _hi) = match ci_method {
                CiMethod::Wilson => wilson::wilson_ci(successes, n, confidence).unwrap(),
                CiMethod::Exact => exact::clopper_pearson_ci(successes, n, confidence).unwrap(),
                CiMethod::Jeffreys => jeffreys::jeffreys_ci(successes, n, confidence).unwrap(),
            };
            let ci_low_effect = lo - 0.5;
            assert!(
                (ci_low_effect - pass_above).abs() < 1e-6,
                "{ci_method:?}: recomputed ci_low {ci_low_effect} should land on pass_above \
                 {pass_above}, gamma={gamma}"
            );
        }
    }

    #[test]
    fn mean_diff_has_no_achieved_alpha() {
        assert_eq!(
            achieved_alpha(
                MetricKind::MeanDiff,
                CiMethod::Wilson,
                0,
                100,
                0.05,
                0.02,
                0.95
            ),
            None
        );
    }

    #[test]
    fn none_leaves_reports_completely_untouched() {
        let mut reports = vec![winrate_pass(80, 100, 0.02)];
        let configs = vec![winrate_config()];
        apply_correction(&mut reports, &configs, Correction::None, 0.95);
        assert_eq!(reports[0].verdict, Verdict::Pass);
        assert_eq!(reports[0].correction_method, None);
        assert_eq!(reports[0].family_size, None);
        assert_eq!(reports[0].achieved_alpha, None);
        assert_eq!(reports[0].unadjusted_verdict, None);
    }

    #[test]
    fn single_metric_family_degenerates_to_a_no_op() {
        // family_size=1: alpha/1 == alpha, exactly the report's own existing pass condition.
        let mut reports = vec![winrate_pass(80, 100, 0.02)];
        let configs = vec![winrate_config()];
        apply_correction(&mut reports, &configs, Correction::Bonferroni, 0.95);
        assert_eq!(reports[0].verdict, Verdict::Pass);
        apply_correction(&mut reports, &configs, Correction::Holm, 0.95);
        assert_eq!(reports[0].verdict, Verdict::Pass);
    }

    #[test]
    fn bonferroni_downgrades_a_marginal_pass_in_a_larger_family() {
        // 62/100 is a genuine, bare pass at 95% confidence (ci_low ~= 0.0221, just past
        // pass_above=0.02 - verified against a real `compare` run before writing this test) that
        // survives alone but not split three ways.
        let mut reports = vec![winrate_pass(62, 100, 0.02)];
        let configs = vec![winrate_config()];
        apply_correction(&mut reports, &configs, Correction::None, 0.95);
        assert_eq!(reports[0].verdict, Verdict::Pass);

        let single = winrate_pass(62, 100, 0.02);
        let mut family = vec![
            clone_report(&single),
            clone_report(&single),
            clone_report(&single),
        ];
        let configs = vec![winrate_config(), winrate_config(), winrate_config()];
        apply_correction(&mut family, &configs, Correction::Bonferroni, 0.95);
        assert_eq!(family[0].verdict, Verdict::Inconclusive);
        assert_eq!(family[0].unadjusted_verdict, Some(Verdict::Pass));
        assert_eq!(family[0].family_size, Some(3));
    }

    fn clone_report(r: &Report) -> Report {
        Report {
            schema_version: r.schema_version,
            verdict: r.verdict,
            metric: r.metric,
            baseline_count: r.baseline_count,
            candidate_count: r.candidate_count,
            paired_count: r.paired_count,
            effect: r.effect,
            confidence: r.confidence,
            ci_low: r.ci_low,
            ci_high: r.ci_high,
            pass_above: r.pass_above,
            fail_below: r.fail_below,
            timeouts: r.timeouts,
            crashes: r.crashes,
            invalid: r.invalid,
            failure_breakdown: r.failure_breakdown,
            reason: r.reason.clone(),
            estimated_additional_trials: r.estimated_additional_trials,
            warnings: r.warnings.clone(),
            data_quality: r.data_quality,
            correction_method: r.correction_method,
            family_size: r.family_size,
            achieved_alpha: r.achieved_alpha,
            adjusted_alpha_threshold: r.adjusted_alpha_threshold,
            unadjusted_verdict: r.unadjusted_verdict,
        }
    }

    #[test]
    fn holm_sequential_stop_holds_back_a_report_that_would_pass_its_own_threshold() {
        // Three reports ranked by achieved alpha ascending: rank 1 fails its own (tightest)
        // threshold alpha/3; ranks 2 and 3 must then be held back too, even if rank 3's own
        // achieved alpha would individually clear its (loosest) threshold alpha/1 in isolation.
        let alpha = 0.05;
        let achieved = vec![
            Some(alpha / 3.0 + 0.001), // rank 1: fails alpha/3 by a hair
            Some(alpha / 2.0 - 0.001), // rank 2: would pass alpha/2 alone
            Some(alpha - 0.001),       // rank 3: would pass alpha/1 alone
        ];
        let (rejected, _) = holm_reject(&achieved, alpha, 3);
        assert_eq!(rejected, vec![Some(false), Some(false), Some(false)]);
    }

    #[test]
    fn holm_rejects_every_report_when_the_whole_ordered_prefix_holds() {
        let alpha = 0.05;
        let achieved = vec![
            Some(alpha / 3.0 - 1e-6),
            Some(alpha / 2.0 - 1e-6),
            Some(alpha - 1e-6),
        ];
        let (rejected, _) = holm_reject(&achieved, alpha, 3);
        assert_eq!(rejected, vec![Some(true), Some(true), Some(true)]);
    }

    #[test]
    fn mean_diff_report_counts_toward_family_size_but_keeps_its_own_verdict_and_gets_a_warning() {
        let mean_diff = report_with(
            MetricKind::MeanDiff,
            Verdict::Pass,
            0,
            100,
            0.05,
            0.01,
            0.09,
            0.0,
            0.0,
        );
        let winrate = winrate_pass(62, 100, 0.02);
        let mut reports = vec![mean_diff, winrate];
        let configs = vec![
            MetricConfig::MeanDiff {
                bootstrap_method: crate::BootstrapMethod::Percentile,
            },
            winrate_config(),
        ];
        apply_correction(&mut reports, &configs, Correction::Bonferroni, 0.95);

        assert_eq!(reports[0].verdict, Verdict::Pass);
        assert_eq!(reports[0].achieved_alpha, None);
        assert_eq!(reports[0].family_size, Some(2));
        assert!(reports[0].warnings.iter().any(|w| w.contains("mean-diff")));
    }
}
