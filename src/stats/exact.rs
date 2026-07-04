//! Clopper-Pearson exact confidence interval for a binomial proportion.
//! Unlike `wilson_ci`'s normal approximation, this is derived directly from
//! the binomial distribution (via its relationship to the Beta
//! distribution), so it holds its nominal coverage guarantee exactly at any
//! sample size - at the cost of usually being wider than Wilson's interval.
//! Only defined for a true integer-count `Binomial(n, p)`; it does not
//! generalize to a fractional proportion (see `metrics::compute`'s
//! `IncompatibleCiMethod` check for `elo`, which has fractional
//! successes since draws count as half a win).

use statrs::distribution::{Beta, ContinuousCDF};

use crate::error::VeridictError;

/// `successes` out of `n` trials, at the given two-sided confidence level.
/// `lower = Beta(x, n-x+1).inverse_cdf(alpha/2)` (0 when x=0),
/// `upper = Beta(x+1, n-x).inverse_cdf(1-alpha/2)` (1 when x=n) - the
/// standard relation between the binomial and Beta distributions.
pub fn clopper_pearson_ci(
    successes: u64,
    n: u64,
    confidence: f64,
) -> Result<(f64, f64), VeridictError> {
    if !confidence.is_finite() || confidence <= 0.0 || confidence >= 1.0 {
        return Err(VeridictError::InvalidConfidence(confidence));
    }
    debug_assert!(
        n > 0,
        "clopper_pearson_ci called with n == 0; caller must guard"
    );
    debug_assert!(
        successes <= n,
        "clopper_pearson_ci called with successes > n"
    );

    let alpha = 1.0 - confidence;
    let x = successes as f64;
    let n_f = n as f64;

    let lower = if successes == 0 {
        0.0
    } else {
        // x in 1..=n here, so both shape params (x, n-x+1) are provably > 0.
        Beta::new(x, n_f - x + 1.0)
            .expect("clopper_pearson_ci: x in 1..=n guarantees positive beta shape params")
            .inverse_cdf(alpha / 2.0)
    };
    let upper = if successes == n {
        1.0
    } else {
        // x in 0..n here, so both shape params (x+1, n-x) are provably > 0.
        Beta::new(x + 1.0, n_f - x)
            .expect("clopper_pearson_ci: x in 0..n guarantees positive beta shape params")
            .inverse_cdf(1.0 - alpha / 2.0)
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

    // Verified against scipy.stats.beta.ppf and an independent F-distribution
    // formula; x=5,n=20 matches the textbook example in Zar's
    // "Biostatistical Analysis".
    #[test]
    fn matches_verified_table() {
        let cases = [
            (5, 20, 0.95, (0.086571, 0.491046)),
            (0, 10, 0.95, (0.000000, 0.308497)),
            (50, 100, 0.95, (0.398321, 0.601679)),
            (100, 100, 0.95, (0.963783, 1.000000)),
            (1, 1, 0.95, (0.025000, 1.000000)),
            (7, 7, 0.99, (0.469117, 1.000000)),
        ];
        for (x, n, confidence, (expected_lo, expected_hi)) in cases {
            let (lo, hi) = clopper_pearson_ci(x, n, confidence).unwrap();
            assert_close(lo, expected_lo, 1e-6);
            assert_close(hi, expected_hi, 1e-6);
        }
    }

    #[test]
    fn rejects_invalid_confidence() {
        for c in [0.0, 1.0, -0.5, 1.5, f64::NAN, f64::INFINITY] {
            assert!(matches!(
                clopper_pearson_ci(5, 10, c),
                Err(VeridictError::InvalidConfidence(_))
            ));
        }
    }

    #[test]
    fn wider_than_or_equal_to_wilson() {
        // Clopper-Pearson is the conservative/exact interval; it should never
        // be narrower than Wilson's approximation at the same (x, n, confidence).
        let (cp_lo, cp_hi) = clopper_pearson_ci(50, 100, 0.95).unwrap();
        let (w_lo, w_hi) = crate::stats::wilson::wilson_ci(50, 100, 0.95).unwrap();
        assert!(cp_lo <= w_lo);
        assert!(cp_hi >= w_hi);
    }
}
