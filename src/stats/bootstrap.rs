//! Paired bootstrap confidence intervals for a mean difference, or for an arbitrary quantile
//! difference (`quantile-diff`): the original plain percentile method, BCa (bias-corrected and
//! accelerated), and the basic/reflected bootstrap - all three covered in Efron & Tibshirani, "An
//! Introduction to the Bootstrap" (1993, ch. 14) - each added alongside the others rather than
//! replacing them, so existing callers' output doesn't silently change.
//!
//! Seeding: caller-supplied, defaulting to `DEFAULT_SEED` (see `--seed` on
//! the CLI). Same input + same seed gives bit-identical output across runs,
//! which is what CI needs; the seed is configurable per AGENTS.md's Phase 2
//! "deterministic bootstrap seed" item.

use rand::rngs::StdRng;
use rand::{RngExt, SeedableRng};
use statrs::distribution::{ContinuousCDF, Normal};

use crate::Outcome;
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

/// Number of consecutive retries `cluster_bootstrap_outcome_draws`/`iid_bootstrap_outcome_draws`
/// allow a single resample to redraw when it happens to contain zero decisive (non-draw)
/// outcomes - only reachable for `winrate`'s statistic on a draw-heavy, small-cluster-count input
/// (see that function's doc); large enough that hitting the cap at all would mean the real data
/// itself has essentially no decisive outcomes to resample from, a case `finish()` already rejects
/// before ever reaching the bootstrap.
const CLUSTER_RESAMPLE_RETRY_CAP: u32 = 1_000;

/// One resample's `(baseline_wins, candidate_wins, draws)`, drawing `clusters.len()` clusters
/// with replacement and pooling every outcome each drawn cluster contains. Retries (see
/// `CLUSTER_RESAMPLE_RETRY_CAP`) if the draw contains no decisive outcome at all - only
/// `statistic` callers that need at least one decisive outcome (`winrate`'s proportion) are
/// affected; `elo`'s statistic is well-defined even from an all-draws resample, so its caller
/// never actually retries in practice.
fn resample_outcome_pool(clusters: &[Vec<Outcome>], rng: &mut StdRng) -> (u64, u64, u64) {
    let k = clusters.len();
    let mut baseline_wins = 0u64;
    let mut candidate_wins = 0u64;
    let mut draws = 0u64;
    for _ in 0..k {
        let idx = rng.random_range(0..k);
        for outcome in &clusters[idx] {
            match outcome {
                Outcome::BaselineWin => baseline_wins += 1,
                Outcome::CandidateWin => candidate_wins += 1,
                Outcome::Draw => draws += 1,
            }
        }
    }
    (baseline_wins, candidate_wins, draws)
}

/// Cluster bootstrap over win/loss/draw outcome clusters (e.g. records sharing a `--cluster-by-id`
/// id, such as the same opening/testcase played several times): resamples whole clusters with
/// replacement instead of individual records, so trials that share a common source of correlation
/// don't each count as an independent unit of evidence - the correct nonparametric generalization
/// of `resample_edge_multinomial`'s per-edge resampling to a multi-record, possibly-correlated
/// cluster. Returns each resample's `statistic(baseline_wins, candidate_wins, draws)`, sorted -
/// not itself a CI; callers derive `ci_low`/`ci_high` via `lo_index`/`hi_index` and the point
/// estimate from the real (unresampled) data, the same "resampled distribution, real-data point
/// estimate" split every other bootstrap CI in this module already uses. `statistic` is a plain
/// `fn` pointer (not `impl Fn`), so the exact same value can drive both this and
/// `iid_bootstrap_outcome_draws` without a closure-capture/lifetime dance.
pub(crate) fn cluster_bootstrap_outcome_draws(
    clusters: &[Vec<Outcome>],
    resamples: usize,
    seed: u64,
    statistic: fn(u64, u64, u64) -> f64,
) -> Vec<f64> {
    let mut rng = StdRng::seed_from_u64(seed);
    let mut draws_out = Vec::with_capacity(resamples);
    for _ in 0..resamples {
        let mut attempt = resample_outcome_pool(clusters, &mut rng);
        let mut value = statistic(attempt.0, attempt.1, attempt.2);
        let mut tries = 0;
        while value.is_nan() && tries < CLUSTER_RESAMPLE_RETRY_CAP {
            attempt = resample_outcome_pool(clusters, &mut rng);
            value = statistic(attempt.0, attempt.1, attempt.2);
            tries += 1;
        }
        draws_out.push(value);
    }
    draws_out.sort_by(f64::total_cmp);
    draws_out
}

/// Same statistic, resampled at the individual-*record* level instead (cluster boundaries
/// ignored entirely) - i.e. the ordinary i.i.d. bootstrap every closed-form CI in this project
/// already implicitly assumes. Used only as `--cluster-by-id`'s `design_effect` baseline
/// (`Var(cluster) / Var(iid)`, both computed from the *same* bootstrap family so the two variance
/// estimates are directly comparable - not a closed-form binomial variance mixed with a bootstrap
/// one, which could disagree at small n for reasons that have nothing to do with clustering).
pub(crate) fn iid_bootstrap_outcome_draws(
    clusters: &[Vec<Outcome>],
    resamples: usize,
    seed: u64,
    statistic: fn(u64, u64, u64) -> f64,
) -> Vec<f64> {
    let records: Vec<Outcome> = clusters.iter().flatten().copied().collect();
    let singleton_clusters: Vec<Vec<Outcome>> = records.into_iter().map(|o| vec![o]).collect();
    cluster_bootstrap_outcome_draws(&singleton_clusters, resamples, seed, statistic)
}

/// `--cluster-by-id`'s full result: a percentile CI from the cluster bootstrap, plus
/// `design_effect`/`effective_sample_size` derived from comparing that same bootstrap's variance
/// against an i.i.d. bootstrap on the identical pooled data (see `iid_bootstrap_outcome_draws`'s
/// doc for why both must come from the same estimator family). `n` is the real total record count
/// (not a resampled one) - `effective_sample_size = n / design_effect`, the standard Kish (1965)
/// design-effect deflation of a naive sample size under clustering.
pub(crate) struct ClusterBootstrapResult {
    pub ci_low: f64,
    pub ci_high: f64,
    pub design_effect: f64,
    pub effective_sample_size: f64,
}

pub(crate) fn cluster_bootstrap_ci(
    clusters: &[Vec<Outcome>],
    n: u64,
    confidence: f64,
    resamples: usize,
    seed: u64,
    statistic: fn(u64, u64, u64) -> f64,
) -> ClusterBootstrapResult {
    let cluster_draws = cluster_bootstrap_outcome_draws(clusters, resamples, seed, statistic);
    let iid_draws = iid_bootstrap_outcome_draws(clusters, resamples, seed, statistic);

    let alpha = 1.0 - confidence;
    let ci_low = cluster_draws[lo_index(alpha / 2.0, resamples)];
    let ci_high = cluster_draws[hi_index(1.0 - alpha / 2.0, resamples)];

    let var_cluster = sample_variance(&cluster_draws);
    let var_iid = sample_variance(&iid_draws);
    // var_iid is 0 only in the degenerate case every i.i.d. resample landed on the exact same
    // statistic (e.g. a single-record input) - design_effect is undefined there, not infinite;
    // 1.0 (no inflation/deflation either way) is the honest default, not a fabricated extreme.
    let design_effect = if var_iid > 0.0 {
        var_cluster / var_iid
    } else {
        1.0
    };
    let effective_sample_size = if design_effect > 0.0 {
        n as f64 / design_effect
    } else {
        n as f64
    };

    ClusterBootstrapResult {
        ci_low,
        ci_high,
        design_effect,
        effective_sample_size,
    }
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

/// Bessel-corrected (`n-1` denominator) sample variance - the standard unbiased estimator, not
/// the population (`n`-denominator) one. Callers with fewer than 2 values must guard themselves;
/// this returns `NaN` at `n=1` (0.0/0.0) rather than panicking, since "undefined" is a more
/// honest result than a fabricated zero.
pub fn sample_variance(values: &[f64]) -> f64 {
    let m = mean(values);
    let sum_sq_dev: f64 = values.iter().map(|v| (v - m).powi(2)).sum();
    sum_sq_dev / (values.len() as f64 - 1.0)
}

pub fn sample_sd(values: &[f64]) -> f64 {
    sample_variance(values).sqrt()
}

/// Type-7 linear-interpolation quantile (R's default `type=7`, also NumPy's `percentile`
/// default) - the least-surprising convention among several published ones. `q` should be in
/// `[0, 1]`; CLI-facing callers restrict further to the open interval `(0, 1)` (see
/// `VeridictError::InvalidQuantile`), since the sample min/max (`q` = 0 or 1) has no
/// well-behaved bootstrap distribution to speak of.
pub fn quantile(values: &[f64], q: f64) -> f64 {
    debug_assert!(!values.is_empty(), "quantile called on an empty sample");
    let mut sorted: Vec<f64> = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    sorted_quantile(&sorted, q)
}

/// `quantile`'s core, given an already-sorted slice - shared with the BCa jackknife below, which
/// rebuilds a sorted subslice with one index skipped (still sorted, no re-sort needed) for each
/// leave-one-out replicate rather than resorting from scratch every time.
fn sorted_quantile(sorted: &[f64], q: f64) -> f64 {
    let n = sorted.len();
    if n == 1 {
        return sorted[0];
    }
    let h = q * (n - 1) as f64;
    let lo = h.floor() as usize;
    let hi = h.ceil() as usize;
    if lo == hi {
        return sorted[lo];
    }
    let frac = h - lo as f64;
    sorted[lo] + frac * (sorted[hi] - sorted[lo])
}

/// Resampled quantiles, sorted ascending - the quantile analogue of `bootstrap_means`.
fn bootstrap_quantiles(diffs: &[f64], q: f64, resamples: usize, seed: u64) -> Vec<f64> {
    let n = diffs.len();
    let mut rng = StdRng::seed_from_u64(seed);
    let mut quantiles: Vec<f64> = Vec::with_capacity(resamples);

    for _ in 0..resamples {
        let mut resample: Vec<f64> = Vec::with_capacity(n);
        for _ in 0..n {
            let idx = rng.random_range(0..n);
            resample.push(diffs[idx]);
        }
        resample.sort_by(f64::total_cmp);
        quantiles.push(sorted_quantile(&resample, q));
    }
    quantiles.sort_by(f64::total_cmp);
    quantiles
}

/// Percentile bootstrap CI for the `q`-th quantile of `diffs` - the quantile analogue of
/// [`bootstrap_mean_diff_ci`]. `diffs` must be non-empty.
pub fn bootstrap_quantile_diff_ci(
    diffs: &[f64],
    q: f64,
    confidence: f64,
    resamples: usize,
    seed: u64,
) -> (f64, f64) {
    debug_assert!(!diffs.is_empty(), "bootstrap called with empty sample");
    debug_assert!(resamples > 0, "bootstrap called with zero resamples");

    let quantiles = bootstrap_quantiles(diffs, q, resamples, seed);
    let alpha = 1.0 - confidence;
    let lo_idx = lo_index(alpha / 2.0, resamples);
    let hi_idx = hi_index(1.0 - alpha / 2.0, resamples).max(lo_idx);

    (quantiles[lo_idx], quantiles[hi_idx])
}

/// Basic (reflected) bootstrap CI for the `q`-th quantile of `diffs` - the quantile analogue of
/// [`bootstrap_mean_diff_ci_basic`]; see that function's doc for the method itself.
pub fn bootstrap_quantile_diff_ci_basic(
    diffs: &[f64],
    q: f64,
    confidence: f64,
    resamples: usize,
    seed: u64,
) -> (f64, f64) {
    debug_assert!(!diffs.is_empty(), "bootstrap called with empty sample");
    debug_assert!(resamples > 0, "bootstrap called with zero resamples");

    let effect = quantile(diffs, q);
    let (perc_lo, perc_hi) = bootstrap_quantile_diff_ci(diffs, q, confidence, resamples, seed);
    (2.0 * effect - perc_hi, 2.0 * effect - perc_lo)
}

/// BCa bootstrap CI for the `q`-th quantile of `diffs` - the quantile analogue of
/// [`bootstrap_mean_diff_ci_bca`], reusing the same generic `bias_correction_z0`/
/// `weighted_acceleration`/`bca_adjusted_percentiles` helpers.
///
/// **Not reachable from the CLI** (`MetricConfig::new` rejects `BootstrapMethod::Bca` for
/// `quantile-diff` with `VeridictError::IncompatibleBootstrapMethod`) - the sample quantile is a
/// non-smooth statistic (the empirical quantile function is a step function), so the jackknife
/// acceleration term this function computes has no solid asymptotic footing the way it does for
/// the mean. Implemented and exported so `tests/calibration/quantile_coverage.rs` can measure its
/// actual coverage directly; lifting the CLI gate is a deliberate follow-up once that evidence is
/// reviewed, not an automatic unlock.
///
/// The jackknife's leave-one-out quantile is recomputed by skipping one index out of the
/// already-sorted sample (no re-sort needed, since removing one element from a sorted sequence
/// leaves the rest sorted) - O(n) per replicate, O(n^2) total. An O(1)-per-replicate index-shift
/// trick is possible but not worth its boundary-case complexity: this jackknife runs once per CI
/// call, dwarfed by the `resamples`-iteration resampling loop, at the trial counts this project
/// targets.
pub fn bootstrap_quantile_diff_ci_bca(
    diffs: &[f64],
    q: f64,
    confidence: f64,
    resamples: usize,
    seed: u64,
) -> (f64, f64) {
    debug_assert!(!diffs.is_empty(), "bootstrap called with empty sample");
    debug_assert!(resamples > 0, "bootstrap called with zero resamples");

    let n = diffs.len();
    if n == 1 {
        // Jackknife/acceleration are undefined for a single observation; mirror the percentile
        // method's (already degenerate) n=1 answer.
        return (diffs[0], diffs[0]);
    }

    let original_estimate = quantile(diffs, q);
    let quantiles = bootstrap_quantiles(diffs, q, resamples, seed);

    let below = quantiles.iter().filter(|&&v| v < original_estimate).count() as f64;
    let z0 = bias_correction_z0(below, resamples);

    let mut sorted: Vec<f64> = diffs.to_vec();
    sorted.sort_by(f64::total_cmp);
    let jack_replicates: Vec<(f64, f64)> = (0..n)
        .map(|skip| {
            let leave_one_out: Vec<f64> = sorted
                .iter()
                .enumerate()
                .filter(|&(i, _)| i != skip)
                .map(|(_, &v)| v)
                .collect();
            (sorted_quantile(&leave_one_out, q), 1.0)
        })
        .collect();
    let a = weighted_acceleration(&jack_replicates);

    let (p_lo, p_hi) = bca_adjusted_percentiles(z0, a, confidence);
    let lo_idx = lo_index(p_lo, resamples);
    let hi_idx = hi_index(p_hi, resamples).max(lo_idx);
    (quantiles[lo_idx], quantiles[hi_idx])
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

    #[test]
    fn sample_variance_matches_a_hand_computed_example() {
        // [2, 4, 4, 4, 5, 5, 7, 9]: mean=5, Bessel-corrected variance=32/7 (a textbook example).
        let values = [2.0, 4.0, 4.0, 4.0, 5.0, 5.0, 7.0, 9.0];
        assert_close(sample_variance(&values), 32.0 / 7.0, 1e-9);
        assert_close(sample_sd(&values), (32.0f64 / 7.0).sqrt(), 1e-9);
    }

    #[test]
    fn sample_variance_of_identical_values_is_zero() {
        assert_eq!(sample_variance(&[3.0, 3.0, 3.0]), 0.0);
        assert_eq!(sample_sd(&[3.0, 3.0, 3.0]), 0.0);
    }

    // --- quantile (type-7 interpolation) ---

    #[test]
    fn quantile_matches_hand_computed_type7_examples() {
        // np.percentile([1..10], 90) == 9.1 under NumPy's (type-7) default.
        let values: Vec<f64> = (1..=10).map(|v| v as f64).collect();
        assert_close(quantile(&values, 0.9), 9.1, 1e-9);
        // Odd count: median lands exactly on the middle element.
        assert_close(quantile(&[1.0, 2.0, 3.0, 4.0, 5.0], 0.5), 3.0, 1e-9);
        // Even count: median interpolates halfway between the two middle elements.
        assert_close(quantile(&[1.0, 2.0, 3.0, 4.0], 0.5), 2.5, 1e-9);
    }

    #[test]
    fn quantile_ignores_input_order() {
        let sorted = [1.0, 2.0, 3.0, 4.0, 5.0];
        let shuffled = [3.0, 1.0, 5.0, 2.0, 4.0];
        assert_eq!(quantile(&sorted, 0.5), quantile(&shuffled, 0.5));
    }

    #[test]
    fn quantile_single_value_is_that_value() {
        assert_eq!(quantile(&[7.0], 0.1), 7.0);
        assert_eq!(quantile(&[7.0], 0.9), 7.0);
    }

    // --- quantile-diff bootstrap CIs ---

    #[test]
    fn quantile_ci_deterministic_across_calls() {
        let diffs = vec![0.1, 0.2, -0.05, 0.3, 0.0, 0.15];
        let (lo1, hi1) = bootstrap_quantile_diff_ci(&diffs, 0.5, 0.95, 2000, DEFAULT_SEED);
        let (lo2, hi2) = bootstrap_quantile_diff_ci(&diffs, 0.5, 0.95, 2000, DEFAULT_SEED);
        assert_eq!(lo1, lo2);
        assert_eq!(hi1, hi2);
    }

    #[test]
    fn quantile_ci_brackets_true_median() {
        let diffs = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        let (lo, hi) = bootstrap_quantile_diff_ci(&diffs, 0.5, 0.95, 5000, DEFAULT_SEED);
        assert!(
            lo <= 3.0 && 3.0 <= hi,
            "expected [{lo}, {hi}] to bracket the true median 3.0"
        );
    }

    #[test]
    fn quantile_ci_single_sample_no_panic() {
        let diffs = vec![2.5];
        let (lo, hi) = bootstrap_quantile_diff_ci(&diffs, 0.5, 0.95, 1000, DEFAULT_SEED);
        assert!(lo.is_finite() && hi.is_finite());
        assert_eq!(lo, 2.5);
        assert_eq!(hi, 2.5);
    }

    #[test]
    fn quantile_ci_ties_do_not_panic() {
        let diffs = vec![1.0, 1.0, 1.0, 2.0, 2.0, 2.0, 2.0, 3.0, 3.0, 0.0, 0.0, 1.0];
        let (lo, hi) = bootstrap_quantile_diff_ci(&diffs, 0.9, 0.95, 2000, DEFAULT_SEED);
        assert!(lo.is_finite() && hi.is_finite());
        assert!(lo <= hi);
    }

    #[test]
    fn quantile_ci_basic_reflects_percentile_exactly() {
        let effect = quantile(&SKEWED, 0.9);
        let (perc_lo, perc_hi) =
            bootstrap_quantile_diff_ci(&SKEWED, 0.9, 0.95, 10_000, DEFAULT_SEED);
        let (basic_lo, basic_hi) =
            bootstrap_quantile_diff_ci_basic(&SKEWED, 0.9, 0.95, 10_000, DEFAULT_SEED);
        assert_eq!(basic_lo, 2.0 * effect - perc_hi);
        assert_eq!(basic_hi, 2.0 * effect - perc_lo);
    }

    #[test]
    fn quantile_ci_basic_single_sample_no_panic() {
        let diffs = vec![2.5];
        let (lo, hi) = bootstrap_quantile_diff_ci_basic(&diffs, 0.5, 0.95, 1000, DEFAULT_SEED);
        assert!(lo.is_finite() && hi.is_finite());
        assert_eq!(lo, 2.5);
        assert_eq!(hi, 2.5);
    }

    #[test]
    fn quantile_ci_bca_deterministic_across_calls() {
        let (lo1, hi1) = bootstrap_quantile_diff_ci_bca(&SKEWED, 0.9, 0.95, 2000, DEFAULT_SEED);
        let (lo2, hi2) = bootstrap_quantile_diff_ci_bca(&SKEWED, 0.9, 0.95, 2000, DEFAULT_SEED);
        assert_eq!(lo1, lo2);
        assert_eq!(hi1, hi2);
    }

    #[test]
    fn quantile_ci_bca_single_sample_no_panic() {
        let diffs = vec![2.5];
        let (lo, hi) = bootstrap_quantile_diff_ci_bca(&diffs, 0.5, 0.95, 1000, DEFAULT_SEED);
        assert!(lo.is_finite() && hi.is_finite());
        assert_eq!(lo, 2.5);
        assert_eq!(hi, 2.5);
    }

    #[test]
    fn quantile_ci_bca_constant_data_no_panic() {
        let diffs = vec![2.5; 5];
        let (lo, hi) = bootstrap_quantile_diff_ci_bca(&diffs, 0.5, 0.95, 1000, DEFAULT_SEED);
        assert!(lo.is_finite() && hi.is_finite());
        assert_eq!(lo, 2.5);
        assert_eq!(hi, 2.5);
    }

    #[test]
    fn quantile_ci_bca_ties_do_not_panic() {
        let diffs = vec![1.0, 1.0, 1.0, 2.0, 2.0, 2.0, 2.0, 3.0, 3.0, 0.0, 0.0, 1.0];
        let (lo, hi) = bootstrap_quantile_diff_ci_bca(&diffs, 0.9, 0.95, 2000, DEFAULT_SEED);
        assert!(lo.is_finite() && hi.is_finite());
        assert!(lo <= hi);
    }

    #[test]
    fn quantile_ci_bca_differs_from_percentile_on_skewed_data() {
        let (bca_lo, bca_hi) =
            bootstrap_quantile_diff_ci_bca(&SKEWED, 0.9, 0.95, 10_000, DEFAULT_SEED);
        let (pct_lo, pct_hi) = bootstrap_quantile_diff_ci(&SKEWED, 0.9, 0.95, 10_000, DEFAULT_SEED);
        assert_ne!((bca_lo, bca_hi), (pct_lo, pct_hi));
    }
}
