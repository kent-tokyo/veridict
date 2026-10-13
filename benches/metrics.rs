//! Quantifies the P0-1 single-pass win: `compute_many` scanning `records`
//! once for N metrics, versus the pre-refactor shape of N independent
//! `compute` calls (one full scan each). Measured result, not assumed: for
//! the three cheap O(n) metrics (no per-record heap growth), single-pass is
//! consistently faster (~12% at N=20,000 here). Adding `mean-diff` (which
//! buffers every diff for its bootstrap) erases that win in this benchmark -
//! looping over 4 heterogeneous `Box<dyn MetricAggregator>` per record is
//! megamorphic dispatch, which defeats indirect-branch prediction in a way
//! that N separate monomorphic passes don't suffer from; that cost can
//! outweigh the saved re-scans once one metric's per-record work is heavy
//! enough. Both scenarios are benchmarked so this tradeoff stays visible
//! instead of asserting a universal win that doesn't hold.
//!
//! `compute`/`compute_many` take a streaming iterator (see
//! `metrics::compute_many`'s doc), so every call site here wraps the
//! in-memory `records` with `.iter().cloned()` - an accepted methodology
//! shift, not a regression to chase: a `Record::clone()` per record now
//! sits inside the timed region, but real streamed input allocates a
//! fresh `Record` per line anyway (via `serde_json`/`csv`
//! deserialization), so this isn't measuring something unrealistic.

use std::hint::black_box;

use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use rand::rngs::StdRng;
use rand::{RngExt, SeedableRng};
use veridict::input::Record;
use veridict::metrics::{compute, compute_many};
use veridict::stats::bootstrap::mean;
use veridict::{BootstrapMethod, CiMethod, FailurePolicy, MetricConfig};

const N: usize = 20_000;
const BOOTSTRAP_N: usize = 2_000;

fn synthetic_records() -> Vec<(usize, Record)> {
    (0..N)
        .map(|i| {
            let baseline = (i % 7) as f64;
            let candidate = baseline + 0.5;
            let result = match i % 3 {
                0 => "candidate_win",
                1 => "baseline_win",
                _ => "draw",
            };
            (
                i + 1,
                Record {
                    id: None,
                    baseline: Some(baseline),
                    candidate: Some(candidate),
                    result: Some(result.to_string()),
                    baseline_status: None,
                    candidate_status: None,
                },
            )
        })
        .collect()
}

const ALL_METRICS: [MetricConfig; 4] = [
    MetricConfig::WinRate {
        ci_method: CiMethod::Wilson,
        failure_policy: FailurePolicy::ReportOnly,
    },
    MetricConfig::MeanDiff {
        bootstrap_method: BootstrapMethod::Percentile,
    },
    MetricConfig::SignTest {
        ci_method: CiMethod::Wilson,
    },
    MetricConfig::Elo {
        failure_policy: FailurePolicy::ReportOnly,
    },
];
// mean-diff's bootstrap resampling dominates total runtime regardless of how
// many passes over `records` happen, which swamps the single-pass win in a
// benchmark that includes it - this subset isolates the O(n) metrics, where
// the record-scan/status-tally overhead this refactor collapses is actually
// the bottleneck being measured.
const CHEAP_METRICS: [MetricConfig; 3] = [
    MetricConfig::WinRate {
        ci_method: CiMethod::Wilson,
        failure_policy: FailurePolicy::ReportOnly,
    },
    MetricConfig::SignTest {
        ci_method: CiMethod::Wilson,
    },
    MetricConfig::Elo {
        failure_policy: FailurePolicy::ReportOnly,
    },
];

fn bench_single_pass_all_metrics(c: &mut Criterion) {
    let records = synthetic_records();
    c.bench_function(
        "compute_many (single pass, 4 metrics incl. mean-diff)",
        |b| {
            b.iter(|| {
                compute_many(
                    records.iter().cloned(),
                    &ALL_METRICS,
                    0.95,
                    2000,
                    0x5EED,
                    false,
                    false,
                )
                .unwrap()
            });
        },
    );
}

fn bench_independent_calls_all_metrics(c: &mut Criterion) {
    let records = synthetic_records();
    c.bench_function(
        "4x independent compute (pre-P0-1 shape, incl. mean-diff)",
        |b| {
            b.iter(|| {
                for &metric in &ALL_METRICS {
                    compute(
                        records.iter().cloned(),
                        metric,
                        0.95,
                        2000,
                        0x5EED,
                        false,
                        false,
                    )
                    .unwrap();
                }
            });
        },
    );
}

fn bench_single_pass_cheap_metrics(c: &mut Criterion) {
    let records = synthetic_records();
    c.bench_function("compute_many (single pass, 3 O(n) metrics)", |b| {
        b.iter(|| {
            compute_many(
                records.iter().cloned(),
                &CHEAP_METRICS,
                0.95,
                2000,
                0x5EED,
                false,
                false,
            )
            .unwrap()
        });
    });
}

fn bench_independent_calls_cheap_metrics(c: &mut Criterion) {
    let records = synthetic_records();
    c.bench_function(
        "3x independent compute (pre-P0-1 shape, O(n) metrics)",
        |b| {
            b.iter(|| {
                for &metric in &CHEAP_METRICS {
                    compute(
                        records.iter().cloned(),
                        metric,
                        0.95,
                        2000,
                        0x5EED,
                        false,
                        false,
                    )
                    .unwrap();
                }
            });
        },
    );
}

fn bench_mean_diff_release_workload(c: &mut Criterion) {
    let records: Vec<_> = synthetic_records().into_iter().take(BOOTSTRAP_N).collect();
    c.bench_function("mean-diff (N=2000, resamples=10000)", |b| {
        b.iter(|| {
            compute(
                records.iter().cloned(),
                MetricConfig::MeanDiff {
                    bootstrap_method: BootstrapMethod::Percentile,
                },
                0.95,
                10_000,
                0x5EED,
                false,
                false,
            )
            .unwrap()
        });
    });
}

/// Diagnostic decomposition of the release workload above. These three
/// microbenchmarks deliberately keep the production API closed: they measure
/// the same amount of RNG index generation, the real compensated `mean`, and
/// the final distribution sort/percentile lookup separately. Their times are
/// not expected to add up exactly to the end-to-end benchmark because the
/// production loop interleaves index generation and accumulation, while this
/// diagnostic isolates them and therefore has different cache behavior.
fn bench_mean_diff_bootstrap_components(c: &mut Criterion) {
    const RESAMPLES: usize = 10_000;
    const DRAWS: usize = BOOTSTRAP_N * RESAMPLES;

    let values: Vec<f64> = (0..BOOTSTRAP_N)
        .map(|i| (i % 7) as f64 + if i % 2 == 0 { 0.5 } else { -0.25 })
        .collect();
    let distribution: Vec<f64> = (0..RESAMPLES)
        .map(|i| ((i * 7_919) % RESAMPLES) as f64 / RESAMPLES as f64)
        .collect();

    let mut group = c.benchmark_group("mean-diff bootstrap components");
    group.sample_size(20);

    group.bench_function("resampling indices (20m draws)", |b| {
        b.iter(|| {
            let mut rng = StdRng::seed_from_u64(0x5EED);
            let mut sink = 0usize;
            for _ in 0..DRAWS {
                sink ^= rng.random_range(0..BOOTSTRAP_N);
            }
            black_box(sink)
        });
    });

    group.bench_function("compensated mean (10000 x N=2000)", |b| {
        b.iter(|| {
            let mut sink = 0.0;
            for _ in 0..RESAMPLES {
                sink += mean(black_box(&values));
            }
            black_box(sink)
        });
    });

    group.bench_function("interval sort and lookup (N=10000)", |b| {
        b.iter_batched(
            || distribution.clone(),
            |mut draws| {
                draws.sort_by(f64::total_cmp);
                let lo = ((0.025 * RESAMPLES as f64).floor() as usize).min(RESAMPLES - 1);
                let hi = ((0.975 * RESAMPLES as f64).ceil() as usize)
                    .saturating_sub(1)
                    .min(RESAMPLES - 1)
                    .max(lo);
                black_box((draws[lo], draws[hi]))
            },
            BatchSize::SmallInput,
        );
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_single_pass_all_metrics,
    bench_independent_calls_all_metrics,
    bench_single_pass_cheap_metrics,
    bench_independent_calls_cheap_metrics,
    bench_mean_diff_release_workload,
    bench_mean_diff_bootstrap_components
);
criterion_main!(benches);
