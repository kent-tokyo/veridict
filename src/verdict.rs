//! Turns an effect size + confidence interval into a pass/fail/inconclusive
//! decision.
//!
//! The gate compares the CI, not the point estimate, against the thresholds:
//! a candidate only passes if it beats `pass_above` *even at the pessimistic
//! (lower) end* of the interval, and only fails if it's below `fail_below`
//! *even at the optimistic (upper) end*. Anything else is inconclusive. This
//! is deliberately conservative per AGENTS.md: "a false pass is worse than
//! an inconclusive result."

use crate::error::VeridictError;
use crate::stats::{elo, exact, jeffreys, wilson};
use crate::{CiMethod, MetricKind, Verdict};

pub struct Thresholds {
    pub pass_above: f64,
    pub fail_below: f64,
}

impl Thresholds {
    pub fn new(pass_above: f64, fail_below: f64) -> Result<Self, VeridictError> {
        if !pass_above.is_finite() || !fail_below.is_finite() {
            return Err(VeridictError::InvalidThreshold(
                "pass_above/fail_below must be finite".to_string(),
            ));
        }
        if fail_below > pass_above {
            return Err(VeridictError::InvalidThreshold(format!(
                "fail_below ({fail_below}) must be <= pass_above ({pass_above})"
            )));
        }
        Ok(Self {
            pass_above,
            fail_below,
        })
    }

    /// Symmetric thresholds around zero: pass above `+min_effect`, fail
    /// below `-min_effect`. What `--min-effect` maps to on the CLI.
    pub fn symmetric(min_effect: f64) -> Result<Self, VeridictError> {
        Self::new(min_effect, -min_effect)
    }
}

/// Combines several metrics' verdicts into one overall verdict for a
/// multi-metric run: any `Fail` sinks the whole run, else any
/// `Inconclusive` holds it back, else `Pass`. An empty iterator has nothing
/// to fail or hold back on, so it counts as `Pass` (compare_many never
/// calls this with zero metrics in practice, since the CLI requires at
/// least one `--metric`).
pub fn aggregate(verdicts: impl IntoIterator<Item = Verdict>) -> Verdict {
    let mut worst = Verdict::Pass;
    for v in verdicts {
        match v {
            Verdict::Fail => return Verdict::Fail,
            Verdict::Inconclusive => worst = Verdict::Inconclusive,
            Verdict::Pass => {}
        }
    }
    worst
}

/// Rough estimate of how many *additional* trials (beyond `paired_count`)
/// would be needed to turn an `Inconclusive` verdict decisive, assuming the
/// CI half-width keeps shrinking as `O(1/sqrt(n))` (the standard CLT
/// scaling) and the effect size itself doesn't move. `None` when there's
/// nothing meaningful to suggest:
/// - the verdict is already `Pass`/`Fail` (nothing left to resolve),
/// - `paired_count == 0` (no rate to scale from),
/// - `effect` is inside `[fail_below, pass_above]` (inclusive) - the "dead
///   zone". This is deliberate, not a missing case: shrinking the CI
///   *around a fixed point estimate already inside the dead zone* can never
///   cross either boundary, no matter how large `n` gets - verified
///   concretely (effect=0.0, thresholds=+-0.02, n=100): the naive formula
///   suggests ~2313 more trials, but the verdict is still `Inconclusive` at
///   that n, and still `Inconclusive` at n=10,000,000. Only a genuinely
///   different effect size resolves a dead-zone result, not more data alone.
/// - the current CI half-width is non-finite, non-positive, or already at
///   or below the target (can happen since real CIs, e.g. Wilson's, aren't
///   perfectly symmetric around `effect` - this formula assumes they are).
///
/// For `winrate`/`sign-test`/`elo`, this binary-searches against the exact
/// same tested CI function the report displays (`wilson`/`jeffreys`/`exact`,
/// per `ci_method` - Wilson/Jeffreys/Exact differ in width at the same `n`,
/// so searching the wrong one would contradict the report's own shown CI),
/// holding the point estimate fixed - not an approximation, an exact search
/// against real, already-tested math. `mean-diff`/`quantile-diff` are the exception:
/// there is no "CI width at a hypothetical `n`" function for a bootstrap CI
/// without real resampled data, so both keep the original `O(1/sqrt(n))` CLT-
/// scaling model, with its own known, quantified bias: verified within
/// ~1.5% of an actual re-run for a clean 4x sample-size jump at moderate n,
/// but a real ~18% *under*-estimate at n=100, because e.g. Wilson's CI also
/// shrinks via an `O(z^2/n)` recentering term this simple `1/sqrt(n)` model
/// doesn't capture (measured for `mean-diff`; `quantile-diff` reuses the same
/// model unverified for its own bootstrap CI, since it's the same "no closed
/// form" shape). Treat `mean-diff`/`quantile-diff`'s result as "roughly this
/// many, plausibly more," not a guarantee; the other metrics' result is exact
/// (mod float precision) for the stated model (point estimate unchanged).
#[allow(clippy::too_many_arguments)]
pub fn estimate_additional_trials(
    metric: MetricKind,
    ci_method: CiMethod,
    verdict: Verdict,
    effect: f64,
    ci_low: f64,
    ci_high: f64,
    paired_count: u64,
    thresholds: &Thresholds,
    confidence: f64,
) -> Option<u64> {
    if verdict != Verdict::Inconclusive || paired_count == 0 {
        return None;
    }

    let target_half_width = if effect > thresholds.pass_above {
        effect - thresholds.pass_above
    } else if effect < thresholds.fail_below {
        thresholds.fail_below - effect
    } else {
        return None;
    };

    let current_half_width = (ci_high - ci_low) / 2.0;
    if !current_half_width.is_finite() || current_half_width <= target_half_width {
        return None;
    }

    let ratio = current_half_width / target_half_width;
    let naive_required_n = paired_count as f64 * ratio * ratio;
    if !naive_required_n.is_finite() || naive_required_n >= u64::MAX as f64 {
        return Some(u64::MAX);
    }
    let naive_estimate = (naive_required_n as u64).saturating_sub(paired_count);

    if metric == MetricKind::MeanDiff || metric == MetricKind::QuantileDiff {
        return Some(naive_estimate);
    }

    // Elo's p_hat isn't in `effect` directly (it's already been through
    // `elo_from_score`'s logistic transform) - recover it via that
    // transform's inverse, the standard logistic Elo win probability.
    let p_hat = if metric == MetricKind::Elo {
        1.0 / (1.0 + 10f64.powf(-effect / 400.0))
    } else {
        effect + 0.5
    };
    let half_width_at = |n: u64| -> Option<f64> {
        let n_f = n as f64;
        let (lo, hi) = if metric == MetricKind::Elo {
            let (lo, hi) = wilson::wilson_ci_from_proportion(p_hat, n_f, confidence).ok()?;
            (elo::elo_from_score(lo), elo::elo_from_score(hi))
        } else {
            match ci_method {
                CiMethod::Wilson => {
                    wilson::wilson_ci_from_proportion(p_hat, n_f, confidence).ok()?
                }
                CiMethod::Jeffreys => {
                    let successes = (p_hat * n_f).round().clamp(0.0, n_f) as u64;
                    jeffreys::jeffreys_ci(successes, n, confidence).ok()?
                }
                CiMethod::Exact => {
                    let successes = (p_hat * n_f).round().clamp(0.0, n_f) as u64;
                    exact::clopper_pearson_ci(successes, n, confidence).ok()?
                }
            }
        };
        Some((hi - lo) / 2.0)
    };

    // The naive O(1/sqrt(n)) estimate is a known *under*-estimate on the
    // metrics being searched here, so doubling it is a safe search bracket
    // (comfortable margin for Elo's nonlinearity shifting the true crossing
    // point relative to the linear-scale approximation).
    let hi_n = ((naive_required_n * 2.0) as u64).max(paired_count + 1);
    if half_width_at(hi_n).is_none_or(|w| w > target_half_width) {
        // The search bracket doesn't hold (a CI function failed, or behaved
        // non-monotonically at this n) - fall back to the always-available
        // approximation rather than returning a possibly-wrong result.
        return Some(naive_estimate);
    }
    let mut lo_n = paired_count;
    let mut hi_n = hi_n;
    while lo_n < hi_n {
        let mid = lo_n + (hi_n - lo_n) / 2;
        match half_width_at(mid) {
            Some(w) if w <= target_half_width => hi_n = mid,
            _ => lo_n = mid + 1,
        }
    }
    Some(lo_n.saturating_sub(paired_count))
}

pub fn decide(ci_low: f64, ci_high: f64, thresholds: &Thresholds) -> (Verdict, String) {
    if ci_low >= thresholds.pass_above {
        (
            Verdict::Pass,
            format!(
                "CI lower bound {ci_low:.4} meets the pass threshold {:.4}",
                thresholds.pass_above
            ),
        )
    } else if ci_high <= thresholds.fail_below {
        (
            Verdict::Fail,
            format!(
                "CI upper bound {ci_high:.4} is at or below the fail threshold {:.4}",
                thresholds.fail_below
            ),
        )
    } else {
        (
            Verdict::Inconclusive,
            format!(
                "CI [{ci_low:.4}, {ci_high:.4}] does not clearly cross the pass ({:.4}) or fail ({:.4}) threshold",
                thresholds.pass_above, thresholds.fail_below
            ),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aggregate_fail_dominates_pass() {
        let v = aggregate([Verdict::Pass, Verdict::Fail, Verdict::Pass]);
        assert_eq!(v, Verdict::Fail);
    }

    #[test]
    fn aggregate_inconclusive_holds_back_pass() {
        let v = aggregate([Verdict::Pass, Verdict::Inconclusive, Verdict::Pass]);
        assert_eq!(v, Verdict::Inconclusive);
    }

    #[test]
    fn aggregate_all_pass_is_pass() {
        let v = aggregate([Verdict::Pass, Verdict::Pass]);
        assert_eq!(v, Verdict::Pass);
    }

    #[test]
    fn passes_when_lower_bound_clears_threshold() {
        let t = Thresholds::symmetric(0.02).unwrap();
        let (v, _) = decide(0.03, 0.10, &t);
        assert_eq!(v, Verdict::Pass);
    }

    #[test]
    fn fails_when_upper_bound_below_negative_threshold() {
        let t = Thresholds::symmetric(0.02).unwrap();
        let (v, _) = decide(-0.10, -0.03, &t);
        assert_eq!(v, Verdict::Fail);
    }

    #[test]
    fn inconclusive_when_ci_straddles_threshold() {
        let t = Thresholds::symmetric(0.02).unwrap();
        let (v, _) = decide(-0.01, 0.05, &t);
        assert_eq!(v, Verdict::Inconclusive);
    }

    #[test]
    fn boundary_is_inclusive_for_pass() {
        let t = Thresholds::symmetric(0.02).unwrap();
        let (v, _) = decide(0.02, 0.05, &t);
        assert_eq!(v, Verdict::Pass);
    }

    #[test]
    fn rejects_fail_above_pass() {
        let result = Thresholds::new(0.0, 0.01);
        assert!(matches!(result, Err(VeridictError::InvalidThreshold(_))));
    }

    #[test]
    fn rejects_non_finite_thresholds() {
        assert!(matches!(
            Thresholds::new(f64::NAN, 0.0),
            Err(VeridictError::InvalidThreshold(_))
        ));
        assert!(matches!(
            Thresholds::new(0.0, f64::NEG_INFINITY),
            Err(VeridictError::InvalidThreshold(_))
        ));
    }

    // --- estimate_additional_trials ---
    // MetricKind::MeanDiff exercises the unchanged O(1/sqrt(n)) formula;
    // the dead-zone/already-decided/zero-paired-count guards below all
    // return `None` before ever touching metric-specific logic, so any
    // metric/ci_method placeholder is fine for those.

    #[test]
    fn already_decided_verdicts_need_no_more_trials() {
        let t = Thresholds::symmetric(0.02).unwrap();
        assert_eq!(
            estimate_additional_trials(
                MetricKind::MeanDiff,
                CiMethod::Wilson,
                Verdict::Pass,
                0.10,
                0.05,
                0.15,
                100,
                &t,
                0.95
            ),
            None
        );
        assert_eq!(
            estimate_additional_trials(
                MetricKind::MeanDiff,
                CiMethod::Wilson,
                Verdict::Fail,
                -0.10,
                -0.15,
                -0.05,
                100,
                &t,
                0.95
            ),
            None
        );
    }

    #[test]
    fn dead_zone_point_estimate_has_no_estimate_even_at_ten_million_trials() {
        // The concrete case that surfaced the "naive formula never
        // terminates" bug: effect sits inside the threshold band itself, so
        // no amount of CI shrinkage around that fixed point can cross
        // either boundary.
        let t = Thresholds::symmetric(0.02).unwrap();
        assert_eq!(
            estimate_additional_trials(
                MetricKind::MeanDiff,
                CiMethod::Wilson,
                Verdict::Inconclusive,
                0.0,
                -0.01,
                0.03,
                100,
                &t,
                0.95
            ),
            None
        );
        assert_eq!(
            estimate_additional_trials(
                MetricKind::MeanDiff,
                CiMethod::Wilson,
                Verdict::Inconclusive,
                0.0,
                -0.01,
                0.03,
                10_000_000,
                &t,
                0.95
            ),
            None
        );
    }

    #[test]
    fn effect_exactly_on_threshold_is_dead_zone() {
        let t = Thresholds::symmetric(0.02).unwrap();
        assert_eq!(
            estimate_additional_trials(
                MetricKind::MeanDiff,
                CiMethod::Wilson,
                Verdict::Inconclusive,
                0.02,
                -0.01,
                0.05,
                100,
                &t,
                0.95
            ),
            None
        );
        assert_eq!(
            estimate_additional_trials(
                MetricKind::MeanDiff,
                CiMethod::Wilson,
                Verdict::Inconclusive,
                -0.02,
                -0.05,
                0.01,
                100,
                &t,
                0.95
            ),
            None
        );
    }

    #[test]
    fn zero_paired_count_has_no_estimate() {
        let t = Thresholds::symmetric(0.02).unwrap();
        assert_eq!(
            estimate_additional_trials(
                MetricKind::MeanDiff,
                CiMethod::Wilson,
                Verdict::Inconclusive,
                0.05,
                -0.01,
                0.10,
                0,
                &t,
                0.95
            ),
            None
        );
    }

    #[test]
    fn mean_diff_outside_dead_zone_suggests_a_plausible_trial_count() {
        // effect=0.03 is past pass_above=0.02, but the CI [-0.01, 0.07] still
        // dips below it - resolvable by more data, unlike the dead-zone case.
        // Not asserting an exact number (the formula has a documented,
        // quantified bias): just that it lands in a sane ballpark.
        let t = Thresholds::symmetric(0.02).unwrap();
        let estimate = estimate_additional_trials(
            MetricKind::MeanDiff,
            CiMethod::Wilson,
            Verdict::Inconclusive,
            0.03,
            -0.01,
            0.07,
            50,
            &t,
            0.95,
        );
        assert!(matches!(estimate, Some(n) if (100..=2000).contains(&n)));
    }

    #[test]
    fn mean_diff_tiny_target_half_width_saturates_instead_of_panicking() {
        // effect is a hair past pass_above, so target_half_width is tiny -
        // this must saturate to u64::MAX, not overflow/panic.
        let t = Thresholds::symmetric(0.02).unwrap();
        let estimate = estimate_additional_trials(
            MetricKind::MeanDiff,
            CiMethod::Wilson,
            Verdict::Inconclusive,
            0.020000001,
            -0.5,
            0.5,
            100,
            &t,
            0.95,
        );
        assert_eq!(estimate, Some(u64::MAX));
    }

    #[test]
    fn winrate_wilson_search_matches_a_direct_wilson_recompute() {
        // effect=p_hat-0.5=0.05 (p_hat=0.55), target_half_width =
        // effect-pass_above = 0.05-0.02 = 0.03; the search's answer, applied
        // back through wilson_ci_from_proportion at the SAME p_hat, must
        // actually clear that width - not just "close to a formula", an
        // exact self-consistency check against the real CI function.
        let t = Thresholds::symmetric(0.02).unwrap();
        let target_half_width = 0.05 - 0.02;
        let (lo, hi) = wilson::wilson_ci(55, 100, 0.95).unwrap();
        let estimate = estimate_additional_trials(
            MetricKind::WinRate,
            CiMethod::Wilson,
            Verdict::Inconclusive,
            0.05,
            lo - 0.5,
            hi - 0.5,
            100,
            &t,
            0.95,
        )
        .unwrap();
        let required_n = 100 + estimate;
        let (rlo, rhi) = wilson::wilson_ci_from_proportion(0.55, required_n as f64, 0.95).unwrap();
        assert!((rhi - rlo) / 2.0 <= target_half_width + 1e-6);
        // One fewer trial should NOT already clear it (search found the
        // smallest n, not an overshoot).
        let (rlo2, rhi2) =
            wilson::wilson_ci_from_proportion(0.55, (required_n - 1) as f64, 0.95).unwrap();
        assert!((rhi2 - rlo2) / 2.0 > target_half_width - 1e-6);
    }

    #[test]
    fn elo_search_recovers_a_consistent_p_hat_and_clears_the_target() {
        let t = Thresholds::new(50.0, -50.0).unwrap();
        let score = 0.6;
        let (wlo, whi) = wilson::wilson_ci_from_proportion(score, 40.0, 0.95).unwrap();
        let effect = elo::elo_from_score(score);
        let target_half_width = effect - 50.0;
        let estimate = estimate_additional_trials(
            MetricKind::Elo,
            CiMethod::Wilson,
            Verdict::Inconclusive,
            effect,
            elo::elo_from_score(wlo),
            elo::elo_from_score(whi),
            40,
            &t,
            0.95,
        )
        .unwrap();
        let required_n = 40 + estimate;
        let (rlo, rhi) = wilson::wilson_ci_from_proportion(score, required_n as f64, 0.95).unwrap();
        let elo_half_width = (elo::elo_from_score(rhi) - elo::elo_from_score(rlo)) / 2.0;
        assert!(elo_half_width <= target_half_width + 1e-6);
    }

    #[test]
    fn jeffreys_search_differs_from_wilson_search_at_the_same_inputs() {
        // Wilson and Jeffreys have different widths at the same n, so
        // searching the wrong one would contradict the report's own shown
        // CI - this checks the two searches are NOT simply reusing the same
        // underlying computation, i.e. `ci_method` is actually load-bearing.
        let t = Thresholds::symmetric(0.02).unwrap();
        let (wlo, whi) = wilson::wilson_ci(55, 100, 0.95).unwrap();
        let (jlo, jhi) = jeffreys::jeffreys_ci(55, 100, 0.95).unwrap();
        let wilson_estimate = estimate_additional_trials(
            MetricKind::WinRate,
            CiMethod::Wilson,
            Verdict::Inconclusive,
            0.05,
            wlo - 0.5,
            whi - 0.5,
            100,
            &t,
            0.95,
        );
        let jeffreys_estimate = estimate_additional_trials(
            MetricKind::WinRate,
            CiMethod::Jeffreys,
            Verdict::Inconclusive,
            0.05,
            jlo - 0.5,
            jhi - 0.5,
            100,
            &t,
            0.95,
        );
        assert!(wilson_estimate.is_some() && jeffreys_estimate.is_some());
        assert_ne!(wilson_estimate, jeffreys_estimate);
    }

    #[test]
    fn search_based_estimate_is_closer_to_a_true_rerun_than_the_old_naive_formula() {
        // The documented ~18%-under-estimate case (n=100, p_hat past
        // threshold): the search-based estimate's suggested n must actually
        // clear the target width when re-run for real, unlike the old
        // formula (which the module doc already records as falling short
        // here).
        let t = Thresholds::symmetric(0.02).unwrap();
        let p_hat = 0.55;
        let target_half_width = (p_hat - 0.5) - 0.02;
        let (lo, hi) = wilson::wilson_ci_from_proportion(p_hat, 100.0, 0.95).unwrap();
        let estimate = estimate_additional_trials(
            MetricKind::WinRate,
            CiMethod::Wilson,
            Verdict::Inconclusive,
            p_hat - 0.5,
            lo - 0.5,
            hi - 0.5,
            100,
            &t,
            0.95,
        )
        .unwrap();
        let required_n = 100 + estimate;
        let (rlo, rhi) = wilson::wilson_ci_from_proportion(p_hat, required_n as f64, 0.95).unwrap();
        assert!(
            (rhi - rlo) / 2.0 <= target_half_width + 1e-6,
            "search-based estimate must actually clear the target width on a real re-run"
        );
    }
}
