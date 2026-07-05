//! Jeffreys interval for a binomial proportion: a Bayesian credible interval
//! using the non-informative Jeffreys prior Beta(0.5, 0.5). By Beta-Binomial
//! conjugacy, the posterior is Beta(x + 0.5, n - x + 0.5); the two-sided
//! interval is the alpha/2 and 1-alpha/2 quantiles of that posterior, with
//! the standard boundary correction (Brown, Cai & DasGupta 2001, "Interval
//! Estimation for a Binomial Proportion"): at x=0 the lower bound is forced
//! to 0 rather than the (small but nonzero) Beta quantile, since a lower
//! bound below 0 is meaningless for a proportion; symmetrically at x=n the
//! upper bound is forced to 1. Only the affected bound is overridden - the
//! other bound at that edge is still the real quantile, same convention
//! `exact.rs` already uses for its own x=0/x=n edges.
//!
//! Like Clopper-Pearson, this is derived directly from a true Beta-Binomial
//! model and does not generalize to a fractional proportion - it is rejected
//! for `elo`/`mean-diff` the same way (see `metrics::compute_many`'s
//! `IncompatibleCiMethod` check).
//!
//! Coverage-wise this sits between Wilson (narrower, can be slightly
//! anti-conservative for small n) and Clopper-Pearson (guaranteed-
//! conservative, usually widest) for interior p - but this ordering is not
//! universal: at x=0 or x=n it can be narrower than *both* (its prior
//! contributes real mass near the boundary that Wilson's normal
//! approximation and CP's worst-case guarantee don't get to use). See
//! `width_between_wilson_and_exact_for_interior_p` below for the spot-check,
//! scoped to interior p for exactly this reason.

use statrs::distribution::{Beta, ContinuousCDF};

use crate::error::VeridictError;

/// `successes` out of `n` trials, at the given two-sided confidence level.
pub fn jeffreys_ci(successes: u64, n: u64, confidence: f64) -> Result<(f64, f64), VeridictError> {
    if !confidence.is_finite() || confidence <= 0.0 || confidence >= 1.0 {
        return Err(VeridictError::InvalidConfidence(confidence));
    }
    debug_assert!(n > 0, "jeffreys_ci called with n == 0; caller must guard");
    debug_assert!(successes <= n, "jeffreys_ci called with successes > n");

    let alpha = 1.0 - confidence;
    let x = successes as f64;
    let n_f = n as f64;

    // x + 0.5 and n - x + 0.5 are both >= 0.5 for every x in 0..=n, so unlike
    // Clopper-Pearson's Beta(x, n-x+1)/Beta(x+1, n-x) (which can hit a
    // literal zero shape param at x=0 or x=n), this Beta is always
    // constructible - one object serves both bounds.
    let posterior = Beta::new(x + 0.5, n_f - x + 0.5)
        .expect("jeffreys_ci: x + 0.5 and n - x + 0.5 are always positive for x in 0..=n");

    let lower = if successes == 0 {
        0.0
    } else {
        posterior.inverse_cdf(alpha / 2.0)
    };
    let upper = if successes == n {
        1.0
    } else {
        posterior.inverse_cdf(1.0 - alpha / 2.0)
    };

    Ok((lower, upper))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_close(actual: f64, expected: f64, tol: f64) {
        assert!(
            (actual - expected).abs() < tol,
            "expected {expected}, got {actual}"
        );
    }

    // Verified against scipy.stats.beta.ppf (same tool exact.rs's own table
    // cites). Jeffreys uses half-integer Beta shapes (x+0.5, n-x+0.5), a
    // regime exact.rs's integer-shape fixtures never exercise; the (1,1) and
    // (0,1) rows are independently re-derived below via a closed-form CDF as
    // a scipy-independent cross-check.
    #[test]
    fn matches_verified_table() {
        let cases = [
            (5, 20, 0.95, (0.102398, 0.464195)),
            (0, 10, 0.95, (0.000000, 0.217196)),
            (50, 100, 0.95, (0.403174, 0.596826)),
            (100, 100, 0.95, (0.975255, 1.000000)),
            (1, 1, 0.95, (0.146746, 1.000000)),
            (0, 1, 0.95, (0.000000, 0.853254)),
            (7, 7, 0.99, (0.581439, 1.000000)),
        ];
        for (x, n, confidence, (expected_lo, expected_hi)) in cases {
            let (lo, hi) = jeffreys_ci(x, n, confidence).unwrap();
            assert_close(lo, expected_lo, 1e-6);
            assert_close(hi, expected_hi, 1e-6);
        }
    }

    #[test]
    fn rejects_invalid_confidence() {
        for c in [0.0, 1.0, -0.5, 1.5, f64::NAN, f64::INFINITY] {
            assert!(matches!(
                jeffreys_ci(5, 10, c),
                Err(VeridictError::InvalidConfidence(_))
            ));
        }
    }

    // Independent-of-scipy pinned checkpoint: Beta(1.5, 0.5) has a
    // closed-form CDF, derivable via x = sin^2(theta):
    //   F(x) = (2/pi) * [arcsin(sqrt(x)) - sqrt(x*(1-x))]
    // (F(0)=0, F(1)=1). x=n=1's lower bound is exactly this Beta's alpha/2
    // quantile (upper is clamped to 1 since x==n). This test computes that
    // quantile via the crate's own code, then verifies the closed form
    // reproduces alpha/2 at that point - independent of any external tool.
    #[test]
    fn x_eq_n_eq_one_matches_closed_form_beta_quantile() {
        let (lo, hi) = jeffreys_ci(1, 1, 0.95).unwrap();
        assert_close(hi, 1.0, 1e-9);

        let f = |x: f64| (2.0 / std::f64::consts::PI) * (x.sqrt().asin() - (x * (1.0 - x)).sqrt());
        assert_close(f(lo), 0.025, 1e-9);
    }

    // Beta(0.5, 1.5) is the mirror of Beta(1.5, 0.5) (CDF via X -> 1-X:
    // (2/pi)*[arcsin(sqrt(x)) + sqrt(x*(1-x))]) - x=0,n=1's upper bound
    // (lower is clamped to 0 since x==0) should satisfy this closed form.
    #[test]
    fn x_eq_zero_n_eq_one_matches_closed_form_beta_quantile() {
        let (lo, hi) = jeffreys_ci(0, 1, 0.95).unwrap();
        assert_close(lo, 0.0, 1e-9);

        let g = |x: f64| (2.0 / std::f64::consts::PI) * (x.sqrt().asin() + (x * (1.0 - x)).sqrt());
        assert_close(g(hi), 0.975, 1e-9);
    }

    // Structural property, needs no external reference: the posterior for x
    // successes is the mirror image of the posterior for n-x successes
    // (Beta(a,b) vs Beta(b,a) via X -> 1-X), so the two intervals must be
    // related by a 1-minus swap.
    #[test]
    fn symmetry_swaps_x_and_n_minus_x() {
        let (x, n, c) = (5, 20, 0.95);
        let (lo1, hi1) = jeffreys_ci(x, n, c).unwrap();
        let (lo2, hi2) = jeffreys_ci(n - x, n, c).unwrap();
        assert_close(lo1, 1.0 - hi2, 1e-9);
        assert_close(hi1, 1.0 - lo2, 1e-9);
    }

    #[test]
    fn all_wins_clamps_high_to_one() {
        let (lo, hi) = jeffreys_ci(9, 9, 0.95).unwrap();
        assert_close(hi, 1.0, 1e-9);
        assert!(lo > 0.0 && lo < 1.0);
    }

    #[test]
    fn all_losses_clamps_low_to_zero() {
        let (lo, hi) = jeffreys_ci(0, 9, 0.95).unwrap();
        assert_close(lo, 0.0, 1e-9);
        assert!(hi > 0.0 && hi < 1.0);
    }

    // Cross-check against the crate's own Wilson/Clopper-Pearson at one
    // interior (non-boundary) fixture - not a universal theorem. At the
    // boundary (x=0 or x=n) Jeffreys can be narrower than *both* (verified:
    // x=100,n=100,0.95 gives widths jeffreys=0.0247 < cp=0.0362 < wilson=
    // 0.0370), so this is deliberately scoped to an interior p where the
    // textbook ordering (Wilson tightest, Jeffreys in between, Clopper-
    // Pearson widest) actually holds.
    #[test]
    fn width_between_wilson_and_exact_for_interior_p() {
        let (x, n, c) = (50, 100, 0.95);
        let (w_lo, w_hi) = crate::stats::wilson::wilson_ci(x, n, c).unwrap();
        let (j_lo, j_hi) = jeffreys_ci(x, n, c).unwrap();
        let (cp_lo, cp_hi) = crate::stats::exact::clopper_pearson_ci(x, n, c).unwrap();
        assert!((w_hi - w_lo) <= (j_hi - j_lo));
        assert!((j_hi - j_lo) <= (cp_hi - cp_lo));
    }
}
