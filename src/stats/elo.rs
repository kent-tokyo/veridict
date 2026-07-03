//! Score-proportion <-> Elo conversions, standard logistic Elo model.
//!
//! A player rated `elo` points above its opponent has expected score
//! `p = 1 / (1 + 10^(-elo/400))`. Inverting gives `elo(p) = -400 *
//! log10(1/p - 1)`, which blows up to +-infinity at `p = 0`/`p = 1` (a
//! non-finite value `serde_json` can't round-trip meaningfully). Callers
//! pass scores through `elo_from_score`, which clamps internally so a
//! shutout sample still produces a finite, reportable number instead of an
//! error or a `null` field.

const P_EPSILON: f64 = 1e-4;

/// Elo difference implied by a score proportion `p` (fraction of available
/// points won, with a draw worth half a point). Clamped to `[P_EPSILON, 1 -
/// P_EPSILON]` before the log, so the largest reportable magnitude is about
/// +-1600 elo rather than +-infinity.
pub fn elo_from_score(p: f64) -> f64 {
    let clamped = p.clamp(P_EPSILON, 1.0 - P_EPSILON);
    -400.0 * (1.0 / clamped - 1.0).log10()
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
    fn even_score_is_zero_elo() {
        assert_close(elo_from_score(0.5), 0.0, 1e-9);
    }

    #[test]
    fn known_conversion_64_percent_is_about_100_elo() {
        assert_close(elo_from_score(0.64), 100.0, 1.0);
    }

    #[test]
    fn is_antisymmetric_around_half() {
        let a = elo_from_score(0.7);
        let b = elo_from_score(0.3);
        assert_close(a, -b, 1e-9);
    }

    #[test]
    fn shutout_scores_clamp_to_a_finite_magnitude() {
        let high = elo_from_score(1.0);
        let low = elo_from_score(0.0);
        assert!(high.is_finite() && high > 0.0);
        assert!(low.is_finite() && low < 0.0);
        assert_close(high, -low, 1e-9);
    }
}
