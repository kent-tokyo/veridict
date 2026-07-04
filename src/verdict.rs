//! Turns an effect size + confidence interval into a pass/fail/inconclusive
//! decision.
//!
//! The gate compares the CI, not the point estimate, against the thresholds:
//! a candidate only passes if it beats `pass_above` *even at the pessimistic
//! (lower) end* of the interval, and only fails if it's below `fail_below`
//! *even at the optimistic (upper) end*. Anything else is inconclusive. This
//! is deliberately conservative per AGENTS.md: "a false pass is worse than
//! an inconclusive result."

use crate::Verdict;
use crate::error::VeridictError;

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
/// This is an estimate with a known, quantified bias, not an exact
/// power-analysis result: verified within ~1.5% of an actual re-run for a
/// clean 4x sample-size jump at moderate n, but a real ~18% *under*-estimate
/// at n=100, because e.g. Wilson's CI also shrinks via an `O(z^2/n)`
/// recentering term this simple `1/sqrt(n)` model doesn't capture. Treat the
/// result as "roughly this many, plausibly more," not a guarantee.
pub fn estimate_additional_trials(
    verdict: Verdict,
    effect: f64,
    ci_low: f64,
    ci_high: f64,
    paired_count: u64,
    thresholds: &Thresholds,
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
    let required_total_n = paired_count as f64 * ratio * ratio;
    if !required_total_n.is_finite() || required_total_n >= u64::MAX as f64 {
        return Some(u64::MAX);
    }
    Some((required_total_n as u64).saturating_sub(paired_count))
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

    #[test]
    fn already_decided_verdicts_need_no_more_trials() {
        let t = Thresholds::symmetric(0.02).unwrap();
        assert_eq!(
            estimate_additional_trials(Verdict::Pass, 0.10, 0.05, 0.15, 100, &t),
            None
        );
        assert_eq!(
            estimate_additional_trials(Verdict::Fail, -0.10, -0.15, -0.05, 100, &t),
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
            estimate_additional_trials(Verdict::Inconclusive, 0.0, -0.01, 0.03, 100, &t),
            None
        );
        assert_eq!(
            estimate_additional_trials(Verdict::Inconclusive, 0.0, -0.01, 0.03, 10_000_000, &t),
            None
        );
    }

    #[test]
    fn effect_exactly_on_threshold_is_dead_zone() {
        let t = Thresholds::symmetric(0.02).unwrap();
        assert_eq!(
            estimate_additional_trials(Verdict::Inconclusive, 0.02, -0.01, 0.05, 100, &t),
            None
        );
        assert_eq!(
            estimate_additional_trials(Verdict::Inconclusive, -0.02, -0.05, 0.01, 100, &t),
            None
        );
    }

    #[test]
    fn zero_paired_count_has_no_estimate() {
        let t = Thresholds::symmetric(0.02).unwrap();
        assert_eq!(
            estimate_additional_trials(Verdict::Inconclusive, 0.05, -0.01, 0.10, 0, &t),
            None
        );
    }

    #[test]
    fn outside_dead_zone_suggests_a_plausible_trial_count() {
        // effect=0.03 is past pass_above=0.02, but the CI [-0.01, 0.07] still
        // dips below it - resolvable by more data, unlike the dead-zone case.
        // Not asserting an exact number (the formula has a documented,
        // quantified bias): just that it lands in a sane ballpark.
        let t = Thresholds::symmetric(0.02).unwrap();
        let estimate = estimate_additional_trials(Verdict::Inconclusive, 0.03, -0.01, 0.07, 50, &t);
        assert!(matches!(estimate, Some(n) if (100..=2000).contains(&n)));
    }

    #[test]
    fn tiny_target_half_width_saturates_instead_of_panicking() {
        // effect is a hair past pass_above, so target_half_width is tiny -
        // this must saturate to u64::MAX, not overflow/panic.
        let t = Thresholds::symmetric(0.02).unwrap();
        let estimate =
            estimate_additional_trials(Verdict::Inconclusive, 0.020000001, -0.5, 0.5, 100, &t);
        assert_eq!(estimate, Some(u64::MAX));
    }
}
