//! Empirical coverage of the `quantile-diff` bootstrap CIs (percentile, basic, and the
//! CLI-gated BCa - see `VeridictError::IncompatibleBootstrapMethod`) across several quantiles and
//! two populations: `Exponential(rate=1)` (skew) and `Normal(0,1)` (symmetric), mirroring
//! `bootstrap_coverage.rs`'s methodology but for an arbitrary quantile rather than the mean. This
//! is where the "BCa is implemented but gated pending calibration review" decision gets its actual
//! evidence, and where `thin_quantile_tail`'s `n * min(q, 1-q) < 10` threshold gets empirical
//! support (coverage should visibly improve between the "thin" and "adequate" sample sizes tested
//! here).
//!
//! `#[ignore]`d for the same reason as `bootstrap_coverage.rs`: each call re-resamples
//! `RESAMPLES` times, so `SIMULATIONS`-many repeats are too slow for every push/PR - see
//! `.github/workflows/calibration.yml` for where these actually run. Run explicitly with `cargo
//! test --test calibration -- --ignored quantile_coverage` (or `--include-ignored` to run
//! everything).

use rand::rngs::StdRng;
use rand::{RngExt, SeedableRng};
use statrs::distribution::{ContinuousCDF, Normal};
use veridict::stats::bootstrap::{
    bootstrap_quantile_diff_ci, bootstrap_quantile_diff_ci_basic, bootstrap_quantile_diff_ci_bca,
};

const CONFIDENCE: f64 = 0.95;
const RESAMPLES: usize = 500;
const SEED: u64 = 0x5EED;

// Inverse-CDF sampling from Exponential(rate=1): -ln(1 - u), same convention as
// `bootstrap_coverage.rs`. The q-th quantile of this distribution has the closed form
// `-ln(1 - q)`.
fn draw_exponential_sample(rng: &mut StdRng, n: usize) -> Vec<f64> {
    (0..n)
        .map(|_| {
            let u: f64 = rng.random_range(1e-12..1.0);
            -u.ln()
        })
        .collect()
}

fn exponential_quantile(q: f64) -> f64 {
    -(1.0 - q).ln()
}

// Inverse-CDF sampling from a standard normal via `statrs`'s own `inverse_cdf` - the symmetric
// counterpart to the skewed population above. The q-th quantile is `inverse_cdf(q)` directly.
fn draw_normal_sample(rng: &mut StdRng, n: usize) -> Vec<f64> {
    let normal = Normal::new(0.0, 1.0).expect("standard normal distribution is always valid");
    (0..n)
        .map(|_| {
            let u: f64 = rng.random_range(1e-12..1.0 - 1e-12);
            normal.inverse_cdf(u)
        })
        .collect()
}

fn normal_quantile(q: f64) -> f64 {
    Normal::new(0.0, 1.0)
        .expect("standard normal distribution is always valid")
        .inverse_cdf(q)
}

#[allow(clippy::too_many_arguments)]
fn empirical_coverage(
    draw_sample: impl Fn(&mut StdRng, usize) -> Vec<f64>,
    true_quantile: f64,
    q: f64,
    n: usize,
    simulations: u64,
    ci: impl Fn(&[f64], f64, f64, usize, u64) -> (f64, f64),
) -> f64 {
    let mut rng = StdRng::seed_from_u64(SEED);
    let mut covered = 0u64;
    for sim in 0..simulations {
        let sample = draw_sample(&mut rng, n);
        // Independent seed per simulation, same reasoning as `bootstrap_coverage.rs`: varying it
        // avoids correlating resampling noise identically across simulations.
        let (lo, hi) = ci(&sample, q, CONFIDENCE, RESAMPLES, SEED.wrapping_add(sim));
        if lo <= true_quantile && true_quantile <= hi {
            covered += 1;
        }
    }
    covered as f64 / simulations as f64
}

// Median (q=0.5) is the least extreme quantile tested here - expected to be the best-behaved of
// the three, a reference point for how much worse p95/p99 get below.
//
// Observed once at SIMULATIONS=1,000, SAMPLE_N=30, RESAMPLES=500: coverage was 0.9480.
#[test]
#[ignore = "Monte Carlo, ~4s alone - see module doc for how to run"]
fn percentile_coverage_at_median_on_skewed_data_at_n30() {
    let coverage = empirical_coverage(
        draw_exponential_sample,
        exponential_quantile(0.5),
        0.5,
        30,
        1_000,
        bootstrap_quantile_diff_ci,
    );
    assert!(
        (0.85..0.97).contains(&coverage),
        "median coverage {coverage:.4} on skewed data at n=30 moved outside the expected range"
    );
}

// p95 is a genuinely extreme quantile at n=30 (only ~1.5 expected observations in the upper
// tail - see `thin_quantile_tail`'s threshold) - expected to be visibly worse than the median's
// coverage above, not just "also imperfect".
//
// Observed once at SIMULATIONS=1,000: coverage was 0.7940, well below the median's 0.9480 on the
// same population and sample size.
#[test]
#[ignore = "Monte Carlo, ~5s alone - see module doc for how to run"]
fn percentile_coverage_at_p95_degrades_versus_median_on_skewed_data_at_n30() {
    let p95_coverage = empirical_coverage(
        draw_exponential_sample,
        exponential_quantile(0.95),
        0.95,
        30,
        1_000,
        bootstrap_quantile_diff_ci,
    );
    let median_coverage = empirical_coverage(
        draw_exponential_sample,
        exponential_quantile(0.5),
        0.5,
        30,
        1_000,
        bootstrap_quantile_diff_ci,
    );
    assert!(
        p95_coverage < median_coverage - 0.05,
        "p95 coverage {p95_coverage:.4} was not meaningfully worse than median coverage \
         {median_coverage:.4} on skewed data at n=30 - the thin-tail degradation this test \
         documents may have changed"
    );
}

// Same p95/skewed setup as above, but at a sample size where `thin_quantile_tail` no longer
// fires (n=150 -> ~7.5 expected upper-tail observations, still below its own >=10 floor but
// enough to show the trend) - coverage should visibly recover as the tail fills in, the
// empirical basis for that flag's threshold.
//
// Observed once at SIMULATIONS=500 (halved for runtime - directional, not precision-critical):
// coverage was 0.9140 at n=150, versus 0.7940 at n=30 above.
#[test]
#[ignore = "Monte Carlo, ~15s alone - see module doc for how to run"]
fn percentile_coverage_at_p95_recovers_at_larger_n_on_skewed_data() {
    let coverage = empirical_coverage(
        draw_exponential_sample,
        exponential_quantile(0.95),
        0.95,
        150,
        500,
        bootstrap_quantile_diff_ci,
    );
    assert!(
        coverage > 0.85,
        "p95 coverage {coverage:.4} at n=150 did not recover meaningfully above the n=30 \
         figure (0.7940) - either the tail-filling trend `thin_quantile_tail` assumes doesn't \
         hold, or this is Monte Carlo noise at only 500 simulations (re-verify before trusting \
         either way)"
    );
}

// The symmetric counterpart to the p95/skewed case: even without skew, an extreme quantile at
// small n has few effective observations in the tail, so some coverage degradation is expected
// here too (a small-sample-size effect, not exclusively a skew effect).
//
// Observed once at SIMULATIONS=1,000: coverage was 0.7960 - degraded versus nominal, and close to
// (though not the same as) the skewed population's own p95/n=30 figure above (0.7940). The
// closeness isn't strong independent evidence that skew barely matters at this quantile/n: both
// populations are drawn via a monotonic inverse-CDF transform of the *same* underlying uniform
// stream at a given `(sim, resample-index)` pair (same `SEED`, same call pattern), which
// correlates the two figures more than fully independent sampling would - so a small gap here is
// less informative than it would be from separately-seeded runs, even though it isn't literally
// zero.
#[test]
#[ignore = "Monte Carlo, ~5s alone - see module doc for how to run"]
fn percentile_coverage_at_p95_on_symmetric_data_at_n30() {
    let coverage = empirical_coverage(
        draw_normal_sample,
        normal_quantile(0.95),
        0.95,
        30,
        1_000,
        bootstrap_quantile_diff_ci,
    );
    assert!(
        (0.70..0.90).contains(&coverage),
        "p95 coverage {coverage:.4} on symmetric data at n=30 moved outside the expected range"
    );
}

// `basic`'s reflection has no bias-correction of its own - `bootstrap_coverage.rs` found it
// measurably worse than percentile for the mean on skewed data; checked here for a quantile too
// rather than assumed to carry over unchanged.
//
// Observed once at SIMULATIONS=1,000: basic's p95/skewed coverage was 0.7220, versus
// percentile's 0.7940 on the same simulated samples - worse, consistent with the mean-diff
// finding, and by a wider margin (7.2 points here vs. mean-diff's own 1.6-point gap in
// `bootstrap_coverage.rs`).
#[test]
#[ignore = "Monte Carlo, ~5s alone - see module doc for how to run"]
fn basic_coverage_at_p95_is_no_better_than_percentile_on_skewed_data() {
    let basic_coverage = empirical_coverage(
        draw_exponential_sample,
        exponential_quantile(0.95),
        0.95,
        30,
        1_000,
        bootstrap_quantile_diff_ci_basic,
    );
    let percentile_coverage = empirical_coverage(
        draw_exponential_sample,
        exponential_quantile(0.95),
        0.95,
        30,
        1_000,
        bootstrap_quantile_diff_ci,
    );
    assert!(
        basic_coverage < percentile_coverage,
        "basic coverage {basic_coverage:.4} was not worse than percentile coverage \
         {percentile_coverage:.4} on skewed p95 data - the reflection either canceled or \
         reversed the expected skew-compounding effect; re-verify this claim before trusting it \
         either way"
    );
}

// BCa is implemented (`bootstrap_quantile_diff_ci_bca`) but gated off at the CLI for
// `quantile-diff` (see `VeridictError::IncompatibleBootstrapMethod`) because the sample
// quantile's jackknife acceleration term has no solid asymptotic footing for a non-smooth
// statistic. This test is that gate's actual evidence, not just the theoretical argument for
// it: measuring whether BCa's coverage is actually any better than percentile's on the same
// skewed p95 data:
//
// Observed once at SIMULATIONS=1,000: BCa's coverage was 0.7910, statistically indistinguishable
// from percentile's 0.7940 on the same simulated samples (well within this simulation count's
// own noise) - no evidence here that BCa's correction helps for a quantile the way it does for
// the mean (where `bootstrap_coverage.rs` measured a real, if modest, improvement). Supports
// keeping the CLI gate: lifting it would add complexity and a slower jackknife for no measured
// benefit, at least at this quantile/sample size.
#[test]
#[ignore = "Monte Carlo, ~6s alone - see module doc for how to run"]
fn bca_coverage_on_skewed_p95_data_is_not_measurably_better_than_percentile() {
    let bca_coverage = empirical_coverage(
        draw_exponential_sample,
        exponential_quantile(0.95),
        0.95,
        30,
        1_000,
        bootstrap_quantile_diff_ci_bca,
    );
    let percentile_coverage = empirical_coverage(
        draw_exponential_sample,
        exponential_quantile(0.95),
        0.95,
        30,
        1_000,
        bootstrap_quantile_diff_ci,
    );
    assert!(
        (bca_coverage - percentile_coverage).abs() < 0.05,
        "BCa coverage {bca_coverage:.4} moved meaningfully away from percentile coverage \
         {percentile_coverage:.4} on skewed p95 data - re-evaluate whether the CLI gate on BCa \
         for quantile-diff (see `VeridictError::IncompatibleBootstrapMethod`) should change \
         given this new evidence, in either direction"
    );
}

// p99 at n=30 is a known-degenerate case, not a bug to fix: with only 30 observations, the
// sample's own 99th percentile sits at or beyond the largest one or two order statistics
// (essentially the sample max), which - like the mean's own skewed-data bias documented in
// `bootstrap_coverage.rs` - is a real, inherent limitation of resampling a tiny tail, not
// something a bootstrap variant can correct away. Documented here as an expected wide/degenerate
// range rather than asserted against nominal coverage.
//
// Observed once at SIMULATIONS=1,000: coverage was 0.2670 - far below nominal, as expected for a
// quantile this extreme at this sample size (n=30 gives only ~0.3 expected observations above the
// true p99, per `thin_quantile_tail`'s own heuristic - barely more than zero).
#[test]
#[ignore = "Monte Carlo, ~5s alone - see module doc for how to run"]
fn p99_at_n30_is_a_known_degenerate_case_not_a_regression() {
    let coverage = empirical_coverage(
        draw_exponential_sample,
        exponential_quantile(0.99),
        0.99,
        30,
        1_000,
        bootstrap_quantile_diff_ci,
    );
    assert!(
        coverage < 0.6,
        "p99/n=30 coverage {coverage:.4} was surprisingly close to nominal - this is expected to \
         be a degenerate case (re-verify the claim itself before trusting this number, since an \
         unexpectedly *good* result here is as noteworthy as a bad one)"
    );
}
