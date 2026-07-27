//! The Bernoulli e-variable and the GRO (growth-rate-optimal) baseline.
//!
//! Every policy in this module - GRO, `bellman`, `edo` - ultimately just picks an action
//! `a in (0,1)` each trial and multiplies wealth by `phi(a, outcome)`. The one fact this file
//! exists to isolate is that **any** `a` keeps that multiplication a valid nonnegative H0-
//! martingale step; nothing downstream can break anytime-validity, only optimality. See `phi`'s
//! own doc for why.

use crate::error::VeridictError;

/// The null (`p0`) and alternative (`p1`) success probabilities for a Bernoulli simple-vs-simple
/// time-sensitive test. Both are *decisive-observation-conditional*: a `Draw` outcome (see
/// `crate::Outcome`) is excluded from `p0`/`p1`'s definition entirely - it advances time (see
/// `TimeSensitiveTest::update`) but never enters the Bernoulli trial the wealth process bets on.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BernoulliHypotheses {
    pub p0: f64,
    pub p1: f64,
}

impl BernoulliHypotheses {
    pub fn validate(&self) -> Result<(), VeridictError> {
        for (name, v) in [("p0", self.p0), ("p1", self.p1)] {
            if !v.is_finite() || v <= 0.0 || v >= 1.0 {
                return Err(VeridictError::InvalidThreshold(format!(
                    "{name} must be finite and in (0, 1), got {v}"
                )));
            }
        }
        if self.p0 >= self.p1 {
            return Err(VeridictError::InvalidThreshold(format!(
                "p0 ({}) must be strictly less than p1 ({}): this module only implements the \
                 one-sided p0 < p1 simple-vs-simple case",
                self.p0, self.p1
            )));
        }
        Ok(())
    }
}

/// Keeps a candidate action strictly inside `(0,1)`. `phi(a, failure)` at `a=1` (or
/// `phi(a, success)` at `a=0`) is exactly `0`, whose log is `-infinity` - a legitimate limit of
/// the math (betting everything on one side), but not a usable action-grid endpoint, GRO output,
/// or EDO closed-form output: an action this extreme forfeits all wealth the instant the other
/// outcome occurs, which is never growth-optimal for `p0 < p1 < 1`.
pub(crate) const ACTION_EPSILON: f64 = 1e-9;

/// The Bernoulli e-variable: `phi(a, success) = a/p0`, `phi(a, failure) = (1-a)/(1-p0)`.
///
/// `p0*phi(a,success) + (1-p0)*phi(a,failure) = 1` for *every* `a` in `(0,1)` (verified
/// numerically in this module's tests, and exactly by algebra: `p0*(a/p0) + (1-p0)*((1-a)/(1-p0))
/// = a + (1-a) = 1`). This is the whole reason grid/closed-form approximation error in *choosing*
/// `a` (see `bellman.rs`, `edo.rs`) can never break anytime-validity: whichever `a` a policy
/// picks, `phi(a, .)` stays a valid nonnegative e-variable under H0 (`Ber(p0)`), so the
/// accumulated wealth product stays a nonnegative H0-martingale regardless of how good the
/// choice of `a` was. Approximation error only ever costs *optimality* (how fast wealth grows
/// under the true alternative), never validity - that separation is the load-bearing property
/// this whole module leans on to justify a numerically-approximated Bellman policy at all.
pub fn phi(a: f64, p0: f64, success: bool) -> f64 {
    if success {
        a / p0
    } else {
        (1.0 - a) / (1.0 - p0)
    }
}

/// `ln(phi(a, p0, success))` - all wealth arithmetic is done in log space (`log_wealth`, not
/// `wealth`) because a long run of trials multiplies dozens to thousands of `phi` factors
/// together; the product can underflow/overflow an `f64` long before the log-sum would, and the
/// wealth threshold `1/alpha` is naturally a log-space comparison (`log_wealth >=
/// ln(1/alpha)`) against a Bellman value function that is itself built additively over log-wealth
/// states.
pub fn log_phi(a: f64, p0: f64, success: bool) -> f64 {
    phi(a, p0, success).ln()
}

/// GRO (growth-rate-optimal): bet the alternative's true parameter every round, `a = p1`,
/// unconditionally on history or time. This is betting-equivalent to (the same multiplicative
/// update as) `stats::sprt`'s classical Wald log-likelihood-ratio walk - both bet the exact
/// likelihood ratio `dPer(p1)/dBer(p0)` every trial - but is implemented independently here
/// rather than calling into `stats::sprt`: this module's `update()` needs a live, resumable
/// per-trial state machine with a *time*-aware threshold/report shape (draws advance time,
/// rejection is time-stamped, `1/alpha` is compared in log-wealth space), none of which
/// `stats::sprt`'s batch/aggregate-count API expects to provide (see `sprt.rs`'s own doc for why
/// it computes its LLR from final aggregate counts rather than replaying a sequence). Sharing
/// code across that shape mismatch would cost more clarity than the few lines of duplication it
/// would save.
pub(crate) fn gro_action(hypotheses: &BernoulliHypotheses) -> f64 {
    hypotheses.p1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phi_is_a_valid_e_variable_across_a_fine_action_grid() {
        let p0 = 0.5;
        let n = 2000;
        for i in 0..=n {
            let a = ACTION_EPSILON + (1.0 - 2.0 * ACTION_EPSILON) * (i as f64 / n as f64);
            let expectation = p0 * phi(a, p0, true) + (1.0 - p0) * phi(a, p0, false);
            assert!(
                (expectation - 1.0).abs() < 1e-9,
                "a={a}, p0*phi(success)+((1-p0))*phi(failure)={expectation}, expected 1.0"
            );
        }
    }

    #[test]
    fn phi_is_a_valid_e_variable_for_varied_p0() {
        for &p0 in &[0.05, 0.3, 0.5, 0.7, 0.95] {
            for &a in &[0.01, 0.1, 0.3, 0.5, 0.7, 0.9, 0.99] {
                let expectation = p0 * phi(a, p0, true) + (1.0 - p0) * phi(a, p0, false);
                assert!(
                    (expectation - 1.0).abs() < 1e-9,
                    "p0={p0}, a={a}, expectation={expectation}"
                );
            }
        }
    }

    #[test]
    fn gro_action_is_the_alternative_parameter() {
        let h = BernoulliHypotheses { p0: 0.5, p1: 0.55 };
        assert_eq!(gro_action(&h), 0.55);
    }

    #[test]
    fn hypotheses_reject_p0_at_or_above_p1() {
        assert!(BernoulliHypotheses { p0: 0.5, p1: 0.5 }.validate().is_err());
        assert!(BernoulliHypotheses { p0: 0.6, p1: 0.5 }.validate().is_err());
    }

    #[test]
    fn hypotheses_reject_out_of_range() {
        assert!(BernoulliHypotheses { p0: 0.0, p1: 0.5 }.validate().is_err());
        assert!(BernoulliHypotheses { p0: 0.5, p1: 1.0 }.validate().is_err());
    }

    #[test]
    fn hypotheses_accept_a_valid_pair() {
        BernoulliHypotheses { p0: 0.5, p1: 0.55 }
            .validate()
            .unwrap();
    }
}
