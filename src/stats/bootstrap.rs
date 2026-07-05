//! Paired bootstrap confidence intervals for a mean difference: the
//! original plain percentile method, BCa (bias-corrected and accelerated),
//! and the basic/reflected bootstrap - all three covered in Efron &
//! Tibshirani, "An Introduction to the Bootstrap" (1993, ch. 14) - each
//! added alongside the others rather than replacing them, so existing
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
/// `pub(crate)`: also used by `bradley_terry::bootstrap_pairwise_elo_diff_cis`
/// to read percentiles off its own (non-mean) bootstrap draws, against
/// however many of them survived the connectivity filter - not always
/// `resamples` itself, hence the second parameter stays a plain length, not
/// tied to this module's `resamples` naming.
pub(crate) fn lo_index(p: f64, resamples: usize) -> usize {
    ((p * resamples as f64).floor() as usize).min(resamples - 1)
}

/// Nearest-rank index for an upper-tail percentile `p` (rounds up, then
/// back one - the same "inclusive on both ends" convention `lo_index` uses,
/// mirrored for the upper tail).
pub(crate) fn hi_index(p: f64, resamples: usize) -> usize {
    let idx_raw = (p * resamples as f64).ceil() as usize;
    idx_raw.saturating_sub(1).min(resamples - 1)
}

/// Resamples one edge's `(wins_lo, wins_hi, draws)` tally via a multinomial
/// draw with the same total `n` and the empirical outcome proportions - the
/// correct nonparametric bootstrap given that only aggregate counts, not
/// raw per-game order, are ever stored for an edge (see
/// `bradley_terry::bootstrap_pairwise_elo_diff_cis`). `n` is preserved
/// exactly, so an edge already observed can never disappear from a
/// resample; it can still become one-sided (losing a draw-created reverse
/// edge), which is the actual disconnection risk callers must handle.
pub(crate) fn resample_edge_multinomial(
    lo_wins: u64,
    hi_wins: u64,
    draws: u64,
    rng: &mut StdRng,
) -> (u64, u64, u64) {
    let n = lo_wins + hi_wins + draws;
    debug_assert!(
        n > 0,
        "resample_edge_multinomial called on an edge with zero games"
    );
    let n_f = n as f64;
    let p_lo = lo_wins as f64 / n_f;
    let p_lo_or_hi = p_lo + hi_wins as f64 / n_f;

    let (mut new_lo, mut new_hi, mut new_draws) = (0u64, 0u64, 0u64);
    for _ in 0..n {
        let u: f64 = rng.random();
        if u < p_lo {
            new_lo += 1;
        } else if u < p_lo_or_hi {
            new_hi += 1;
        } else {
            new_draws += 1;
        }
    }
    (new_lo, new_hi, new_draws)
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

/// Basic (a.k.a. "reflected" or "reverse-percentile") bootstrap CI for the
/// mean of `diffs`: reflects the percentile interval around the original
/// sample's point estimate (`effect`, not a resample mean) -
/// `(2*effect - perc_hi, 2*effect - perc_lo)`. Unlike BCa, this applies no
/// bias-correction or acceleration - it's the "obvious" fix for percentile's
/// known bias problem, but a naive one: on skewed data it reflects the
/// *same* skew that made the percentile interval biased, so it moves the
/// bounds in the opposite qualitative direction from BCa's correction (BCa
/// adjusts which percentiles are read based on where the bootstrap
/// distribution's bias/skew actually point; basic just mirrors the raw
/// percentile bounds around `effect` regardless of *why* they're off
/// center). This is why BCa is generally preferred for skewed data despite
/// basic bootstrap being the simpler, older method.
pub fn bootstrap_mean_diff_ci_basic(
    diffs: &[f64],
    confidence: f64,
    resamples: usize,
    seed: u64,
) -> (f64, f64) {
    debug_assert!(!diffs.is_empty(), "bootstrap called with empty sample");
    debug_assert!(resamples > 0, "bootstrap called with zero resamples");

    let effect = mean(diffs);
    let (perc_lo, perc_hi) = bootstrap_mean_diff_ci(diffs, confidence, resamples, seed);
    (2.0 * effect - perc_hi, 2.0 * effect - perc_lo)
}

/// Bias-correction `z0` from the count of bootstrap draws strictly below the
/// original point estimate, out of `resamples` total. Strict "<" is the
/// textbook definition (Efron & Tibshirani eq. 14.14) - a deliberate choice
/// over e.g. scipy's mean-rank convention; pinned by a test with tied diffs.
/// Clamped away from exactly 0/1 (possible at small n with heavy skew) so
/// `inverse_normal_cdf` never sees +-infinity. `pub(crate)`: shared with
/// `bradley_terry::bootstrap_pairwise_elo_diff_cis`'s per-pair BCa, so both
/// paths compute this identically rather than risking silent drift between
/// two copies of the same formula.
pub(crate) fn bias_correction_z0(below_count: f64, resamples: usize) -> f64 {
    let prop_less = (below_count / resamples as f64)
        .clamp(0.5 / resamples as f64, 1.0 - 0.5 / resamples as f64);
    inverse_normal_cdf(prop_less)
}

/// Skewness-based BCa acceleration from weighted jackknife replicates
/// (`value`, `weight`) pairs. The textbook unweighted formula (every
/// replicate equally likely) is the special case where every weight is
/// `1.0` - kept as one formula rather than two so a caller with genuinely
/// exchangeable replicates (e.g. dropping any one of an edge's identical
/// a-win games gives the same replicate value) can fold them into a single
/// weighted entry instead of repeating it `weight` times. `pub(crate)`:
/// shared with `bradley_terry`'s graph-jackknife BCa for the same reason as
/// [`bias_correction_z0`].
pub(crate) fn weighted_acceleration(replicates: &[(f64, f64)]) -> f64 {
    let total_weight: f64 = replicates.iter().map(|&(_, w)| w).sum();
    if total_weight <= 0.0 {
        // No valid replicates (e.g. every one excluded by the caller for
        // disconnecting the pair being estimated) - the well-defined limit
        // is no acceleration, same as the constant/zero-variance case below.
        return 0.0;
    }
    let weighted_mean: f64 = replicates.iter().map(|&(v, w)| v * w).sum::<f64>() / total_weight;
    let (mut num, mut den) = (0.0, 0.0);
    for &(v, w) in replicates {
        let delta = weighted_mean - v;
        num += w * delta.powi(3);
        den += w * delta.powi(2);
    }
    // Constant/zero-variance replicates make every delta 0, so den is
    // exactly 0 - the well-defined limit there is a=0 (no acceleration),
    // not NaN from a 0/0 division.
    if den < 1e-12 {
        0.0
    } else {
        num / (6.0 * den.powf(1.5))
    }
}

/// BCa-adjusted lower/upper tail probabilities to read off a sorted
/// bootstrap distribution, given a bias-correction `z0` and acceleration
/// `a`. Reduces exactly to the plain percentile method's `alpha/2`/`1 -
/// alpha/2` when `z0 = 0` and `a = 0` - a useful property to check when this
/// looks wrong. `pub(crate)`: shared with `bradley_terry`'s graph BCa for
/// the same reason as [`bias_correction_z0`].
pub(crate) fn bca_adjusted_percentiles(z0: f64, a: f64, confidence: f64) -> (f64, f64) {
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

    (adjust(z_lo), adjust(z_hi))
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

    let below = means.iter().filter(|&&m| m < original_mean).count() as f64;
    let z0 = bias_correction_z0(below, resamples);

    // Acceleration a, via the jackknife - every replicate is equally likely
    // (weight 1.0), the textbook unweighted case of `weighted_acceleration`.
    let total: f64 = diffs.iter().sum();
    let jack_replicates: Vec<(f64, f64)> = diffs
        .iter()
        .map(|d| ((total - d) / (n - 1) as f64, 1.0))
        .collect();
    let a = weighted_acceleration(&jack_replicates);

    let (p_lo, p_hi) = bca_adjusted_percentiles(z0, a, confidence);
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

    // --- resample_edge_multinomial ---

    #[test]
    fn multinomial_resample_preserves_the_total() {
        let mut rng = StdRng::seed_from_u64(DEFAULT_SEED);
        for _ in 0..1000 {
            let (lo, hi, draws) = resample_edge_multinomial(3, 5, 2, &mut rng);
            assert_eq!(lo + hi + draws, 10);
        }
    }

    #[test]
    fn multinomial_resample_is_deterministic_for_a_given_rng_state() {
        let mut rng_a = StdRng::seed_from_u64(DEFAULT_SEED);
        let mut rng_b = StdRng::seed_from_u64(DEFAULT_SEED);
        for _ in 0..50 {
            assert_eq!(
                resample_edge_multinomial(4, 1, 0, &mut rng_a),
                resample_edge_multinomial(4, 1, 0, &mut rng_b)
            );
        }
    }

    // Law-of-large-numbers sanity check, not an independently-verified exact
    // value (a multinomial resample's own RNG draws aren't something an
    // external tool can reproduce bit-for-bit): over many resamples of a
    // fixed (lo, hi, draws) tally, the empirical mean proportion of each
    // outcome should land close to the input proportions.
    #[test]
    fn multinomial_resample_proportions_converge_to_the_input_proportions() {
        let mut rng = StdRng::seed_from_u64(DEFAULT_SEED);
        let (lo_wins, hi_wins, draws, n) = (20u64, 50u64, 30u64, 100u64);
        let resamples = 20_000;
        let (mut sum_lo, mut sum_hi, mut sum_draws) = (0u64, 0u64, 0u64);
        for _ in 0..resamples {
            let (lo, hi, d) = resample_edge_multinomial(lo_wins, hi_wins, draws, &mut rng);
            assert_eq!(lo + hi + d, n);
            sum_lo += lo;
            sum_hi += hi;
            sum_draws += d;
        }
        let mean_lo_frac = sum_lo as f64 / (resamples as f64 * n as f64);
        let mean_hi_frac = sum_hi as f64 / (resamples as f64 * n as f64);
        let mean_draws_frac = sum_draws as f64 / (resamples as f64 * n as f64);
        assert_close(mean_lo_frac, lo_wins as f64 / n as f64, 0.01);
        assert_close(mean_hi_frac, hi_wins as f64 / n as f64, 0.01);
        assert_close(mean_draws_frac, draws as f64 / n as f64, 0.01);
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

    // --- Basic (reflected/reverse-percentile) bootstrap ---

    #[test]
    fn basic_deterministic_across_calls() {
        let diffs = vec![0.1, 0.2, -0.05, 0.3, 0.0, 0.15];
        let (lo1, hi1) = bootstrap_mean_diff_ci_basic(&diffs, 0.95, 2000, DEFAULT_SEED);
        let (lo2, hi2) = bootstrap_mean_diff_ci_basic(&diffs, 0.95, 2000, DEFAULT_SEED);
        assert_eq!(lo1, lo2);
        assert_eq!(hi1, hi2);
    }

    #[test]
    fn basic_brackets_true_mean() {
        let diffs = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        let (lo, hi) = bootstrap_mean_diff_ci_basic(&diffs, 0.95, 5000, DEFAULT_SEED);
        assert!(
            lo <= 3.0 && 3.0 <= hi,
            "expected [{lo}, {hi}] to bracket 3.0"
        );
    }

    #[test]
    fn basic_single_sample_no_panic() {
        let diffs = vec![2.5];
        let (lo, hi) = bootstrap_mean_diff_ci_basic(&diffs, 0.95, 1000, DEFAULT_SEED);
        assert!(lo.is_finite() && hi.is_finite());
        assert_eq!(lo, 2.5);
        assert_eq!(hi, 2.5);
    }

    // Exact structural match #1 (degenerate case): with one observation, the
    // bootstrap "distribution" is a single point, trivially symmetric about
    // itself, so basic == percentile exactly - not approximately.
    #[test]
    fn basic_equals_percentile_exactly_on_single_sample() {
        let diffs = vec![2.5];
        let basic = bootstrap_mean_diff_ci_basic(&diffs, 0.95, 1000, DEFAULT_SEED);
        let percentile = bootstrap_mean_diff_ci(&diffs, 0.95, 1000, DEFAULT_SEED);
        assert_eq!(basic, percentile);
    }

    // Exact structural match #2 (degenerate case): constant data collapses
    // every resample mean to the same constant regardless of which indices
    // the RNG drew, so the bootstrap distribution is exactly (not just
    // approximately) a point mass - basic == percentile exactly here too.
    #[test]
    fn basic_equals_percentile_exactly_on_constant_data() {
        let diffs = vec![2.5; 5];
        let basic = bootstrap_mean_diff_ci_basic(&diffs, 0.95, 1000, DEFAULT_SEED);
        let percentile = bootstrap_mean_diff_ci(&diffs, 0.95, 1000, DEFAULT_SEED);
        assert_eq!(basic, percentile);
    }

    // Correction to a premise worth stating explicitly: for non-degenerate
    // (varied-value) symmetric data, basic does NOT exactly equal
    // percentile, even though the reflection algebra
    // (2*effect - perc_hi = perc_lo when the bootstrap distribution is
    // perfectly symmetric) is correct in the continuous-distribution limit.
    // With a finite resample count and a fixed PRNG stream, the empirical
    // bootstrap-means distribution is only approximately symmetric - the
    // same class of caveat this file already documents for BCa's own
    // "close to but not equal to percentile" test above. So this only
    // checks the two stay close, not that they match.
    #[test]
    fn basic_close_to_percentile_on_roughly_symmetric_data() {
        let diffs = vec![-2.0, -1.0, 0.0, 1.0, 2.0];
        let (basic_lo, basic_hi) = bootstrap_mean_diff_ci_basic(&diffs, 0.95, 20_000, DEFAULT_SEED);
        let (pct_lo, pct_hi) = bootstrap_mean_diff_ci(&diffs, 0.95, 20_000, DEFAULT_SEED);
        assert_close(basic_lo, pct_lo, 0.3);
        assert_close(basic_hi, pct_hi, 0.3);
    }

    #[test]
    fn basic_differs_from_percentile_on_skewed_data() {
        let (basic_lo, basic_hi) =
            bootstrap_mean_diff_ci_basic(&SKEWED, 0.95, 10_000, DEFAULT_SEED);
        let (pct_lo, pct_hi) = bootstrap_mean_diff_ci(&SKEWED, 0.95, 10_000, DEFAULT_SEED);
        assert_ne!((basic_lo, basic_hi), (pct_lo, pct_hi));
    }

    #[test]
    fn basic_ties_do_not_panic() {
        let diffs = vec![1.0, 1.0, 1.0, 2.0, 2.0, 2.0, 2.0, 3.0, 3.0, 0.0, 0.0, 1.0];
        let (lo, hi) = bootstrap_mean_diff_ci_basic(&diffs, 0.95, 2000, DEFAULT_SEED);
        assert!(lo.is_finite() && hi.is_finite());
        assert!(lo <= hi);
    }

    // Core correctness guard: not tautological despite calling the same
    // formula the implementation uses - it pins the exact algebraic
    // relation (real bug class it catches: swapped lo/hi, wrong sign, or
    // using a resample mean instead of the original `effect`). Since both
    // this test and the implementation call `bootstrap_mean_diff_ci` with
    // the identical seed, the percentile bounds are bit-identical, so the
    // identity holds to exactly 0.0 - `assert_eq!`, not a tolerance.
    #[test]
    fn basic_reflects_percentile_exactly() {
        let effect = mean(&SKEWED);
        let (perc_lo, perc_hi) = bootstrap_mean_diff_ci(&SKEWED, 0.95, 10_000, DEFAULT_SEED);
        let (basic_lo, basic_hi) =
            bootstrap_mean_diff_ci_basic(&SKEWED, 0.95, 10_000, DEFAULT_SEED);
        assert_eq!(basic_lo, 2.0 * effect - perc_hi);
        assert_eq!(basic_hi, 2.0 * effect - perc_lo);
    }
}
