//! Empirical coverage for `paired-elo`'s pair-level percentile bootstrap.
//!
//! Both simulations have true expected score 0.5 (logistic Elo 0). The first is draw-heavy. The
//! second gives both color-reversed trials a shared latent opening strength, producing positive
//! within-pair correlation. Resampling complete pair scores should retain that dependence instead
//! of pretending the two trials are independent.
//!
//! Ignored in the default suite because each experiment contains a nested bootstrap. Run with:
//! `cargo test --test calibration -- --ignored paired_elo_coverage`.

use rand::rngs::StdRng;
use rand::{RngExt, SeedableRng};
use veridict::stats::bootstrap::bootstrap_mean_diff_ci;

const CONFIDENCE: f64 = 0.95;
const TRUE_SCORE: f64 = 0.5;
const PAIRS: usize = 120;
const RESAMPLES: usize = 400;
const SIMULATIONS: u64 = 800;
const SEED: u64 = 0x5EED_CAFE;

fn coverage(mut draw_pair_score: impl FnMut(&mut StdRng) -> f64) -> f64 {
    let mut rng = StdRng::seed_from_u64(SEED);
    let mut covered = 0u64;
    for simulation in 0..SIMULATIONS {
        let scores: Vec<f64> = (0..PAIRS).map(|_| draw_pair_score(&mut rng)).collect();
        let (lo, hi) = bootstrap_mean_diff_ci(
            &scores,
            CONFIDENCE,
            RESAMPLES,
            SEED.wrapping_add(simulation),
        );
        if lo <= TRUE_SCORE && TRUE_SCORE <= hi {
            covered += 1;
        }
    }
    covered as f64 / SIMULATIONS as f64
}

fn draw_draw_heavy_pair(rng: &mut StdRng) -> f64 {
    let trial_score = |rng: &mut StdRng| {
        let u: f64 = rng.random();
        if u < 0.15 {
            1.0
        } else if u < 0.85 {
            0.5
        } else {
            0.0
        }
    };
    (trial_score(rng) + trial_score(rng)) / 2.0
}

fn draw_positively_correlated_pair(rng: &mut StdRng) -> f64 {
    // Shared latent opening strength: p=0.8 or p=0.2 with equal probability. Each game's
    // marginal expected score is 0.5, while Cov(game1, game2)=Var(p)=0.09 > 0.
    let p = if rng.random::<bool>() { 0.8 } else { 0.2 };
    let a = if rng.random::<f64>() < p { 1.0 } else { 0.0 };
    let b = if rng.random::<f64>() < p { 1.0 } else { 0.0 };
    (a + b) / 2.0
}

#[test]
#[ignore = "Monte Carlo pair-bootstrap coverage calibration"]
fn paired_elo_coverage_is_near_nominal_on_draw_heavy_pairs() {
    let observed = coverage(draw_draw_heavy_pair);
    assert!(
        (0.90..=0.99).contains(&observed),
        "draw-heavy pair-bootstrap coverage {observed:.4} is not near 95%"
    );
}

#[test]
#[ignore = "Monte Carlo pair-bootstrap coverage calibration"]
fn paired_elo_coverage_is_near_nominal_with_positive_within_pair_correlation() {
    let observed = coverage(draw_positively_correlated_pair);
    assert!(
        (0.90..=0.99).contains(&observed),
        "correlated-pair bootstrap coverage {observed:.4} is not near 95%"
    );
}
