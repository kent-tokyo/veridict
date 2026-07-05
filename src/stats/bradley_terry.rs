//! General-graph Bradley-Terry MLE solver (Zermelo 1929 / Hunter 2004's
//! Minorization-Maximization fixed point), for `matrix --matches` data where
//! candidates play each other directly and the star-graph closed-form
//! shortcut (see `matrix.rs`'s module doc) doesn't apply.
//!
//! **Existence requires strong connectivity of the directed "who scored any
//! points against whom" graph** (Ford 1957): if some nonempty proper subset
//! of competitors never lost or drew against anyone outside it, that
//! subset's ratings diverge to infinity relative to the rest - there is no
//! finite MLE, not merely a wide/uncertain one. This is derived directly
//! from the log-likelihood in this project's own verification (pushing a
//! subset's ratings to infinity strictly increases the likelihood whenever
//! that subset never conceded a point to the outside), not just cited. A
//! draw counts as scoring a point in **both** directions for this purpose
//! (0.5 to each side) - treating a draw as "no edge" would wrongly flag
//! legitimate all-draws data as disconnected.
//!
//! Consequently, `fit_graph` always computes strongly connected components
//! first and fits each independently - it never infers disconnection from
//! the iteration failing to converge, because a genuinely divergent
//! component's relative-change-per-iteration can decay like `1/n` (already
//! shrinking on every step, but never below any fixed threshold): no choice
//! of iteration cap and tolerance reliably tells "slowly converging" apart
//! from "slowly diverging" by watching iteration behavior alone, while the
//! structural check is both cheap (`O(competitors + pairs)`) and exact.
//! Non-convergence within the cap therefore means something else: a
//! technically-connected but heavily lopsided component.
//!
//! Each component's ratings are relative to that component's own pin (its
//! lowest-index member) and are **not comparable** across components -
//! callers must check `CompetitorFit::component` before treating two
//! `elo` values as a meaningful difference.

use std::collections::HashMap;

use rand::rngs::StdRng;
use rand::{RngExt, SeedableRng};

use crate::BootstrapMethod;
use crate::error::VeridictError;
use crate::stats::bootstrap::{
    bca_adjusted_percentiles, bias_correction_z0, hi_index, lo_index, resample_edge_multinomial,
    weighted_acceleration,
};

/// Aggregated head-to-head tally between two competitors, identified by
/// 0-based index into the caller's competitor list. This module does no
/// I/O and knows nothing about record schemas - callers aggregate raw
/// match records into these first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PairRecord {
    pub a: usize,
    pub b: usize,
    pub a_wins: u64,
    pub b_wins: u64,
    pub draws: u64,
}

/// Which mutually-comparable group each competitor belongs to. Same
/// `component_of[i]` means ratings share one scale and `elo[i] - elo[j]` is
/// a real, finite, estimable quantity; different components mean there is
/// no finite Bradley-Terry estimate of relative strength at all (see module
/// docs).
#[derive(Debug, Clone)]
pub struct Connectivity {
    pub component_of: Vec<usize>,
    pub num_components: usize,
}

/// Kosaraju's algorithm on the directed graph implied by `pairs` (edge a->b
/// iff a's total score against b - wins plus 0.5 per draw - is > 0; a draw
/// therefore creates an edge in both directions). Iterative, not recursive,
/// so a large input can't stack-overflow. `O(n_competitors + pairs.len())`.
pub fn strongly_connected_components(n_competitors: usize, pairs: &[PairRecord]) -> Connectivity {
    let mut adj = vec![Vec::new(); n_competitors];
    let mut radj = vec![Vec::new(); n_competitors];
    for p in pairs {
        if p.a_wins > 0 {
            adj[p.a].push(p.b);
            radj[p.b].push(p.a);
        }
        if p.b_wins > 0 {
            adj[p.b].push(p.a);
            radj[p.a].push(p.b);
        }
        if p.draws > 0 {
            adj[p.a].push(p.b);
            radj[p.b].push(p.a);
            adj[p.b].push(p.a);
            radj[p.a].push(p.b);
        }
    }

    // Pass 1: finish order via iterative DFS on the forward graph.
    let mut visited = vec![false; n_competitors];
    let mut finish_order = Vec::with_capacity(n_competitors);
    for start in 0..n_competitors {
        if visited[start] {
            continue;
        }
        visited[start] = true;
        let mut stack = vec![(start, 0usize)];
        while let Some(&mut (node, ref mut next_idx)) = stack.last_mut() {
            if *next_idx < adj[node].len() {
                let neighbor = adj[node][*next_idx];
                *next_idx += 1;
                if !visited[neighbor] {
                    visited[neighbor] = true;
                    stack.push((neighbor, 0));
                }
            } else {
                finish_order.push(node);
                stack.pop();
            }
        }
    }

    // Pass 2: DFS on the reverse graph in reverse finish order; each DFS
    // tree is exactly one strongly connected component.
    let mut component_of = vec![usize::MAX; n_competitors];
    let mut num_components = 0;
    for &start in finish_order.iter().rev() {
        if component_of[start] != usize::MAX {
            continue;
        }
        let comp = num_components;
        num_components += 1;
        component_of[start] = comp;
        let mut stack = vec![start];
        while let Some(node) = stack.pop() {
            for &neighbor in &radj[node] {
                if component_of[neighbor] == usize::MAX {
                    component_of[neighbor] = comp;
                    stack.push(neighbor);
                }
            }
        }
    }

    Connectivity {
        component_of,
        num_components,
    }
}

/// A competitor's fitted rating. `elo` is relative to this competitor's OWN
/// component's pin (0.0 for the pin itself) and is not comparable across
/// different `component` values.
#[derive(Debug, Clone, Copy)]
pub struct CompetitorFit {
    pub component: usize,
    pub elo: f64,
}

pub const MAX_ITERATIONS: usize = 10_000;
/// Relative change in `pi` between iterations, below which a component is
/// considered converged. `1e-9` on the win-ratio scale is about `1.7e-7`
/// elo (`400/ln(10) * 1e-9`) - far below any reportable precision, while
/// loose enough that well-posed graphs settle in well under 100 iterations.
pub const CONVERGENCE_THRESHOLD: f64 = 1e-9;

/// Fits Bradley-Terry ratings independently per strongly connected
/// component (Zermelo/Hunter MM fixed point: `pi_i(new) = W_i /
/// sum_j[n_ij / (pi_i + pi_j)]`). Always computes `strongly_connected_components`
/// first (see module docs for why). Pin = lowest input-order index in each
/// component (arbitrary but irrelevant: any `elo_i - elo_j` within a
/// component is pin-invariant by construction - Bradley-Terry only
/// identifies ratios).
pub fn fit_graph(
    n_competitors: usize,
    pairs: &[PairRecord],
) -> Result<Vec<CompetitorFit>, VeridictError> {
    debug_assert!(n_competitors > 0, "fit_graph called with zero competitors");
    for p in pairs {
        debug_assert!(p.a != p.b, "fit_graph given a self-pair");
        debug_assert!(
            p.a < n_competitors && p.b < n_competitors,
            "fit_graph given an out-of-range competitor index"
        );
        debug_assert!(
            p.a_wins + p.b_wins + p.draws > 0,
            "fit_graph given a pair with zero games"
        );
    }

    let connectivity = strongly_connected_components(n_competitors, pairs);

    let mut members: Vec<Vec<usize>> = vec![Vec::new(); connectivity.num_components];
    for (global_idx, &comp) in connectivity.component_of.iter().enumerate() {
        members[comp].push(global_idx);
    }

    let mut local_index = vec![0usize; n_competitors];
    for member_list in &members {
        for (local_idx, &global_idx) in member_list.iter().enumerate() {
            local_index[global_idx] = local_idx;
        }
    }

    let mut component_pairs: Vec<Vec<PairRecord>> = vec![Vec::new(); connectivity.num_components];
    for p in pairs {
        let comp = connectivity.component_of[p.a];
        if comp == connectivity.component_of[p.b] {
            component_pairs[comp].push(PairRecord {
                a: local_index[p.a],
                b: local_index[p.b],
                a_wins: p.a_wins,
                b_wins: p.b_wins,
                draws: p.draws,
            });
        }
    }

    let mut elo = vec![0.0; n_competitors];
    for comp in 0..connectivity.num_components {
        let n_local = members[comp].len();
        if n_local <= 1 {
            continue; // singleton component: the only member is its own pin.
        }
        let local_elo = fit_component(n_local, &component_pairs[comp], &members[comp])?;
        for (local_idx, &global_idx) in members[comp].iter().enumerate() {
            elo[global_idx] = local_elo[local_idx];
        }
    }

    Ok((0..n_competitors)
        .map(|i| CompetitorFit {
            component: connectivity.component_of[i],
            elo: elo[i],
        })
        .collect())
}

/// Runs the MM fixed point to convergence for one already-verified-connected
/// component, `members[i]` giving the global competitor index for local
/// index `i` (used only to build an informative error on non-convergence).
fn fit_component(
    n_local: usize,
    pairs: &[PairRecord],
    members: &[usize],
) -> Result<Vec<f64>, VeridictError> {
    let mut pi = vec![1.0; n_local];
    let mut last_relative_change = f64::INFINITY;
    let mut worst_local = 0usize;

    for _ in 0..MAX_ITERATIONS {
        let mut wins = vec![0.0; n_local];
        let mut denom = vec![0.0; n_local];
        for p in pairs {
            let n = (p.a_wins + p.b_wins + p.draws) as f64;
            wins[p.a] += p.a_wins as f64 + 0.5 * p.draws as f64;
            wins[p.b] += p.b_wins as f64 + 0.5 * p.draws as f64;
            let shared = n / (pi[p.a] + pi[p.b]);
            denom[p.a] += shared;
            denom[p.b] += shared;
        }

        let mut new_pi: Vec<f64> = (0..n_local)
            .map(|i| {
                if denom[i] > 0.0 {
                    wins[i] / denom[i]
                } else {
                    pi[i]
                }
            })
            .collect();
        let scale = new_pi[0];
        for v in &mut new_pi {
            *v /= scale;
        }

        let mut max_relative_change = 0.0_f64;
        for i in 0..n_local {
            let rel = (new_pi[i] - pi[i]).abs() / pi[i];
            if rel > max_relative_change {
                max_relative_change = rel;
                worst_local = i;
            }
        }
        last_relative_change = max_relative_change;
        pi = new_pi;

        if max_relative_change < CONVERGENCE_THRESHOLD {
            return Ok(pi.iter().map(|&p| 400.0 * p.log10()).collect());
        }
    }

    Err(VeridictError::BradleyTerryDidNotConverge {
        iterations: MAX_ITERATIONS,
        last_relative_change,
        threshold: CONVERGENCE_THRESHOLD,
        worst_competitor: members[worst_local],
    })
}

/// Minimum fraction of bootstrap resamples that must both converge and keep
/// `i`/`j` in the same component for that pair's CI to be reported at all.
/// Set high, not a bare majority: a percentile CI computed from only the
/// resamples that happened to stay connected is conditioned on
/// connectivity, which discards exactly the most extreme resamples and is
/// therefore optimistically narrow - the opposite of this project's "a
/// false pass is worse than an inconclusive result" bias.
pub const CONNECTED_FRACTION_THRESHOLD: f64 = 0.9;

/// Bootstrap confidence intervals for every pairwise `elo_i - elo_j` where
/// `i` and `j` share a component in `fits` (the caller's already-computed
/// real fit, reused rather than recomputed here so the bootstrap can never
/// disagree with the real fit about which pairs are even eligible, and so
/// `fits[i].elo - fits[j].elo` is available as the true point estimate for
/// `Basic`/`Bca`'s bias correction without an extra refit).
///
/// One resample = a fresh `resample_edge_multinomial` draw of every edge in
/// `pairs`, followed by exactly one `strongly_connected_components` +
/// `fit_graph` refit of the whole graph - cost is `O(resamples *
/// fit_graph(n_competitors))`, independent of the number of pairs being
/// harvested, since every pair's draw comes from that one refit rather than
/// a refit of its own. A resample where `fit_graph` returns `Err` (a
/// pathological, heavily-lopsided resample that didn't converge)
/// contributes nothing to any pair, exactly like a resample where that pair
/// split into different components - both cases simply fail to extend that
/// pair's draw list; there is no separate "converged" counter. A pair's
/// *connected fraction* is therefore always `draws.len() / resamples`: a
/// pair that splits in 40% of resamples must show a 60% fraction, not look
/// "fully connected" because everything that happened to converge also
/// happened to stay together (see `CONNECTED_FRACTION_THRESHOLD`'s docs for
/// why that distinction matters). Pairs below the threshold are omitted
/// from the result; the caller renders a missing `(i, j)` key (`i < j`) as
/// `None`.
///
/// `pairs` must already be in the caller's canonical sorted order (the same
/// order `fits` was computed from).
///
/// Every resample is an independent unit of work: a master `StdRng` seeded
/// from `seed` draws `resamples` independent `u64` sub-seeds (cheap,
/// `O(resamples)`, and the only serial step), then each resample seeds its
/// own fresh `StdRng` from its sub-seed and runs entirely on its own -
/// resample, refit, harvest - with no state shared between resamples.
/// `rand_core::SeedableRng::seed_from_u64` expands a `u64` through a PCG32
/// mixing step specifically so related seed values don't produce correlated
/// streams, so drawing sub-seeds from a real PRNG stream (rather than e.g.
/// `seed + resample_index`) needs no separate decorrelation argument - it
/// inherits that guarantee directly. Because every resample is independent,
/// this is fully parallelizable with no batching or ordering constraint:
/// output is a function of `(seed, pairs, resamples)` alone, invariant to
/// thread count or how work is chunked across threads. See
/// `bootstrap_pairwise_elo_diff_cis_serial` (test-only) for how this
/// differs from - and is tested against - the plain sequential version.
///
/// `bootstrap_method` selects how the sorted per-pair draws become an
/// interval - see `finalize_cis` (private). `Bca` additionally runs a one-time
/// (not per-resample) leave-one-game-out jackknife over `pairs` - see
/// `jackknife_replicates` - so only pay that extra cost when requested.
pub fn bootstrap_pairwise_elo_diff_cis(
    n_competitors: usize,
    pairs: &[PairRecord],
    fits: &[CompetitorFit],
    resamples: usize,
    seed: u64,
    confidence: f64,
    bootstrap_method: BootstrapMethod,
) -> HashMap<(usize, usize), (f64, f64)> {
    let num_workers = std::thread::available_parallelism()
        .map(std::num::NonZeroUsize::get)
        .unwrap_or(1);
    bootstrap_pairwise_elo_diff_cis_with_workers(
        n_competitors,
        pairs,
        fits,
        resamples,
        seed,
        confidence,
        bootstrap_method,
        num_workers,
    )
}

/// Same as [`bootstrap_pairwise_elo_diff_cis`], with the worker count taken
/// as an explicit parameter instead of read from the hardware - exists so
/// tests can prove output is invariant to worker count directly (see
/// `output_is_identical_regardless_of_worker_count`), the exact property
/// that makes `--seed` reproducible across machines with different core
/// counts, not just reproducible against itself on one machine.
#[allow(clippy::too_many_arguments)]
fn bootstrap_pairwise_elo_diff_cis_with_workers(
    n_competitors: usize,
    pairs: &[PairRecord],
    fits: &[CompetitorFit],
    resamples: usize,
    seed: u64,
    confidence: f64,
    bootstrap_method: BootstrapMethod,
    num_workers: usize,
) -> HashMap<(usize, usize), (f64, f64)> {
    debug_assert!(
        resamples > 0,
        "bootstrap_pairwise_elo_diff_cis called with zero resamples"
    );

    let component_of: Vec<usize> = fits.iter().map(|f| f.component).collect();

    // Cheap and serial: independent per-resample sub-seeds, not the
    // resamples themselves - O(resamples), not O(resamples * games).
    let mut master_rng = StdRng::seed_from_u64(seed);
    let sub_seeds: Vec<u64> = (0..resamples).map(|_| master_rng.random()).collect();

    let empty_template = init_draws_map(n_competitors, &component_of);
    let num_workers = num_workers.min(resamples).max(1);
    let chunk_size = resamples.div_ceil(num_workers).max(1);

    // Each worker generates its own resamples lazily from its slice of
    // sub-seeds (no upfront materialization needed - nothing to batch for
    // memory, unlike a design that has to generate serially ahead of time)
    // and fits/harvests them into its own local accumulator - no shared
    // mutable state, so no Mutex/Arc needed under `thread::scope`.
    let local_maps: Vec<HashMap<(usize, usize), Vec<f64>>> = std::thread::scope(|scope| {
        sub_seeds
            .chunks(chunk_size)
            .map(|chunk| {
                let template = &empty_template;
                scope.spawn(move || {
                    let mut local = template.clone();
                    for &sub_seed in chunk {
                        let mut rng = StdRng::seed_from_u64(sub_seed);
                        let resampled = resample_one(pairs, &mut rng);
                        if let Ok(resampled_fits) = fit_graph(n_competitors, &resampled) {
                            harvest(&resampled_fits, &mut local);
                        }
                    }
                    local
                })
            })
            .collect::<Vec<_>>()
            .into_iter()
            .map(|h| h.join().expect("bootstrap worker thread panicked"))
            .collect()
    });

    let mut draws = empty_template;
    for local in local_maps {
        for (key, mut vals) in local {
            draws.entry(key).or_default().append(&mut vals);
        }
    }

    // Only pay for the jackknife when BCa actually needs it - it's cheap
    // relative to the resampling loop, but not free, and Percentile/Basic
    // have no use for it at all.
    let jackknife = matches!(bootstrap_method, BootstrapMethod::Bca)
        .then(|| jackknife_replicates(n_competitors, pairs, &component_of));

    finalize_cis(
        draws,
        resamples,
        confidence,
        fits,
        bootstrap_method,
        jackknife.as_ref(),
    )
}

/// Plain single-threaded reference implementation of the same bootstrap
/// procedure (one shared `StdRng`, drawn from sequentially), kept as a
/// **statistical reference** for [`bootstrap_pairwise_elo_diff_cis`] - not
/// a bit-identical oracle (the parallel version's independent per-resample
/// RNG streams make that comparison meaningless; there is no "the same
/// draws in a different order" relationship between them). What the two
/// *do* share is every downstream step (`resample_one`, `fit_graph`,
/// `harvest`, `finalize_cis`), so at a high resample count both converge to
/// the same true percentile, differing only by `O(1/sqrt(resamples))` Monte
/// Carlo noise - unless the parallel version's per-resample seeding is
/// somehow biased, in which case the gap widens beyond that. See the
/// `..._matches_serial_within_tolerance` test for the calibrated bound.
/// Test-only: no non-test code should ever need this over the parallel
/// version.
#[cfg(test)]
fn bootstrap_pairwise_elo_diff_cis_serial(
    n_competitors: usize,
    pairs: &[PairRecord],
    fits: &[CompetitorFit],
    resamples: usize,
    seed: u64,
    confidence: f64,
) -> HashMap<(usize, usize), (f64, f64)> {
    debug_assert!(
        resamples > 0,
        "bootstrap_pairwise_elo_diff_cis_serial called with zero resamples"
    );

    let component_of: Vec<usize> = fits.iter().map(|f| f.component).collect();
    let mut draws = init_draws_map(n_competitors, &component_of);
    let mut rng = StdRng::seed_from_u64(seed);
    for _ in 0..resamples {
        let resampled = resample_one(pairs, &mut rng);
        if let Ok(resampled_fits) = fit_graph(n_competitors, &resampled) {
            harvest(&resampled_fits, &mut draws);
        }
    }

    // This oracle exists to validate the percentile resampling procedure
    // itself (see the doc comment above) - Basic/BCa build on the same
    // draws via a deterministic, already-separately-tested post-processing
    // step, so there's nothing extra for this reference to check there.
    finalize_cis(
        draws,
        resamples,
        confidence,
        fits,
        BootstrapMethod::Percentile,
        None,
    )
}

fn init_draws_map(
    n_competitors: usize,
    original_component_of: &[usize],
) -> HashMap<(usize, usize), Vec<f64>> {
    let mut draws = HashMap::new();
    for i in 0..n_competitors {
        for j in (i + 1)..n_competitors {
            if original_component_of[i] == original_component_of[j] {
                draws.insert((i, j), Vec::new());
            }
        }
    }
    draws
}

fn resample_one(pairs: &[PairRecord], rng: &mut StdRng) -> Vec<PairRecord> {
    pairs
        .iter()
        .map(|p| {
            let (a_wins, b_wins, draw_count) =
                resample_edge_multinomial(p.a_wins, p.b_wins, p.draws, rng);
            PairRecord {
                a: p.a,
                b: p.b,
                a_wins,
                b_wins,
                draws: draw_count,
            }
        })
        .collect()
}

fn harvest(fits: &[CompetitorFit], draws: &mut HashMap<(usize, usize), Vec<f64>>) {
    for (&(i, j), pair_draws) in draws.iter_mut() {
        if fits[i].component == fits[j].component {
            pair_draws.push(fits[i].elo - fits[j].elo);
        }
    }
}

/// One-time (not per-resample) leave-one-game-out jackknife over `pairs`,
/// for BCa's acceleration term. The actual bootstrap resamples at the
/// individual-game level (`resample_edge_multinomial` draws each of an
/// edge's `n` games independently), so the textbook jackknife would drop
/// one game at a time - but `PairRecord` only stores aggregate per-category
/// counts, and games within a category are exchangeable (dropping any one
/// of an edge's `a_wins` games gives an identical resulting tally). So the
/// true per-game jackknife collapses to at most 3 distinct replicates per
/// edge (drop-one-a-win, drop-one-b-win, drop-one-draw), each weighted by
/// that category's count - `weighted_acceleration` is built to consume
/// exactly this (value, weight) shape. This is *not* the same thing as
/// dropping a whole edge: an edge can carry thousands of games (see
/// `near_convergence_boundary_pairs`), and deleting all of them at once
/// would be a massive, wrong-unit perturbation, not a one-observation
/// influence estimate.
///
/// One refit produces a replicate for every still-eligible pair at once
/// (same sharing pattern `harvest` uses for real resamples), so total cost
/// is `O(nonzero_categories * fit_graph(n_competitors))` - independent of
/// `resamples` and of how many pairs need a CI. A category whose decrement
/// empties an edge entirely removes that edge from the perturbed graph
/// (fit_graph requires every remaining pair to have at least one game). A
/// perturbed fit that errors, or that leaves a particular pair
/// disconnected, simply contributes no replicate for that pair - the same
/// "doesn't apply here" omission `harvest` already uses for real resamples,
/// applied per pair rather than globally.
/// Per-pair jackknife replicates for BCa's acceleration term: each `(value,
/// weight)` is one perturbed-graph `elo_i - elo_j` and the count of games
/// that perturbation represents (see `jackknife_replicates`'s doc).
type JackknifeReplicates = HashMap<(usize, usize), Vec<(f64, f64)>>;

fn jackknife_replicates(
    n_competitors: usize,
    pairs: &[PairRecord],
    original_component_of: &[usize],
) -> JackknifeReplicates {
    let mut replicates: JackknifeReplicates = HashMap::new();
    for i in 0..n_competitors {
        for j in (i + 1)..n_competitors {
            if original_component_of[i] == original_component_of[j] {
                replicates.insert((i, j), Vec::new());
            }
        }
    }

    for edge_idx in 0..pairs.len() {
        let edge = pairs[edge_idx];
        for (count, apply_decrement) in [
            (
                edge.a_wins,
                (|e: &mut PairRecord| e.a_wins -= 1) as fn(&mut PairRecord),
            ),
            (
                edge.b_wins,
                (|e: &mut PairRecord| e.b_wins -= 1) as fn(&mut PairRecord),
            ),
            (
                edge.draws,
                (|e: &mut PairRecord| e.draws -= 1) as fn(&mut PairRecord),
            ),
        ] {
            if count == 0 {
                continue;
            }
            let mut perturbed_edge = edge;
            apply_decrement(&mut perturbed_edge);

            let mut perturbed: Vec<PairRecord> = pairs.to_vec();
            if perturbed_edge.a_wins + perturbed_edge.b_wins + perturbed_edge.draws == 0 {
                // That was the edge's only game - it no longer exists, not
                // a zero-game edge (fit_graph requires every pair it's
                // given to have at least one game).
                perturbed.remove(edge_idx);
            } else {
                perturbed[edge_idx] = perturbed_edge;
            }

            let weight = count as f64;
            if let Ok(fits) = fit_graph(n_competitors, &perturbed) {
                for (&(i, j), reps) in replicates.iter_mut() {
                    if fits[i].component == fits[j].component {
                        reps.push((fits[i].elo - fits[j].elo, weight));
                    }
                }
            }
        }
    }

    replicates
}

fn finalize_cis(
    draws: HashMap<(usize, usize), Vec<f64>>,
    resamples: usize,
    confidence: f64,
    fits: &[CompetitorFit],
    bootstrap_method: BootstrapMethod,
    jackknife: Option<&JackknifeReplicates>,
) -> HashMap<(usize, usize), (f64, f64)> {
    let alpha = 1.0 - confidence;
    let mut result = HashMap::new();
    for ((i, j), mut pair_draws) in draws {
        let fraction = pair_draws.len() as f64 / resamples as f64;
        if fraction < CONNECTED_FRACTION_THRESHOLD {
            continue;
        }
        pair_draws.sort_by(f64::total_cmp);
        let n = pair_draws.len();
        let original = fits[i].elo - fits[j].elo;

        // Basic reads the same plain percentiles as Percentile, then
        // reflects the result around the original estimate below - only
        // Bca reads different (bias/acceleration-adjusted) percentiles.
        let (p_lo, p_hi) = match bootstrap_method {
            BootstrapMethod::Percentile | BootstrapMethod::Basic => {
                (alpha / 2.0, 1.0 - alpha / 2.0)
            }
            // One-time check at implementation time (10-competitor
            // round-robin, 15-45 games/edge, varying skew): mean endpoint
            // gap vs. Percentile was ~1.8 Elo, max ~5 Elo - a real but
            // modest correction at these realistic sample sizes, not a
            // dramatic one. BCa is still worth having (it's the textbook
            // answer to percentile's known bias/skew sensitivity, and the
            // gap grows with skew/smaller samples), but don't expect a
            // large practical difference on well-sampled, moderately-skewed
            // data - most of the value shows up on small or heavily skewed
            // edges (see `bca_differs_from_percentile_on_a_skewed_graph`).
            BootstrapMethod::Bca => {
                let below = pair_draws.iter().filter(|&&v| v < original).count() as f64;
                let z0 = bias_correction_z0(below, n);
                let empty = Vec::new();
                let pair_replicates = jackknife
                    .and_then(|reps| reps.get(&(i, j)))
                    .unwrap_or(&empty);
                let a = weighted_acceleration(pair_replicates);
                bca_adjusted_percentiles(z0, a, confidence)
            }
        };

        let lo_idx = lo_index(p_lo, n);
        let hi_idx = hi_index(p_hi, n).max(lo_idx);
        let (lo, hi) = (pair_draws[lo_idx], pair_draws[hi_idx]);

        let bounds = match bootstrap_method {
            BootstrapMethod::Basic => (2.0 * original - hi, 2.0 * original - lo),
            BootstrapMethod::Percentile | BootstrapMethod::Bca => (lo, hi),
        };
        result.insert((i, j), bounds);
    }
    result
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
    fn multi_node_cycle_converges_to_expected_ratings() {
        // A beats B 4-1, B beats C 3-2, C beats A 1-4: a genuine cycle, no
        // one dominates or is dominated. Independently verified in Python
        // (200,000-iteration reference solve, tol 1e-14):
        // Elo(B)-Elo(A) = -215.5356238480202, Elo(C)-Elo(A) = -268.53764941971946.
        let pairs = [
            PairRecord {
                a: 0,
                b: 1,
                a_wins: 4,
                b_wins: 1,
                draws: 0,
            },
            PairRecord {
                a: 1,
                b: 2,
                a_wins: 3,
                b_wins: 2,
                draws: 0,
            },
            PairRecord {
                a: 2,
                b: 0,
                a_wins: 1,
                b_wins: 4,
                draws: 0,
            },
        ];
        let fits = fit_graph(3, &pairs).unwrap();
        assert_eq!(fits[0].component, fits[1].component);
        assert_eq!(fits[1].component, fits[2].component);
        assert_close(fits[0].elo, 0.0, 1e-9);
        assert_close(fits[1].elo - fits[0].elo, -215.5356238480202, 1e-6);
        assert_close(fits[2].elo - fits[0].elo, -268.53764941971946, 1e-6);
    }

    #[test]
    fn undirected_connected_but_not_strongly_connected_is_detected() {
        // A=0,B=1,C=2,D=3. {A,B} beats {C,D} every cross-game despite mutual
        // wins within each pair (A<->B, C<->D) - undirected-connected (every
        // node has both a win and a loss overall), but NOT strongly
        // connected: no edge leads from {C,D} back to {A,B}.
        let pairs = [
            PairRecord {
                a: 0,
                b: 1,
                a_wins: 1,
                b_wins: 1,
                draws: 0,
            }, // A vs B
            PairRecord {
                a: 2,
                b: 3,
                a_wins: 1,
                b_wins: 1,
                draws: 0,
            }, // C vs D
            PairRecord {
                a: 0,
                b: 2,
                a_wins: 1,
                b_wins: 0,
                draws: 0,
            }, // A beat C
            PairRecord {
                a: 0,
                b: 3,
                a_wins: 1,
                b_wins: 0,
                draws: 0,
            }, // A beat D
            PairRecord {
                a: 1,
                b: 2,
                a_wins: 1,
                b_wins: 0,
                draws: 0,
            }, // B beat C
            PairRecord {
                a: 1,
                b: 3,
                a_wins: 1,
                b_wins: 0,
                draws: 0,
            }, // B beat D
        ];
        let connectivity = strongly_connected_components(4, &pairs);
        assert_eq!(connectivity.num_components, 2);
        assert_eq!(connectivity.component_of[0], connectivity.component_of[1]);
        assert_eq!(connectivity.component_of[2], connectivity.component_of[3]);
        assert_ne!(connectivity.component_of[0], connectivity.component_of[2]);
    }

    #[test]
    fn all_draws_is_not_flagged_disconnected() {
        // A draw must create edges in BOTH directions - otherwise this
        // legitimate, fully-connected all-draws graph would wrongly be
        // flagged as disconnected.
        let pairs = [
            PairRecord {
                a: 0,
                b: 1,
                a_wins: 0,
                b_wins: 0,
                draws: 3,
            },
            PairRecord {
                a: 1,
                b: 2,
                a_wins: 0,
                b_wins: 0,
                draws: 3,
            },
        ];
        let connectivity = strongly_connected_components(3, &pairs);
        assert_eq!(connectivity.num_components, 1);

        let fits = fit_graph(3, &pairs).unwrap();
        for f in &fits {
            assert_close(f.elo, 0.0, 1e-6);
        }
    }

    #[test]
    fn disconnected_input_does_not_iterate_to_a_wrong_answer() {
        // Two separate 2-node components, each individually well-posed
        // (mutual wins on both sides). fit_graph must return real,
        // finite, per-component-correct ratings for both - not an error
        // and not a single shared scale.
        let pairs = [
            PairRecord {
                a: 0,
                b: 1,
                a_wins: 3,
                b_wins: 1,
                draws: 0,
            },
            PairRecord {
                a: 2,
                b: 3,
                a_wins: 1,
                b_wins: 3,
                draws: 0,
            },
        ];
        let fits = fit_graph(4, &pairs).unwrap();
        assert_eq!(fits[0].component, fits[1].component);
        assert_eq!(fits[2].component, fits[3].component);
        assert_ne!(fits[0].component, fits[2].component);
        // Within {0,1}: 0 beat 1 three times out of four -> 0 is stronger.
        assert!(fits[1].elo < fits[0].elo);
        // Within {2,3}: 3 beat 2 three times out of four -> 3 is stronger.
        assert!(fits[3].elo > fits[2].elo);
    }

    #[test]
    fn singleton_competitor_with_no_games_gets_pinned_elo() {
        let pairs = [PairRecord {
            a: 0,
            b: 1,
            a_wins: 2,
            b_wins: 2,
            draws: 0,
        }];
        let fits = fit_graph(3, &pairs).unwrap();
        assert_ne!(fits[2].component, fits[0].component);
        assert_close(fits[2].elo, 0.0, 1e-9);
    }

    #[test]
    fn extreme_lopsided_but_connected_graph_errors_instead_of_hanging() {
        // {A,B} tied a million-to-a-million, {C,D} likewise, bridged by a
        // single A-vs-C tie where A won 10^15 times and C won back exactly
        // once - both directions present, so strongly connected (Ford's
        // condition holds, a finite MLE exists), but so lopsided the MM
        // iteration hasn't settled below the threshold within the cap
        // (independently confirmed in Python: still not converged even
        // after 2,000,000 iterations). This must error, not silently
        // return a barely-moving, still-drifting answer.
        let pairs = [
            PairRecord {
                a: 0,
                b: 1,
                a_wins: 1_000_000,
                b_wins: 1_000_000,
                draws: 0,
            },
            PairRecord {
                a: 2,
                b: 3,
                a_wins: 1_000_000,
                b_wins: 1_000_000,
                draws: 0,
            },
            PairRecord {
                a: 0,
                b: 2,
                a_wins: 1_000_000_000_000_000,
                b_wins: 1,
                draws: 0,
            },
        ];
        match fit_graph(4, &pairs) {
            Err(VeridictError::BradleyTerryDidNotConverge { iterations, .. }) => {
                assert_eq!(iterations, MAX_ITERATIONS);
            }
            other => panic!("expected BradleyTerryDidNotConverge, got {other:?}"),
        }
    }

    #[test]
    fn two_node_shutout_is_two_singleton_components() {
        // No draws, all wins one direction: not strongly connected, so this
        // is the exact scenario matrix.rs's integration layer must route
        // around via the star-graph closed form instead of calling this
        // solver directly - documented here so the boundary is explicit.
        let pairs = [PairRecord {
            a: 0,
            b: 1,
            a_wins: 5,
            b_wins: 0,
            draws: 0,
        }];
        let connectivity = strongly_connected_components(2, &pairs);
        assert_eq!(connectivity.num_components, 2);
    }

    // --- bootstrap_pairwise_elo_diff_cis ---

    // Two robust clusters {0,1} and {2,3} (each tied 20-20, so resampling
    // essentially never flips either internal edge's bidirectionality)
    // joined by a single fragile bridge (1-2, tied 1-1): resampling that
    // bridge has a 50% chance of collapsing to one-sided (binomial(2, 0.5)
    // landing on 0 or 2), which splits the whole graph into two
    // components. This is the concrete "conditioning on connectivity would
    // be optimistic" scenario the threshold exists for.
    fn fragile_bridge_pairs() -> [PairRecord; 3] {
        [
            PairRecord {
                a: 0,
                b: 1,
                a_wins: 20,
                b_wins: 20,
                draws: 0,
            },
            PairRecord {
                a: 2,
                b: 3,
                a_wins: 20,
                b_wins: 20,
                draws: 0,
            },
            PairRecord {
                a: 1,
                b: 2,
                a_wins: 1,
                b_wins: 1,
                draws: 0,
            },
        ]
    }

    #[test]
    fn fragile_bridge_cross_pairs_degrade_to_no_ci_but_same_side_pairs_keep_one() {
        let pairs = fragile_bridge_pairs();
        let fits = fit_graph(4, &pairs).unwrap();
        assert!(
            fits.iter().all(|f| f.component == fits[0].component),
            "the original (unresampled) graph must be a single component"
        );

        let cis = bootstrap_pairwise_elo_diff_cis(
            4,
            &pairs,
            &fits,
            2000,
            42,
            0.95,
            BootstrapMethod::Percentile,
        );

        // Cross-bridge pairs: connected in only ~50% of resamples, far below
        // the 90% threshold - must be omitted (renders as `None`).
        for cross in [(0, 2), (0, 3), (1, 2), (1, 3)] {
            assert!(
                !cis.contains_key(&cross),
                "{cross:?} should have no reliable CI (fragile bridge)"
            );
        }
        // Same-side pairs: always in the same component regardless of what
        // the bridge does - must get a real CI.
        for same_side in [(0, 1), (2, 3)] {
            assert!(
                cis.contains_key(&same_side),
                "{same_side:?} should get a real CI (robust internal edge)"
            );
            let (lo, hi) = cis[&same_side];
            assert!(lo.is_finite() && hi.is_finite() && lo <= hi);
        }
    }

    #[test]
    fn bootstrap_cis_are_deterministic_for_a_fixed_seed() {
        let pairs = fragile_bridge_pairs();
        let fits = fit_graph(4, &pairs).unwrap();

        let cis_a = bootstrap_pairwise_elo_diff_cis(
            4,
            &pairs,
            &fits,
            500,
            7,
            0.95,
            BootstrapMethod::Percentile,
        );
        let cis_b = bootstrap_pairwise_elo_diff_cis(
            4,
            &pairs,
            &fits,
            500,
            7,
            0.95,
            BootstrapMethod::Percentile,
        );
        assert_eq!(cis_a, cis_b);
    }

    #[test]
    fn output_is_identical_regardless_of_worker_count() {
        // The actual reproducibility guarantee isn't just "same output when
        // I call it twice on my machine" (which a worker-count-dependent
        // bug would still pass) - it's "same `--seed` gives the same output
        // on a 1-core box and a 64-core box." Sub-seeds are generated
        // before the chunk split and merged order-independently, so this
        // should hold exactly, not just approximately.
        let pairs = dense_pairs_for_test(6, 20);
        let fits = fit_graph(6, &pairs).unwrap();

        let mut results = [1usize, 2, 3, 7, 64].map(|num_workers| {
            bootstrap_pairwise_elo_diff_cis_with_workers(
                6,
                &pairs,
                &fits,
                200,
                42,
                0.95,
                BootstrapMethod::Percentile,
                num_workers,
            )
        });
        let first = results[0].clone();
        for other in results.iter_mut().skip(1) {
            assert_eq!(
                &first, other,
                "bootstrap output must be identical regardless of worker count"
            );
        }
    }

    #[test]
    fn different_seeds_can_change_the_ci() {
        let pairs = fragile_bridge_pairs();
        let fits = fit_graph(4, &pairs).unwrap();

        let cis_a = bootstrap_pairwise_elo_diff_cis(
            4,
            &pairs,
            &fits,
            500,
            1,
            0.95,
            BootstrapMethod::Percentile,
        );
        let cis_b = bootstrap_pairwise_elo_diff_cis(
            4,
            &pairs,
            &fits,
            500,
            2,
            0.95,
            BootstrapMethod::Percentile,
        );
        assert_ne!(cis_a, cis_b);
    }

    #[test]
    fn disconnected_pairs_never_get_a_ci() {
        // Two clusters with no bridge at all: genuinely disconnected from
        // the start, not merely fragile.
        let pairs = [
            PairRecord {
                a: 0,
                b: 1,
                a_wins: 5,
                b_wins: 5,
                draws: 0,
            },
            PairRecord {
                a: 2,
                b: 3,
                a_wins: 5,
                b_wins: 5,
                draws: 0,
            },
        ];
        let fits = fit_graph(4, &pairs).unwrap();
        let component_of: Vec<usize> = fits.iter().map(|f| f.component).collect();
        assert_ne!(component_of[0], component_of[2]);

        let cis = bootstrap_pairwise_elo_diff_cis(
            4,
            &pairs,
            &fits,
            200,
            42,
            0.95,
            BootstrapMethod::Percentile,
        );
        for cross in [(0, 2), (0, 3), (1, 2), (1, 3)] {
            assert!(!cis.contains_key(&cross));
        }
    }

    // A resample near the edge of convergence should never propagate as a
    // panic or an error out of the bootstrap loop - it should simply fail
    // to extend that resample's contribution, exactly like a disconnected
    // resample. `(tie=100, a_wins=10_000, b_wins=1)` converges for the real
    // (unresampled) data - verified independently in Python at 3062
    // iterations - but sits close enough to the convergence boundary that
    // this is a meaningful stress test of the catch-and-skip path even
    // though it can't deterministically force a non-convergent resample
    // (the multinomial resampler can never make a single edge's ratio more
    // extreme than its own observed total, so guaranteeing non-convergence
    // by construction isn't possible - this asserts robustness, not that
    // the Err branch is definitely hit on every run).
    fn near_convergence_boundary_pairs() -> [PairRecord; 3] {
        [
            PairRecord {
                a: 0,
                b: 1,
                a_wins: 100,
                b_wins: 100,
                draws: 0,
            },
            PairRecord {
                a: 2,
                b: 3,
                a_wins: 100,
                b_wins: 100,
                draws: 0,
            },
            PairRecord {
                a: 0,
                b: 2,
                a_wins: 10_000,
                b_wins: 1,
                draws: 0,
            },
        ]
    }

    #[test]
    fn resample_near_the_convergence_boundary_does_not_panic_or_propagate_an_error() {
        let pairs = near_convergence_boundary_pairs();
        let fits = fit_graph(4, &pairs).expect("the real, unresampled data converges");

        // Small resample count: this edge's `n` (10,101) makes each
        // resample O(n), so keep the total bounded for a fast test.
        let cis = bootstrap_pairwise_elo_diff_cis(
            4,
            &pairs,
            &fits,
            50,
            42,
            0.95,
            BootstrapMethod::Percentile,
        );
        // No panic, no hang - reaching this line is the actual assertion.
        // Whatever comes back (a real CI, or None because too many
        // resamples disconnected/failed to converge) is valid.
        for (_, (lo, hi)) in cis {
            assert!(lo.is_finite() && hi.is_finite());
        }
    }

    /// Dense round-robin fixture: every pair of `n` nodes plays
    /// `games_per_pair` games, split 3/5-2/5 so every edge is genuinely
    /// bidirectional (never a degenerate shutout) and resampling essentially
    /// never flips connectivity - used where a test needs the bootstrap
    /// procedure itself to be well-conditioned, not the object under test.
    fn dense_pairs_for_test(n: usize, games_per_pair: u64) -> Vec<PairRecord> {
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

    // --- parallel vs. serial: statistical closeness, not bit-identity ---
    //
    // The parallel version's independent per-resample RNG streams mean it
    // no longer produces the same draws as the sequential oracle - a plain
    // `==` check would be meaningless. What both implementations *do* share
    // is every downstream step (`resample_one`/`fit_graph`/`harvest`/
    // `finalize_cis`), so at a high resample count they should converge to
    // the same true percentile, differing only by Monte Carlo noise -
    // unless the parallel version's per-resample seeding is biased, which
    // would widen the gap well beyond that noise floor. Tolerance below is
    // calibrated empirically (same approach as the BCa fixture's tolerance
    // in an earlier sprint): observed gap at 8,000 resamples on this
    // well-conditioned graph was well under 5 Elo, so 25 leaves a wide
    // margin above sampling noise while still catching a genuinely biased
    // seeding scheme (which would show up as a much larger, systematic gap).
    #[test]
    fn parallel_matches_serial_within_tolerance_at_high_resamples() {
        const TOLERANCE_ELO: f64 = 25.0;
        let pairs = dense_pairs_for_test(3, 100);
        let fits = fit_graph(3, &pairs).unwrap();

        let parallel = bootstrap_pairwise_elo_diff_cis(
            3,
            &pairs,
            &fits,
            8000,
            11,
            0.95,
            BootstrapMethod::Percentile,
        );
        let serial = bootstrap_pairwise_elo_diff_cis_serial(3, &pairs, &fits, 8000, 13, 0.95);

        assert_eq!(
            parallel.len(),
            serial.len(),
            "both implementations should find the same pairs reliably connected \
             on this well-conditioned graph"
        );
        for (key, (par_lo, par_hi)) in &parallel {
            let (ser_lo, ser_hi) = serial[key];
            assert!(
                (par_lo - ser_lo).abs() < TOLERANCE_ELO && (par_hi - ser_hi).abs() < TOLERANCE_ELO,
                "pair {key:?}: parallel ({par_lo}, {par_hi}) vs serial ({ser_lo}, {ser_hi}) \
                 differ by more than {TOLERANCE_ELO} Elo - possible biased sub-seeding"
            );
        }
    }

    #[test]
    fn per_resample_sub_seeds_are_unique() {
        // Cheap smoke check: with a 64-bit seed space and 10,000 draws,
        // collisions should never happen in practice - a duplicate here
        // would indicate a broken seeding scheme (e.g. accidentally
        // reseeding the same value), not bad luck.
        let mut rng = StdRng::seed_from_u64(0x5EED);
        let sub_seeds: std::collections::HashSet<u64> = (0..10_000).map(|_| rng.random()).collect();
        assert_eq!(sub_seeds.len(), 10_000, "expected 10,000 unique sub-seeds");
    }

    // --- small resample counts ---

    #[test]
    fn resamples_smaller_than_available_parallelism_does_not_panic() {
        let pairs = fragile_bridge_pairs();
        let fits = fit_graph(4, &pairs).unwrap();
        for resamples in [1usize, 10] {
            let cis = bootstrap_pairwise_elo_diff_cis(
                4,
                &pairs,
                &fits,
                resamples,
                42,
                0.95,
                BootstrapMethod::Percentile,
            );
            for (_, (lo, hi)) in cis {
                assert!(lo.is_finite() && hi.is_finite() && lo <= hi);
            }
        }
    }

    // --- Basic bootstrap method ---

    // Core correctness guard, not tautological despite calling the same
    // formula the implementation uses: pins the exact algebraic relation
    // between Basic and Percentile (real bug class it catches: swapped
    // lo/hi, wrong sign, or reflecting around a resample statistic instead
    // of the real point estimate). Both calls share the same seed, so the
    // underlying sorted draws are bit-identical - the reflection identity
    // holds to exactly 0.0, `assert_eq!`, not a tolerance.
    #[test]
    fn basic_reflects_percentile_exactly() {
        let pairs = dense_pairs_for_test(4, 40);
        let fits = fit_graph(4, &pairs).unwrap();
        let percentile = bootstrap_pairwise_elo_diff_cis(
            4,
            &pairs,
            &fits,
            1000,
            7,
            0.95,
            BootstrapMethod::Percentile,
        );
        let basic = bootstrap_pairwise_elo_diff_cis(
            4,
            &pairs,
            &fits,
            1000,
            7,
            0.95,
            BootstrapMethod::Basic,
        );

        assert_eq!(percentile.len(), basic.len());
        for (key, (perc_lo, perc_hi)) in &percentile {
            let original = fits[key.0].elo - fits[key.1].elo;
            let (basic_lo, basic_hi) = basic[key];
            assert_eq!(basic_lo, 2.0 * original - perc_hi);
            assert_eq!(basic_hi, 2.0 * original - perc_lo);
        }
    }

    #[test]
    fn basic_ci_is_well_formed() {
        let pairs = dense_pairs_for_test(5, 30);
        let fits = fit_graph(5, &pairs).unwrap();
        let cis = bootstrap_pairwise_elo_diff_cis(
            5,
            &pairs,
            &fits,
            1000,
            42,
            0.95,
            BootstrapMethod::Basic,
        );
        assert!(!cis.is_empty());
        for (_, (lo, hi)) in cis {
            assert!(lo.is_finite() && hi.is_finite() && lo <= hi);
        }
    }

    // --- BCa bootstrap method ---

    #[test]
    fn bca_ci_is_well_formed() {
        let pairs = dense_pairs_for_test(5, 30);
        let fits = fit_graph(5, &pairs).unwrap();
        let cis =
            bootstrap_pairwise_elo_diff_cis(5, &pairs, &fits, 1000, 42, 0.95, BootstrapMethod::Bca);
        assert!(!cis.is_empty());
        for (_, (lo, hi)) in cis {
            assert!(lo.is_finite() && hi.is_finite() && lo <= hi);
        }
    }

    // Not an exact-reduction check (see the equivalent caveat on
    // `bootstrap::bca_close_to_percentile_bounds_on_roughly_symmetric_data`)
    // - a genuinely balanced graph's bootstrap distribution is only
    // approximately symmetric at a finite resample count. This just checks
    // BCa stays in the same neighborhood as Percentile for well-behaved,
    // unskewed data, catching a wildly wrong (not just imperceptibly
    // different) bias/acceleration computation.
    #[test]
    fn bca_close_to_percentile_on_a_balanced_graph() {
        let pairs = dense_pairs_for_test(3, 100);
        let fits = fit_graph(3, &pairs).unwrap();
        let percentile = bootstrap_pairwise_elo_diff_cis(
            3,
            &pairs,
            &fits,
            20_000,
            42,
            0.95,
            BootstrapMethod::Percentile,
        );
        let bca = bootstrap_pairwise_elo_diff_cis(
            3,
            &pairs,
            &fits,
            20_000,
            42,
            0.95,
            BootstrapMethod::Bca,
        );
        // Tolerance calibrated empirically (observed max gap here ~4.9 Elo)
        // - comfortably above that noise floor, far below the tens-to-
        // hundreds of Elo a genuinely wrong bias/acceleration term would
        // produce.
        for (key, (perc_lo, perc_hi)) in &percentile {
            let (bca_lo, bca_hi) = bca[key];
            assert_close(bca_lo, *perc_lo, 15.0);
            assert_close(bca_hi, *perc_hi, 15.0);
        }
    }

    #[test]
    fn bca_differs_from_percentile_on_a_skewed_graph() {
        // Lopsided win counts (not the balanced 3/5-2/5 split
        // `dense_pairs_for_test` uses) push each edge's resampled score
        // rate toward a boundary, which is what actually produces skew in
        // the bootstrap distribution of elo_i - elo_j.
        let pairs = [
            PairRecord {
                a: 0,
                b: 1,
                a_wins: 19,
                b_wins: 1,
                draws: 0,
            },
            PairRecord {
                a: 1,
                b: 2,
                a_wins: 19,
                b_wins: 1,
                draws: 0,
            },
            PairRecord {
                a: 2,
                b: 0,
                a_wins: 10,
                b_wins: 10,
                draws: 0,
            },
        ];
        let fits = fit_graph(3, &pairs).unwrap();
        let percentile = bootstrap_pairwise_elo_diff_cis(
            3,
            &pairs,
            &fits,
            10_000,
            42,
            0.95,
            BootstrapMethod::Percentile,
        );
        let bca = bootstrap_pairwise_elo_diff_cis(
            3,
            &pairs,
            &fits,
            10_000,
            42,
            0.95,
            BootstrapMethod::Bca,
        );
        assert_ne!(percentile, bca);
    }

    #[test]
    fn bca_does_not_panic_when_jackknife_replicates_disconnect_the_pair() {
        // The bridge edge has a single game - jackknifing it (dropping that
        // one game) removes the edge entirely, splitting the graph into two
        // components. Every jackknife replicate for a cross-bridge pair is
        // therefore excluded (see `jackknife_replicates`'s doc), leaving
        // that pair with zero valid replicates - the `a = 0` fallback must
        // handle this without panicking (e.g. on an empty-slice reduction).
        let pairs = fragile_bridge_pairs();
        let fits = fit_graph(4, &pairs).unwrap();
        let cis =
            bootstrap_pairwise_elo_diff_cis(4, &pairs, &fits, 2000, 42, 0.95, BootstrapMethod::Bca);
        // Same connectivity behavior as Percentile: same-side pairs get a
        // CI, cross-bridge pairs don't (too fragile under resampling).
        for same_side in [(0, 1), (2, 3)] {
            let (lo, hi) = cis[&same_side];
            assert!(lo.is_finite() && hi.is_finite() && lo <= hi);
        }
        for cross in [(0, 2), (0, 3), (1, 2), (1, 3)] {
            assert!(!cis.contains_key(&cross));
        }
    }

    // --- BCa correctness oracles ---
    //
    // Structural tests (lo <= hi, no panic, "shifts vs percentile") all pass
    // through a sign-flipped or misweighted acceleration term - a wrong `a`
    // still produces a well-formed, shifted interval. Two checks close that
    // gap. (A statistical oracle comparing this graph path against the 1-D
    // scipy-validated reference through the Elo transform was tried first
    // and abandoned: BCa's transformation-invariance is only asymptotic, and
    // binary per-game data collapses to a coarse, discrete bootstrap
    // distribution at any n small enough to keep the acceleration signal
    // visible - both effects produce misleading "mismatches" unrelated to
    // implementation correctness. Deterministic checks avoid both.)

    // `jackknife_replicates` is the novel, risky code this sprint added
    // (category weighting, decrement-and-refit, harvest). `fit_graph` is
    // deterministic and the 2-node case is closed-form, so this checks it
    // exactly - no bootstrap, no seed, no discreteness to fight.
    #[test]
    fn jackknife_replicates_matches_closed_form_on_a_2_node_graph() {
        use crate::stats::elo::elo_from_score;

        let pairs = [PairRecord {
            a: 0,
            b: 1,
            a_wins: 160,
            b_wins: 40,
            draws: 0,
        }];
        let reps = jackknife_replicates(2, &pairs, &[0, 0]);
        let pair = &reps[&(0, 1)];

        // Category order in `jackknife_replicates` is a_wins, b_wins, draws;
        // draws is zero here so exactly 2 replicates, one per nonzero
        // category. Each perturbed edge has 199 total games.
        assert_eq!(pair.len(), 2);
        let (drop_a_win_value, drop_a_win_weight) = pair[0];
        let (drop_b_win_value, drop_b_win_weight) = pair[1];
        assert_close(drop_a_win_weight, 160.0, 1e-9);
        assert_close(drop_b_win_weight, 40.0, 1e-9);
        // Dropping an a-win leaves b_wins=40 of 199 games; dropping a b-win
        // leaves b_wins=39 of 199. Reported as -elo_from_score(b's rate)
        // since key (0, 1) is fits[0].elo - fits[1].elo and node 0 pins at 0.
        assert_close(drop_a_win_value, -elo_from_score(40.0 / 199.0), 1e-6);
        assert_close(drop_b_win_value, -elo_from_score(39.0 / 199.0), 1e-6);
    }

    // Pins the end-to-end BCa output on a skewed 2-node graph at a fixed
    // seed, mirroring `bootstrap.rs`'s `bca_matches_fixture_at_fixed_seed`.
    // Values below were read off a verified-correct run (z0 and `a` checked
    // by hand against the closed-form skewness formula for binary data
    // before pinning). A regression here - e.g. the z0 comparison sign this
    // was mutation-tested against during development - changes these
    // values; a structural-only test would not have caught it.
    #[test]
    fn bca_pairwise_ci_matches_pinned_reference_at_fixed_seed() {
        let pairs = [PairRecord {
            a: 0,
            b: 1,
            a_wins: 160,
            b_wins: 40,
            draws: 0,
        }];
        let fits = fit_graph(2, &pairs).unwrap();
        let cis = bootstrap_pairwise_elo_diff_cis(
            2,
            &pairs,
            &fits,
            20_000,
            42,
            0.95,
            BootstrapMethod::Bca,
        );
        let (lo, hi) = cis[&(0, 1)];
        assert_close(lo, 172.78363838458742, 1e-6);
        assert_close(hi, 294.61000431176035, 1e-6);

        // And it must actually differ from Percentile at this fixture -
        // otherwise the pin above could be passing percentile-only output.
        let percentile_cis = bootstrap_pairwise_elo_diff_cis(
            2,
            &pairs,
            &fits,
            20_000,
            42,
            0.95,
            BootstrapMethod::Percentile,
        );
        assert_ne!(cis[&(0, 1)], percentile_cis[&(0, 1)]);
    }
}
