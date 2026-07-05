//! Quantifies the general-graph Bradley-Terry solver's cost, and specifically
//! the overhead the star-graph routing decision in `matrix.rs` avoids: on
//! star-shaped input, `matrix::run` uses a closed form (`elo_from_score`
//! per file) rather than ever invoking `bradley_terry::fit_graph` - the
//! first two benchmarks here measure both sides of that choice on
//! equivalent data, so the tradeoff is visible rather than assumed.

use criterion::{Criterion, criterion_group, criterion_main};
use veridict::BootstrapMethod;
use veridict::VeridictError;
use veridict::input::Record;
use veridict::matrix;
use veridict::stats::bootstrap::DEFAULT_SEED;
use veridict::stats::bradley_terry::{PairRecord, bootstrap_pairwise_elo_diff_cis, fit_graph};

fn win_loss_draw_records(n: usize, candidate_win_fraction: f64) -> Vec<(usize, Record)> {
    let candidate_wins = (n as f64 * candidate_win_fraction) as usize;
    (0..n)
        .map(|i| {
            let result = if i < candidate_wins {
                "candidate_win"
            } else {
                "baseline_win"
            };
            (
                i + 1,
                Record {
                    id: None,
                    baseline: None,
                    candidate: None,
                    result: Some(result.to_string()),
                    baseline_status: None,
                    candidate_status: None,
                },
            )
        })
        .collect()
}

type MatchFile = Vec<Result<(usize, veridict::input::MatchRecord), VeridictError>>;

fn no_matches() -> Vec<Result<MatchFile, VeridictError>> {
    Vec::new()
}

/// A star graph: node 0 is the implicit hub (`baseline`'s role), nodes
/// `1..n` each only play node 0 - the topology the closed form is proven
/// equivalent to and the router in `matrix.rs` detects and shortcuts.
fn star_pairs(n: usize) -> Vec<PairRecord> {
    (1..n)
        .map(|i| PairRecord {
            a: 0,
            b: i,
            a_wins: 12,
            b_wins: 8,
            draws: 0,
        })
        .collect()
}

/// Round-robin: every pair of nodes plays each other directly - the
/// densest topology at a given node count.
fn dense_pairs(n: usize, games_per_pair: u64) -> Vec<PairRecord> {
    let a_wins = games_per_pair * 3 / 5;
    let b_wins = games_per_pair - a_wins;
    (0..n)
        .flat_map(|i| ((i + 1)..n).map(move |j| (i, j)))
        .map(|(a, b)| PairRecord {
            a,
            b,
            a_wins,
            b_wins,
            draws: 0,
        })
        .collect()
}

/// Ring: each node only plays its two neighbors - the sparsest topology
/// that's still (with a return game on every edge) strongly connected.
fn sparse_ring_pairs(n: usize, games_per_pair: u64) -> Vec<PairRecord> {
    let a_wins = games_per_pair * 3 / 5;
    let b_wins = games_per_pair - a_wins;
    (0..n)
        .map(|i| PairRecord {
            a: i,
            b: (i + 1) % n,
            a_wins,
            b_wins,
            draws: 0,
        })
        .collect()
}

const STAR_N: usize = 50;

fn bench_closed_form_on_star_shaped_input(c: &mut Criterion) {
    c.bench_function(
        "matrix::run closed-form path on star-shaped input (N=50)",
        |b| {
            b.iter(|| {
                let named = (1..STAR_N).map(|i| {
                    Ok::<_, VeridictError>((
                        format!("c{i}"),
                        win_loss_draw_records(20, 0.6).into_iter().map(Ok),
                    ))
                });
                matrix::run(
                    named,
                    no_matches(),
                    0.95,
                    false,
                    0,
                    DEFAULT_SEED,
                    BootstrapMethod::Percentile,
                )
                .unwrap()
            });
        },
    );
}

fn bench_general_solver_on_star_shaped_input(c: &mut Criterion) {
    let pairs = star_pairs(STAR_N);
    c.bench_function(
        "bradley_terry::fit_graph on the same star-shaped input (N=50)",
        |b| {
            b.iter(|| fit_graph(STAR_N, &pairs).unwrap());
        },
    );
}

fn bench_solver_convergence_by_node_count(c: &mut Criterion) {
    for &n in &[5usize, 20, 100] {
        let pairs = dense_pairs(n, 20);
        c.bench_function(&format!("fit_graph dense round-robin (N={n})"), |b| {
            b.iter(|| fit_graph(n, &pairs).unwrap());
        });
    }
}

fn bench_solver_sparse_vs_dense_graph(c: &mut Criterion) {
    const N: usize = 30;
    let dense = dense_pairs(N, 20);
    let sparse = sparse_ring_pairs(N, 20);
    c.bench_function(&format!("fit_graph dense round-robin (N={N})"), |b| {
        b.iter(|| fit_graph(N, &dense).unwrap());
    });
    c.bench_function(&format!("fit_graph sparse ring (N={N})"), |b| {
        b.iter(|| fit_graph(N, &sparse).unwrap());
    });
}

fn bench_bootstrap_pairwise_cis(c: &mut Criterion) {
    // Fixed, modest scope matching this file's existing benchmarks: a
    // mixed-graph-shaped input at a resample count representative of
    // `matrix`'s CLI default (2,000), so this tracks the actual cost users
    // pay for general-graph confidence intervals.
    const N: usize = 20;
    let pairs = dense_pairs(N, 20);
    let fits = fit_graph(N, &pairs).unwrap();
    c.bench_function(
        "bootstrap_pairwise_elo_diff_cis dense round-robin (N=20, resamples=500)",
        |b| {
            b.iter(|| {
                bootstrap_pairwise_elo_diff_cis(
                    N,
                    &pairs,
                    &fits,
                    500,
                    DEFAULT_SEED,
                    0.95,
                    BootstrapMethod::Percentile,
                )
            });
        },
    );
}

fn bench_bootstrap_pairwise_cis_large(c: &mut Criterion) {
    // The CLI's actual default (`--resamples 2000`) at the largest node
    // count this file benchmarks elsewhere (N=100 dense) - the case that
    // motivated parallelizing bootstrap resampling in the first place. Only
    // the parallel (production) implementation is reachable here - the
    // plain serial reference lives behind `#[cfg(test)]` in
    // `bradley_terry.rs`, now kept as a statistical (not bit-identical)
    // reference - see `bootstrap_pairwise_elo_diff_cis`'s module docs for
    // why the two no longer produce the same draws. One-time before/after
    // measurement at this exact shape (10-core machine, release build):
    // serial (the `#[cfg(test)]` oracle) ~7.9s, parallel (this benchmark)
    // ~2.1s - about 3.8x. An earlier design that kept RNG generation
    // serial and parallelized only the `fit_graph` refit only reached
    // ~1.6x, Amdahl-bound by that serial resampling step; giving every
    // resample its own independently-seeded RNG stream (so resampling
    // itself parallelizes too, not just the refit) removed most of that
    // ceiling. Re-run this benchmark to check whether the ratio still
    // holds after any future change to the resampling or harvest steps.
    const N: usize = 100;
    let pairs = dense_pairs(N, 20);
    let fits = fit_graph(N, &pairs).unwrap();
    let mut group = c.benchmark_group("bootstrap_pairwise_elo_diff_cis large");
    group.sample_size(10);
    group.bench_function("dense round-robin (N=100, resamples=2000)", |b| {
        b.iter(|| {
            bootstrap_pairwise_elo_diff_cis(
                N,
                &pairs,
                &fits,
                2000,
                DEFAULT_SEED,
                0.95,
                BootstrapMethod::Percentile,
            )
        });
    });
    group.finish();
}

criterion_group!(
    benches,
    bench_closed_form_on_star_shaped_input,
    bench_general_solver_on_star_shaped_input,
    bench_solver_convergence_by_node_count,
    bench_solver_sparse_vs_dense_graph,
    bench_bootstrap_pairwise_cis,
    bench_bootstrap_pairwise_cis_large,
);
criterion_main!(benches);
