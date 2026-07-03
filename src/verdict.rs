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
}
