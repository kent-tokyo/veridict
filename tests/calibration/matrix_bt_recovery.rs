//! Can `bradley_terry::fit_graph` recover a *known* true rating vector from simulated match data?
//! The existing test suite only checks the solver against fixed, hand-picked win/loss counts
//! verified against an independently-computed reference solve
//! (`multi_node_cycle_converges_to_expected_ratings`) - a "does the algorithm compute what a
//! reference implementation computes" check, not "does the algorithm recover the truth from noisy
//! simulated data" - a different and, for a decision gate, more directly relevant property.
//!
//! Bradley-Terry only identifies rating *differences* (any two fits related by shifting every
//! rating by the same constant are equally valid - this project's own pin-invariance is already
//! tested elsewhere), so recovery is checked on `elo_i - elo_j` for pairs sharing a component,
//! not absolute ratings. Recovery error should shrink as games-per-pair grows - checked at three
//! scales to demonstrate genuine convergence, not a single lucky sample.

use rand::rngs::StdRng;
use rand::{RngExt, SeedableRng};
use veridict::stats::bradley_terry::{PairRecord, fit_graph};
use veridict::stats::sprt::score_from_elo;

const SEED: u64 = 0x5EED;

/// Planted true ratings for 6 competitors, spread over 250 Elo - a realistic tournament spread,
/// not an edge case (no ties, no extreme gaps that would make win probabilities near 0/1 and
/// swamp recovery with variance unrelated to sample size).
const TRUE_ELO: [f64; 6] = [0.0, 40.0, 90.0, 130.0, 190.0, 250.0];

/// Simulates a dense round-robin (every pair plays `games_per_pair` games, no draws - a clean,
/// unambiguous true model) from `TRUE_ELO` via the standard logistic win-probability formula
/// (`score_from_elo`, the same formula this project uses for Elo/SPRT/Bradley-Terry alike), fits
/// via `fit_graph`, and returns the fitted ratings.
#[allow(clippy::needless_range_loop)] // i and j index two arrays (TRUE_ELO) and build a pair (a, b) together, not a single-slice walk
fn simulate_and_fit(
    rng: &mut StdRng,
    games_per_pair: u64,
) -> Vec<veridict::stats::bradley_terry::CompetitorFit> {
    let n = TRUE_ELO.len();
    let mut pairs = Vec::new();
    for i in 0..n {
        for j in (i + 1)..n {
            let p_i_beats_j = score_from_elo(TRUE_ELO[i] - TRUE_ELO[j]);
            let a_wins = (0..games_per_pair)
                .filter(|_| rng.random::<f64>() < p_i_beats_j)
                .count() as u64;
            pairs.push(PairRecord {
                a: i,
                b: j,
                a_wins,
                b_wins: games_per_pair - a_wins,
                draws: 0,
            });
        }
    }
    fit_graph(n, &pairs).unwrap()
}

/// Mean absolute error of every fitted pairwise difference against the planted true difference,
/// across every pair (all pairs share one component here - dense round-robin is always strongly
/// connected, per `fit_graph`'s own existence condition).
#[allow(clippy::needless_range_loop)] // i and j index both `fits` and `TRUE_ELO` together, not a single-slice walk
fn mean_abs_diff_error(fits: &[veridict::stats::bradley_terry::CompetitorFit]) -> f64 {
    let n = TRUE_ELO.len();
    let mut total = 0.0;
    let mut count = 0;
    for i in 0..n {
        for j in (i + 1)..n {
            let recovered = fits[i].elo - fits[j].elo;
            let truth = TRUE_ELO[i] - TRUE_ELO[j];
            total += (recovered - truth).abs();
            count += 1;
        }
    }
    total / count as f64
}

// At a small games-per-pair count, recovery is noisy but should still be in the right ballpark
// (not off by an amount comparable to the ratings' own spread, which would mean the solver isn't
// tracking the data at all). Mutation-tested: undercounting one side's win tally by 30% in
// `fit_component`'s MM update (`bradley_terry.rs`) pushed this to 76.6, caught by the 60.0 floor.
//
// Observed once at games_per_pair=20, SEED=0x5EED: mean absolute error was 37.7 Elo - noisy, as
// expected at this sample size, but nowhere near the ~100 Elo scale of TRUE_ELO's own spread.
#[test]
fn recovery_is_in_the_right_ballpark_at_small_sample() {
    let mut rng = StdRng::seed_from_u64(SEED);
    let fits = simulate_and_fit(&mut rng, 20);
    let error = mean_abs_diff_error(&fits);
    assert!(
        error < 60.0,
        "mean absolute recovery error {error:.1} at games_per_pair=20 is too large relative to \
         TRUE_ELO's spread - the solver isn't tracking the simulated data"
    );
}

// The convergence property: recovery error should shrink substantially as games-per-pair grows,
// demonstrating the solver is actually consistent (converges to the truth with more data), not
// just "close enough" at one arbitrarily chosen sample size.
//
// The same mutation that the small-sample test above catches (30% win-undercount bias) does
// *not* fail the two `error_N < error_M` orderings below on its own: a biased-but-decreasing
// estimator can still shrink monotonically toward its wrong asymptote (measured: mutated
// error_20/200/2000 = 76.6/68.6/61.5 - still strictly decreasing). The explicit
// `error_2000 < 10.0` floor closes that gap by checking convergence lands near the *truth*, not
// just that variance shrinks - the same mutation pushes error_2000 to 61.5, comfortably above
// this floor, while the true baseline sits at 2.7.
//
// Observed once, SEED=0x5EED: mean absolute error was 37.7 at games_per_pair=20, 12.2 at
// games_per_pair=200, 2.7 at games_per_pair=2,000 - a clear, monotone shrink across two full
// orders of magnitude in sample size, roughly consistent with the expected `O(1/sqrt(games))`
// scaling of a maximum-likelihood estimator (a 10x games increase should shrink error by about
// sqrt(10) ≈ 3.2x; observed ratios were 3.1x then 4.5x - in the right regime, not exact, as
// expected from a single seed rather than an average over many).
#[test]
fn recovery_error_shrinks_as_games_per_pair_grows() {
    let mut rng = StdRng::seed_from_u64(SEED);
    let error_20 = mean_abs_diff_error(&simulate_and_fit(&mut rng, 20));
    let error_200 = mean_abs_diff_error(&simulate_and_fit(&mut rng, 200));
    let error_2000 = mean_abs_diff_error(&simulate_and_fit(&mut rng, 2_000));

    assert!(
        error_200 < error_20,
        "recovery error {error_200:.1} at games_per_pair=200 was not smaller than {error_20:.1} \
         at games_per_pair=20 - the solver should converge with more data, not stay flat or worsen"
    );
    assert!(
        error_2000 < error_200,
        "recovery error {error_2000:.1} at games_per_pair=2,000 was not smaller than \
         {error_200:.1} at games_per_pair=200"
    );
    assert!(
        error_2000 < 10.0,
        "recovery error {error_2000:.1} at games_per_pair=2,000 did not converge near the truth \
         - a monotonically-shrinking-but-still-biased estimator would pass the two orderings \
         above without this explicit absolute check"
    );
}
