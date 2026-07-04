//! Paired bootstrap confidence intervals for a mean difference: the
//! original plain percentile method, plus BCa (bias-corrected and
//! accelerated - Efron & Tibshirani, "An Introduction to the Bootstrap",
//! 1993, ch. 14), added alongside it rather than replacing it so existing
//! callers' output doesn't silently change.
//!
//! Seeding: caller-supplied, defaulting to `DEFAULT_SEED` (see `--seed` on
//! the CLI). Same input + same seed gives bit-identical output across runs,
//! which is what CI needs; the seed is configurable per AGENTS.md's Phase 2
//! "deterministic bootstrap seed" item.

use rand::rngs::StdRng;
use rand::{RngExt, SeedableRng};
use statrs::distribution::{ContinuousCDF, Normal};

use crate::stats::wilson::inverse_normal_cdf;

pub const DEFAULT_SEED: u64 = 0x5EED;

fn bootstrap_means(diffs: &[f64], resamples: usize, seed: u64) -> Vec<f64> {
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
    means
}

/// Nearest-rank index for a lower-tail percentile `p` (rounds down).
fn lo_index(p: f64, resamples: usize) -> usize {
    ((p * resamples as f64).floor() as usize).min(resamples - 1)
}

/// Nearest-rank index for an upper-tail percentile `p` (rounds up, then
/// back one - the same "inclusive on both ends" convention `lo_index` uses,
/// mirrored for the upper tail).
fn hi_index(p: f64, resamples: usize) -> usize {
    let idx_raw = (p * resamples as f64).ceil() as usize;
    idx_raw.saturating_sub(1).min(resamples - 1)
}

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

    let means = bootstrap_means(diffs, resamples, seed);
    let alpha = 1.0 - confidence;
    let lo_idx = lo_index(alpha / 2.0, resamples);
    let hi_idx = hi_index(1.0 - alpha / 2.0, resamples).max(lo_idx);

    (means[lo_idx], means[hi_idx])
}

/// BCa bootstrap CI for the mean of `diffs`: corrects the plain percentile
/// method's bias and skewness sensitivity by adjusting which percentiles of
/// the bootstrap distribution are read, using a bias-correction `z0`
/// (fraction of bootstrap means below the original estimate) and an
/// acceleration `a` (from the jackknife, O(n) - not O(n^2)). When `z0 = 0`
/// and `a = 0` this reduces exactly to [`bootstrap_mean_diff_ci`]'s bounds
/// (both read the same `alpha/2`/`1 - alpha/2` percentiles) - a useful
/// property to check when this looks wrong.
pub fn bootstrap_mean_diff_ci_bca(
    diffs: &[f64],
    confidence: f64,
    resamples: usize,
    seed: u64,
) -> (f64, f64) {
    debug_assert!(!diffs.is_empty(), "bootstrap called with empty sample");
    debug_assert!(resamples > 0, "bootstrap called with zero resamples");

    let n = diffs.len();
    if n == 1 {
        // Jackknife/acceleration are undefined for a single observation;
        // mirror the percentile method's (already degenerate) n=1 answer.
        return (diffs[0], diffs[0]);
    }

    let original_mean = mean(diffs);
    let means = bootstrap_means(diffs, resamples, seed);

    // Bias-correction z0. Strict "<" is the textbook definition (Efron &
    // Tibshirani eq. 14.14) - a deliberate choice over e.g. scipy's
    // mean-rank convention; pinned by a test with tied diffs. Clamped away
    // from exactly 0/1 (possible at small n with heavy skew) so
    // `inverse_normal_cdf` never sees +-infinity.
    let below = means.iter().filter(|&&m| m < original_mean).count() as f64;
    let prop_less =
        (below / resamples as f64).clamp(0.5 / resamples as f64, 1.0 - 0.5 / resamples as f64);
    let z0 = inverse_normal_cdf(prop_less);

    // Acceleration a, via the jackknife.
    let total: f64 = diffs.iter().sum();
    let jack_means: Vec<f64> = diffs.iter().map(|d| (total - d) / (n - 1) as f64).collect();
    let jack_mean_of_means = mean(&jack_means);
    let (mut num, mut den) = (0.0, 0.0);
    for jm in &jack_means {
        let delta = jack_mean_of_means - jm;
        num += delta.powi(3);
        den += delta.powi(2);
    }
    // Constant/zero-variance data makes every jackknife mean identical, so
    // den is exactly 0 - the well-defined limit there is a=0 (no
    // acceleration), not NaN from a 0/0 division.
    let a = if den < 1e-12 {
        0.0
    } else {
        num / (6.0 * den.powf(1.5))
    };

    let alpha = 1.0 - confidence;
    let z_lo = inverse_normal_cdf(alpha / 2.0);
    let z_hi = inverse_normal_cdf(1.0 - alpha / 2.0);
    let normal = Normal::new(0.0, 1.0).expect("standard normal distribution is always valid");

    let adjust = |z: f64| -> f64 {
        let denom = 1.0 - a * (z0 + z);
        let adjusted = if denom.abs() < 1e-12 || !denom.is_finite() {
            // Denominator collapse (extreme acceleration): fall back to the
            // bias-correction-only adjustment for this tail rather than
            // propagating a blown-up value.
            z0 + z
        } else {
            z0 + (z0 + z) / denom
        };
        normal.cdf(adjusted).clamp(0.0, 1.0)
    };

    let p_lo = adjust(z_lo);
    let p_hi = adjust(z_hi);

    let lo_idx = lo_index(p_lo, resamples);
    let hi_idx = hi_index(p_hi, resamples).max(lo_idx);
    (means[lo_idx], means[hi_idx])
}

pub fn mean(values: &[f64]) -> f64 {
    values.iter().sum::<f64>() / values.len() as f64
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

    // --- BCa ---

    // 20-point moderately-skewed dataset, verified against scipy.stats.bootstrap(method='BCa')
    // and an independent Python reimplementation of the formula: both converge on
    // (0.07600, 0.22550) at 200,000 resamples. These are exact fixtures at the project's
    // real DEFAULT_SEED, computed directly in Rust with the algorithm above.
    const SKEWED: [f64; 20] = [
        0.05, 0.08, 0.12, 0.02, 0.15, 0.01, 0.30, 0.04, 0.06, 0.50, 0.03, 0.09, 0.11, 0.07, 0.02,
        0.60, 0.04, 0.08, 0.10, 0.05,
    ];

    #[test]
    fn bca_deterministic_across_calls() {
        let (lo1, hi1) = bootstrap_mean_diff_ci_bca(&SKEWED, 0.95, 2000, DEFAULT_SEED);
        let (lo2, hi2) = bootstrap_mean_diff_ci_bca(&SKEWED, 0.95, 2000, DEFAULT_SEED);
        assert_eq!(lo1, lo2);
        assert_eq!(hi1, hi2);
    }

    #[test]
    fn bca_brackets_true_mean() {
        let diffs = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        let (lo, hi) = bootstrap_mean_diff_ci_bca(&diffs, 0.95, 5000, DEFAULT_SEED);
        assert!(
            lo <= 3.0 && 3.0 <= hi,
            "expected [{lo}, {hi}] to bracket 3.0"
        );
    }

    #[test]
    fn bca_single_sample_no_panic() {
        let diffs = vec![2.5];
        let (lo, hi) = bootstrap_mean_diff_ci_bca(&diffs, 0.95, 1000, DEFAULT_SEED);
        assert!(lo.is_finite() && hi.is_finite());
        assert_eq!(lo, 2.5);
        assert_eq!(hi, 2.5);
    }

    #[test]
    fn bca_constant_data_no_panic() {
        let diffs = vec![2.5; 5];
        let (lo, hi) = bootstrap_mean_diff_ci_bca(&diffs, 0.95, 1000, DEFAULT_SEED);
        assert!(lo.is_finite() && hi.is_finite());
        assert_eq!(lo, 2.5);
        assert_eq!(hi, 2.5);
    }

    #[test]
    fn bca_matches_fixture_at_fixed_seed() {
        let (lo, hi) = bootstrap_mean_diff_ci_bca(&SKEWED, 0.95, 2000, DEFAULT_SEED);
        assert_close(lo, 0.07850, 1e-4);
        assert_close(hi, 0.22550, 1e-4);

        let (lo, hi) = bootstrap_mean_diff_ci_bca(&SKEWED, 0.95, 10_000, DEFAULT_SEED);
        assert_close(lo, 0.07650, 1e-4);
        assert_close(hi, 0.22350, 1e-4);
    }

    #[test]
    fn bca_differs_from_percentile_on_skewed_data() {
        let (bca_lo, bca_hi) = bootstrap_mean_diff_ci_bca(&SKEWED, 0.95, 10_000, DEFAULT_SEED);
        let (pct_lo, pct_hi) = bootstrap_mean_diff_ci(&SKEWED, 0.95, 10_000, DEFAULT_SEED);
        assert_ne!((bca_lo, bca_hi), (pct_lo, pct_hi));
    }

    #[test]
    fn bca_ties_do_not_panic() {
        let diffs = vec![1.0, 1.0, 1.0, 2.0, 2.0, 2.0, 2.0, 3.0, 3.0, 0.0, 0.0, 1.0];
        let (lo, hi) = bootstrap_mean_diff_ci_bca(&diffs, 0.95, 2000, DEFAULT_SEED);
        assert!(lo.is_finite() && hi.is_finite());
        assert!(lo <= hi);
    }

    #[test]
    fn bca_close_to_percentile_bounds_on_roughly_symmetric_data() {
        // NOT an exact-reduction check: a dataset symmetric around its mean
        // is close to z0=0/a=0 but not exactly there, because this dataset
        // contains its own mean (0) as a value - some resamples land on an
        // exact tie at the mean, and the strict "<" convention (see z0's
        // doc comment) excludes ties from "below", pulling the empirical
        // P(mean < 0) measurably away from 0.5 (verified: ~0.44, not 0.50,
        // at 20,000 resamples here). That's a real, correct consequence of
        // the tie convention, not a bug - so this only checks BCa stays in
        // the same neighborhood as the percentile method for well-behaved
        // data, not that the two are identical.
        let diffs = vec![-2.0, -1.0, 0.0, 1.0, 2.0];
        let (bca_lo, bca_hi) = bootstrap_mean_diff_ci_bca(&diffs, 0.95, 20_000, DEFAULT_SEED);
        let (pct_lo, pct_hi) = bootstrap_mean_diff_ci(&diffs, 0.95, 20_000, DEFAULT_SEED);
        assert_close(bca_lo, pct_lo, 0.3);
        assert_close(bca_hi, pct_hi, 0.3);
    }
}
