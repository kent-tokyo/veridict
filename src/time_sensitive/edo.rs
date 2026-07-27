//! EDO (exponential-decay-optimal): a stationary (time-independent) approximation to the Bellman
//! policy, derived specifically for `RewardSchedule::ExponentialDecay`'s `R(t) = exp(-t/time_scale)`.
//! It has no meaning for `HardDeadline`/`Tabulated`, which don't have a `time_scale` to be
//! stationary with respect to (`TimeSensitiveConfig::validate` rejects `policy: Edo` paired with
//! any other reward kind).
//!
//! Never called "Bellman-optimal": it's a *stationary* approximation (one fixed action for every
//! trial, independent of `t`), which is what makes it cheap (closed-form, no grid), not what
//! makes it exact - the true time-sensitive-optimal action generally does depend on how close `t`
//! is to a wealth-dependent effective horizon, which EDO deliberately ignores. `method` in
//! `TimeSensitiveReport` reads `edo_stationary_approximation` for exactly this reason.
//!
//! ## The moment equation, and where the closed form below comes from
//!
//! EDO's stationary action `a*` is the maximizer of `M(a, eta) = p1*(a/p0)^eta +
//! (1-p1)*((1-a)/(1-p0))^eta` at the `eta* in (0,1)` solving `sup_a M(a, eta) = exp(1/time_scale)`.
//!
//! Setting `dM/da = 0` and solving (independent derivation - not transcribed from the paper) gives
//! a closed form for the maximizing action at any fixed `eta`, in terms of the odds `odds0 =
//! p0/(1-p0)`, `odds1 = p1/(1-p1)`:
//!
//! ```text
//! odds_a(eta) = odds0 * (odds1 / odds0) ^ (1 / (1 - eta))       for eta in [0, 1)
//! a*(eta)     = odds_a(eta) / (1 + odds_a(eta))
//! ```
//!
//! Two checks confirm this is the right branch (a naive read of the general-case formula could
//! also suggest an `eta`-exponent instead of `1/(1-eta)` - only the latter satisfies both):
//! - `eta -> 0` must recover GRO (`a* = p1`): at `eta=0`, `1/(1-eta) = 1`, so `odds_a(0) = odds0 *
//!   (odds1/odds0) = odds1`, giving `a*(0) = odds1/(1+odds1) = p1`. Checked in this file's tests.
//! - `eta in (0,1)` must give `a* > p1` ("EDO bets more aggressively than GRO," since a decaying
//!   reward makes patience itself costly): since `odds1/odds0 > 1` (as `p0 < p1`) and `1/(1-eta) >
//!   1` for `eta > 0`, `odds_a(eta) > odds1`, so `a*(eta) > p1`. Also checked in this file's
//!   tests.
//!
//! `sup_a M(a, eta)` is continuous and increases from `1` (at `eta=0`, for any `p0<p1`) to `p1/p0`
//! (as `eta -> 1^-`, where `a*(eta) -> 1`), so a root `eta* in (0,1)` of `sup_a M(a,eta) =
//! exp(1/time_scale)` exists iff `exp(1/time_scale) < p1/p0`, i.e. `time_scale >
//! 1/ln(p1/p0)` - the existence condition `solve_eta` checks before bisecting.

use super::e_variable::{ACTION_EPSILON, BernoulliHypotheses};
use super::grid::{self, WealthGrid};
use super::reward::RewardSchedule;
use crate::error::VeridictError;

const BISECTION_TOLERANCE: f64 = 1e-10;
const BISECTION_MAX_ITERATIONS: u32 = 200;
/// How close `eta` is allowed to approach `1` during bisection. Kept well short of `1.0` itself:
/// `1/(1-eta)` diverges there, and `a_star_ln_odds`'s own overflow guard (see its doc) already
/// saturates the action at that point, so searching any closer buys no additional resolution.
const ETA_MAX: f64 = 1.0 - 1e-12;

/// `ln(odds_a(eta))`, computed additively in log space rather than via `odds0 *
/// (odds1/odds0).powf(...)` directly: for `eta` close to `1`, `1/(1-eta)` is large and
/// `(odds1/odds0)^{large}` overflows `f64` to `+infinity` long before `eta` itself reaches `1`
/// (e.g. `odds1/odds0 = 1.22` already overflows around `eta ~ 0.9997`) - and `infinity /
/// (1+infinity)` is `NaN`, not `1.0`. Working in log space and saturating explicitly below keeps
/// this exact and NaN-free across the whole `eta` domain, not just the typical, comfortably-
/// inside-the-existence-region case.
fn ln_odds_a(eta: f64, ln_odds0: f64, ln_ratio: f64) -> f64 {
    ln_odds0 + ln_ratio / (1.0 - eta)
}

/// Past this log-odds magnitude, `odds_a / (1 + odds_a)` is no longer just close to `1` but
/// rounds to *exactly* `1.0` in `f64` (once `odds_a` exceeds roughly `2^52`, `1 + odds_a` rounds
/// down to `odds_a` itself) - well before `odds_a` itself would overflow `f64::exp` to
/// `+infinity` (which would instead compute `infinity/infinity = NaN`). Either failure mode
/// saturates the action at exactly `1.0`, so guard well ahead of both and saturate explicitly to
/// `1 - ACTION_EPSILON` instead, matching every other action-producing path in this module (see
/// its doc) that keeps `a` strictly inside `(0,1)`.
const ODDS_OVERFLOW_GUARD: f64 = 40.0;

fn a_star(eta: f64, ln_odds0: f64, ln_ratio: f64) -> f64 {
    let ln_oa = ln_odds_a(eta, ln_odds0, ln_ratio);
    if ln_oa > ODDS_OVERFLOW_GUARD {
        return 1.0 - ACTION_EPSILON;
    }
    let oa = ln_oa.exp();
    oa / (1.0 + oa)
}

fn m(a: f64, eta: f64, p0: f64, p1: f64) -> f64 {
    p1 * (a / p0).powf(eta) + (1.0 - p1) * ((1.0 - a) / (1.0 - p0)).powf(eta)
}

/// Solves `sup_a M(a, eta) = exp(1/time_scale)` for `eta* in (0, 1)` via bisection (the inner
/// `sup_a` is the closed form `a_star` above, not a nested search - see the module doc for why
/// this reduces to a single one-dimensional root find). Returns
/// [`VeridictError::EdoRootDoesNotExist`] both when the analytic existence condition fails
/// (`time_scale` too small) and when it holds only in a limit too close to `eta=1` to resolve
/// within `ETA_MAX`/`BISECTION_TOLERANCE` - both are honest "no usable EDO action here," not a
/// silently-degraded answer.
pub(crate) fn solve_eta(
    hypotheses: &BernoulliHypotheses,
    time_scale: f64,
) -> Result<f64, VeridictError> {
    let BernoulliHypotheses { p0, p1 } = *hypotheses;
    let ratio = p1 / p0;
    let target = (1.0 / time_scale).exp();
    let min_time_scale = 1.0 / ratio.ln();
    let existence_error = || VeridictError::EdoRootDoesNotExist {
        p0,
        p1,
        time_scale,
        ratio,
        min_time_scale,
    };
    if target >= ratio {
        return Err(existence_error());
    }

    let ln_odds0 = (p0 / (1.0 - p0)).ln();
    let ln_odds1 = (p1 / (1.0 - p1)).ln();
    let ln_ratio = ln_odds1 - ln_odds0;
    let f = |eta: f64| m(a_star(eta, ln_odds0, ln_ratio), eta, p0, p1) - target;

    debug_assert!(
        f(0.0) < 0.0,
        "target = exp(1/time_scale) > 1 always for time_scale > 0"
    );
    let mut lo = 0.0_f64;
    let mut hi = ETA_MAX;
    if f(hi) < 0.0 {
        // eta* exists mathematically (the analytic condition above holds) but lies too close to
        // 1 to resolve at ETA_MAX's precision - time_scale is barely above min_time_scale.
        return Err(existence_error());
    }
    for _ in 0..BISECTION_MAX_ITERATIONS {
        let mid = 0.5 * (lo + hi);
        let f_mid = f(mid);
        if f_mid.abs() < BISECTION_TOLERANCE {
            return Ok(mid);
        }
        if f_mid < 0.0 {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    Ok(0.5 * (lo + hi))
}

pub(crate) struct EdoPolicy {
    action: f64,
    eta: f64,
    planned_value: f64,
}

impl EdoPolicy {
    /// `reward` must be `RewardSchedule::ExponentialDecay` (see the module doc) -
    /// `TimeSensitiveConfig::validate` enforces this before an `EdoPolicy` is ever built, so this
    /// panics rather than returning a `Result` for the wrong reward kind: that would be a bug in
    /// this crate's own dispatch, not a caller input error to report gracefully.
    pub(crate) fn build(
        hypotheses: &BernoulliHypotheses,
        alpha: f64,
        reward: &RewardSchedule,
        wealth_grid_size: usize,
    ) -> Result<Self, VeridictError> {
        let time_scale = match reward {
            RewardSchedule::ExponentialDecay { time_scale, .. } => *time_scale,
            _ => panic!("EdoPolicy::build requires RewardSchedule::ExponentialDecay"),
        };
        let eta = solve_eta(hypotheses, time_scale)?;
        let ln_odds0 = (hypotheses.p0 / (1.0 - hypotheses.p0)).ln();
        let ln_odds1 = (hypotheses.p1 / (1.0 - hypotheses.p1)).ln();
        let action = a_star(eta, ln_odds0, ln_odds1 - ln_odds0);
        let grid = WealthGrid::new(alpha, wealth_grid_size);
        let planned_value =
            grid::evaluate_fixed_action(hypotheses.p0, hypotheses.p1, reward, &grid, action);
        Ok(Self {
            action,
            eta,
            planned_value,
        })
    }

    pub(crate) fn action(&self) -> f64 {
        self.action
    }

    pub(crate) fn eta(&self) -> f64 {
        self.eta
    }

    pub(crate) fn planned_value(&self) -> f64 {
        self.planned_value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hyp() -> BernoulliHypotheses {
        BernoulliHypotheses { p0: 0.5, p1: 0.55 }
    }

    #[test]
    fn eta_to_zero_recovers_gro() {
        let h = hyp();
        let ln_odds0 = (h.p0 / (1.0 - h.p0)).ln();
        let ln_odds1 = (h.p1 / (1.0 - h.p1)).ln();
        let a = a_star(0.0, ln_odds0, ln_odds1 - ln_odds0);
        assert!((a - h.p1).abs() < 1e-9);
    }

    #[test]
    fn eta_in_open_interval_gives_action_above_p1() {
        let h = hyp();
        let ln_odds0 = (h.p0 / (1.0 - h.p0)).ln();
        let ln_odds1 = (h.p1 / (1.0 - h.p1)).ln();
        for &eta in &[0.1, 0.3, 0.5, 0.7, 0.9] {
            let a = a_star(eta, ln_odds0, ln_odds1 - ln_odds0);
            assert!(a > h.p1, "eta={eta}, a*={a}, expected > p1={}", h.p1);
        }
    }

    #[test]
    fn a_star_never_overflows_to_nan_near_eta_one() {
        let h = hyp();
        let ln_odds0 = (h.p0 / (1.0 - h.p0)).ln();
        let ln_odds1 = (h.p1 / (1.0 - h.p1)).ln();
        for &eta in &[0.999, 0.9999, 0.999999, ETA_MAX] {
            let a = a_star(eta, ln_odds0, ln_odds1 - ln_odds0);
            assert!(a.is_finite() && a > 0.0 && a < 1.0, "eta={eta}, a*={a}");
        }
    }

    #[test]
    fn solve_eta_rejects_time_scale_below_existence_threshold() {
        let h = hyp();
        // min_time_scale = 1/ln(p1/p0); anything at or below it has no eta* in (0,1).
        let min_time_scale = 1.0 / (h.p1 / h.p0).ln();
        assert!(matches!(
            solve_eta(&h, min_time_scale * 0.5),
            Err(VeridictError::EdoRootDoesNotExist { .. })
        ));
    }

    #[test]
    fn solve_eta_finds_a_root_for_a_comfortable_time_scale() {
        let h = hyp();
        let eta = solve_eta(&h, 800.0).unwrap();
        assert!((0.0..1.0).contains(&eta));
        let ln_odds0 = (h.p0 / (1.0 - h.p0)).ln();
        let ln_odds1 = (h.p1 / (1.0 - h.p1)).ln();
        let target = (1.0_f64 / 800.0).exp();
        let value = m(a_star(eta, ln_odds0, ln_odds1 - ln_odds0), eta, h.p0, h.p1);
        assert!((value - target).abs() < 1e-6);
    }

    #[test]
    fn action_approaches_gro_as_time_scale_grows() {
        let h = hyp();
        let eta_small_urgency = solve_eta(&h, 1.0e9).unwrap();
        let ln_odds0 = (h.p0 / (1.0 - h.p0)).ln();
        let ln_odds1 = (h.p1 / (1.0 - h.p1)).ln();
        let a = a_star(eta_small_urgency, ln_odds0, ln_odds1 - ln_odds0);
        assert!(
            (a - h.p1).abs() < 1e-4,
            "a={a}, expected close to p1={}",
            h.p1
        );
    }
}
