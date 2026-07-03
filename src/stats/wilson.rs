//! Wilson score interval for a binomial proportion.
//!
//! Every win-rate verdict flows through `inverse_normal_cdf`, so a wrong
//! coefficient here silently corrupts every confidence interval built on
//! top of it. Test against known z-values before trusting anything else.

use crate::error::VeridictError;

/// Inverse standard normal CDF (probit function), via Peter Acklam's
/// rational approximation (<https://web.archive.org/web/20151030215612/http://home.online.no/~pjacklam/notes/invnorm/>).
/// Relative accuracy is about 1.15e-9 over (0, 1); no Halley refinement step
/// is applied since that accuracy is well within what a confidence interval
/// needs.
fn inverse_normal_cdf(p: f64) -> f64 {
    const A: [f64; 6] = [
        -3.969683028665376e+01,
        2.209460984245205e+02,
        -2.759285104469687e+02,
        1.383_577_518_672_69e2,
        -3.066479806614716e+01,
        2.506628277459239e+00,
    ];
    const B: [f64; 5] = [
        -5.447609879822406e+01,
        1.615858368580409e+02,
        -1.556989798598866e+02,
        6.680131188771972e+01,
        -1.328068155288572e+01,
    ];
    const C: [f64; 6] = [
        -7.784894002430293e-03,
        -3.223964580411365e-01,
        -2.400758277161838e+00,
        -2.549732539343734e+00,
        4.374664141464968e+00,
        2.938163982698783e+00,
    ];
    const D: [f64; 4] = [
        7.784695709041462e-03,
        3.224671290700398e-01,
        2.445134137142996e+00,
        3.754408661907416e+00,
    ];

    const P_LOW: f64 = 0.02425;
    const P_HIGH: f64 = 1.0 - P_LOW;

    if p < P_LOW {
        let q = (-2.0 * p.ln()).sqrt();
        (((((C[0] * q + C[1]) * q + C[2]) * q + C[3]) * q + C[4]) * q + C[5])
            / ((((D[0] * q + D[1]) * q + D[2]) * q + D[3]) * q + 1.0)
    } else if p <= P_HIGH {
        let q = p - 0.5;
        let r = q * q;
        (((((A[0] * r + A[1]) * r + A[2]) * r + A[3]) * r + A[4]) * r + A[5]) * q
            / (((((B[0] * r + B[1]) * r + B[2]) * r + B[3]) * r + B[4]) * r + 1.0)
    } else {
        let q = (-2.0 * (1.0 - p).ln()).sqrt();
        -(((((C[0] * q + C[1]) * q + C[2]) * q + C[3]) * q + C[4]) * q + C[5])
            / ((((D[0] * q + D[1]) * q + D[2]) * q + D[3]) * q + 1.0)
    }
}

/// Wilson score interval for `successes` out of `n` trials at the given
/// two-sided confidence level.
///
/// Preconditions (caller's responsibility, not re-validated here since this
/// is an internal helper always called after `n > 0` is already known):
/// `n > 0`. `confidence` is validated here since it comes straight from
/// user-supplied CLI input and `f64::from_str` happily parses `"nan"`.
pub fn wilson_ci(successes: u64, n: u64, confidence: f64) -> Result<(f64, f64), VeridictError> {
    if !confidence.is_finite() || confidence <= 0.0 || confidence >= 1.0 {
        return Err(VeridictError::InvalidConfidence(confidence));
    }
    debug_assert!(n > 0, "wilson_ci called with n == 0; caller must guard");

    let alpha = 1.0 - confidence;
    let z = inverse_normal_cdf(1.0 - alpha / 2.0);
    let n_f = n as f64;
    let p_hat = successes as f64 / n_f;

    let denom = 1.0 + z * z / n_f;
    let center = (p_hat + z * z / (2.0 * n_f)) / denom;
    let margin = (z / denom) * (p_hat * (1.0 - p_hat) / n_f + z * z / (4.0 * n_f * n_f)).sqrt();

    let low = (center - margin).clamp(0.0, 1.0);
    let high = (center + margin).clamp(0.0, 1.0);
    Ok((low, high))
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

    #[test]
    fn known_z_values() {
        assert_close(inverse_normal_cdf(1.0 - 0.10 / 2.0), 1.6448536, 1e-6);
        assert_close(inverse_normal_cdf(1.0 - 0.05 / 2.0), 1.9599640, 1e-6);
        assert_close(inverse_normal_cdf(1.0 - 0.01 / 2.0), 2.5758293, 1e-6);
    }

    #[test]
    fn wilson_50_of_100_at_95() {
        let (low, high) = wilson_ci(50, 100, 0.95).unwrap();
        assert_close(low, 0.4038315, 1e-6);
        assert_close(high, 0.5961685, 1e-6);
    }

    #[test]
    fn wilson_60_of_100_at_95() {
        let (low, high) = wilson_ci(60, 100, 0.95).unwrap();
        assert_close(low, 0.5020026, 1e-6);
        assert_close(high, 0.6905987, 1e-6);
    }

    #[test]
    fn wilson_all_wins_clamps_high_to_one() {
        let (low, high) = wilson_ci(9, 9, 0.95).unwrap();
        assert_close(high, 1.0, 1e-9);
        assert!(low < 1.0 && low > 0.0);
    }

    #[test]
    fn wilson_all_losses_clamps_low_to_zero() {
        let (low, high) = wilson_ci(0, 9, 0.95).unwrap();
        assert_close(low, 0.0, 1e-9);
        assert!(high < 1.0 && high > 0.0);
    }

    #[test]
    fn rejects_invalid_confidence() {
        for c in [0.0, 1.0, -0.5, 1.5, f64::NAN, f64::INFINITY] {
            assert!(matches!(
                wilson_ci(5, 10, c),
                Err(VeridictError::InvalidConfidence(_))
            ));
        }
    }
}
