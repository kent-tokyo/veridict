//! Wald's sequential probability ratio test (Wald 1945) on decisive
//! (non-draw) trials.
//!
//! ponytail: this is the classic two-outcome Wald SPRT, not the
//! trinomial/pentanomial variant chess-engine testers (Fishtest, cutechess)
//! use, which also models the draw rate to shrink faster on draw-heavy
//! testcases. That variant needs a second nuisance parameter and isn't
//! boring, verifiable math the way this is; upgrade to it if draw-heavy
//! inputs make this converge too slowly. Draws here carry no information
//! about which Elo hypothesis is true and are excluded from the LLR,
//! matching the "decisive games only" convention already used by
//! `winrate`/`sign-test`.

/// Expected score of a player rated `elo` points above its opponent, under
/// the standard logistic Elo model.
pub fn score_from_elo(elo: f64) -> f64 {
    1.0 / (1.0 + 10f64.powf(-elo / 400.0))
}

pub struct SprtBounds {
    pub lower: f64,
    pub upper: f64,
}

/// Wald's decision boundaries for false-positive rate `alpha` (accepting H1
/// when H0 is true) and false-negative rate `beta` (accepting H0 when H1 is
/// true).
pub fn bounds(alpha: f64, beta: f64) -> SprtBounds {
    SprtBounds {
        lower: (beta / (1.0 - alpha)).ln(),
        upper: ((1.0 - beta) / alpha).ln(),
    }
}

/// Log-likelihood-ratio contribution of one decisive trial: `p1`/`p0` are
/// the candidate's expected score under H1/H0.
pub fn llr_delta(candidate_won: bool, p0: f64, p1: f64) -> f64 {
    if candidate_won {
        (p1 / p0).ln()
    } else {
        ((1.0 - p1) / (1.0 - p0)).ln()
    }
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
    fn score_from_elo_matches_known_values() {
        assert_close(score_from_elo(0.0), 0.5, 1e-9);
        assert_close(score_from_elo(100.0), 0.64, 1e-2);
    }

    #[test]
    fn bounds_match_wald_formula() {
        let b = bounds(0.05, 0.05);
        assert_close(b.upper, (0.95_f64 / 0.05).ln(), 1e-9);
        assert_close(b.lower, (0.05_f64 / 0.95).ln(), 1e-9);
    }

    #[test]
    fn a_run_of_candidate_wins_drives_llr_upward() {
        let (p0, p1) = (score_from_elo(0.0), score_from_elo(10.0));
        let mut llr = 0.0;
        for _ in 0..1000 {
            llr += llr_delta(true, p0, p1);
        }
        assert!(llr > 0.0);
    }

    #[test]
    fn a_run_of_candidate_losses_drives_llr_downward() {
        let (p0, p1) = (score_from_elo(0.0), score_from_elo(10.0));
        let mut llr = 0.0;
        for _ in 0..1000 {
            llr += llr_delta(false, p0, p1);
        }
        assert!(llr < 0.0);
    }
}
