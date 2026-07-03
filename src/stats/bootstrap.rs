//! Paired percentile bootstrap confidence interval for a mean difference.
//!
//! Method: percentile bootstrap (not bias-corrected/accelerated — BCa is
//! materially more code and isn't needed for an MVP regression gate).
//!
//! Seeding: caller-supplied, defaulting to `DEFAULT_SEED` (see `--seed` on
//! the CLI). Same input + same seed gives bit-identical output across runs,
//! which is what CI needs; the seed is configurable per AGENTS.md's Phase 2
//! "deterministic bootstrap seed" item.

use rand::rngs::StdRng;
use rand::{RngExt, SeedableRng};

pub const DEFAULT_SEED: u64 = 0x5EED;

/// 95% (or whatever `confidence` requests) percentile bootstrap CI for the
/// mean of `diffs`. The point estimate `effect` comes from the original
/// sample, not from the resampled means. `diffs` must be non-empty.
pub fn bootstrap_mean_diff_ci(
    diffs: &[f64],
    confidence: f64,
    resamples: usize,
    seed: u64,
) -> (f64, f64) {
    debug_assert!(!diffs.is_empty(), "bootstrap called with empty sample");
    debug_assert!(resamples > 0, "bootstrap called with zero resamples");

    let n = diffs.len();
    let mut rng = StdRng::seed_from_u64(seed);
    let mut means: Vec<f64> = Vec::with_capacity(resamples);

    for _ in 0..resamples {
        let mut sum = 0.0;
        for _ in 0..n {
            let idx = rng.random_range(0..n);
            sum += diffs[idx];
        }
        means.push(sum / n as f64);
    }
    means.sort_by(f64::total_cmp);

    let alpha = 1.0 - confidence;
    let lo_idx = ((alpha / 2.0) * resamples as f64).floor() as usize;
    let lo_idx = lo_idx.min(resamples - 1);
    let hi_idx_raw = ((1.0 - alpha / 2.0) * resamples as f64).ceil() as usize;
    let hi_idx = hi_idx_raw.saturating_sub(1).min(resamples - 1).max(lo_idx);

    (means[lo_idx], means[hi_idx])
}

pub fn mean(values: &[f64]) -> f64 {
    values.iter().sum::<f64>() / values.len() as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_across_calls() {
        let diffs = vec![0.1, 0.2, -0.05, 0.3, 0.0, 0.15];
        let (lo1, hi1) = bootstrap_mean_diff_ci(&diffs, 0.95, 2000, DEFAULT_SEED);
        let (lo2, hi2) = bootstrap_mean_diff_ci(&diffs, 0.95, 2000, DEFAULT_SEED);
        assert_eq!(lo1, lo2);
        assert_eq!(hi1, hi2);
    }

    #[test]
    fn different_seed_can_change_output() {
        let diffs = vec![0.1, 0.2, -0.05, 0.3, 0.0, 0.15, 0.4, -0.2, 0.05, 0.25];
        let a = bootstrap_mean_diff_ci(&diffs, 0.95, 2000, DEFAULT_SEED);
        let b = bootstrap_mean_diff_ci(&diffs, 0.95, 2000, 42);
        assert_ne!(a, b);
    }

    #[test]
    fn brackets_true_mean() {
        let diffs = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        let (lo, hi) = bootstrap_mean_diff_ci(&diffs, 0.95, 5000, DEFAULT_SEED);
        assert!(
            lo <= 3.0 && 3.0 <= hi,
            "expected [{lo}, {hi}] to bracket 3.0"
        );
    }

    #[test]
    fn single_sample_no_panic() {
        let diffs = vec![2.5];
        let (lo, hi) = bootstrap_mean_diff_ci(&diffs, 0.95, 1000, DEFAULT_SEED);
        assert!(lo.is_finite() && hi.is_finite());
        assert_eq!(lo, 2.5);
        assert_eq!(hi, 2.5);
    }

    #[test]
    fn mean_of_known_values() {
        assert!((mean(&[0.1, 0.2, -0.05]) - 0.08333333333333333).abs() < 1e-9);
    }
}
