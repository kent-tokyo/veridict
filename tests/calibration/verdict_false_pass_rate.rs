//! The capstone check: does the full `winrate`-style pipeline (`wilson_ci` + `verdict::decide`)
//! actually bound the false-pass rate AGENTS.md's central claim depends on ("a false pass is
//! worse than an inconclusive result")? If `binomial_coverage.rs`'s per-method coverage checks
//! are correct, this should already follow - a well-calibrated two-sided CI at confidence `c`
//! puts the true parameter below its lower bound on close to `(1-c)/2` of draws, so a true effect
//! sitting exactly at the pass threshold should trigger a false pass on close to `(1-c)/2` of
//! draws too. This test checks that composition holds end to end, through the real public
//! `verdict::decide` function, not just asserted from the coverage result in isolation.

use rand::rngs::StdRng;
use rand::{RngExt, SeedableRng};
use veridict::Verdict;
use veridict::stats::wilson::wilson_ci;
use veridict::verdict::{Thresholds, decide};

const CONFIDENCE: f64 = 0.95;
const N: u64 = 100;
const SIMULATIONS: u64 = 10_000;
const SEED: u64 = 0x5EED;

/// Simulates one `winrate`-shaped experiment: draw `successes` from `Binomial(N, true_p)`,
/// compute the Wilson CI, center it the same way `metrics::winrate` does (`- 0.5`, since
/// `winrate`'s `effect`/`ci_low`/`ci_high` are deviation from a 50/50 split, not the raw
/// proportion), and run it through the real `verdict::decide`.
fn simulate_verdict(rng: &mut StdRng, true_p: f64, thresholds: &Thresholds) -> Verdict {
    let successes = (0..N).filter(|_| rng.random::<f64>() < true_p).count() as u64;
    let (lo, hi) = wilson_ci(successes, N, CONFIDENCE).unwrap();
    decide(lo - 0.5, hi - 0.5, thresholds).0
}

fn false_pass_rate(true_p: f64, thresholds: &Thresholds) -> f64 {
    let mut rng = StdRng::seed_from_u64(SEED);
    let false_passes = (0..SIMULATIONS)
        .filter(|_| simulate_verdict(&mut rng, true_p, thresholds) == Verdict::Pass)
        .count();
    false_passes as f64 / SIMULATIONS as f64
}

// The true effect sits exactly AT the pass threshold (min_effect=0.05, true winrate deviation
// exactly 0.05 i.e. true_p=0.55) - the single worst case for false-pass rate, since it's the
// point closest to "should legitimately pass" while still requiring the CI to have (wrongly)
// excluded the true parameter entirely below its lower bound. For a well-calibrated two-sided CI
// at confidence `c`, this should land near `(1-c)/2` - half of the two-sided error rate, since
// only the "CI's lower bound ended up above the true parameter" side of coverage failure produces
// a false pass here (the other half, "CI's upper bound ended up below," would show up as a false
// fail at this boundary instead, not a false pass).
//
// Observed once at SIMULATIONS=10,000, N=100, SEED=0x5EED: false-pass rate was 0.0261, close to
// the theoretical (1-0.95)/2=0.025 - consistent with Wilson's coverage already measured in
// binomial_coverage.rs, composed correctly through the real decide() function.
#[test]
fn false_pass_rate_at_the_threshold_boundary_tracks_half_the_two_sided_error_rate() {
    let thresholds = Thresholds::symmetric(0.05).unwrap();
    let rate = false_pass_rate(0.55, &thresholds);
    let expected = (1.0 - CONFIDENCE) / 2.0;
    assert!(
        rate < expected + 0.02,
        "false-pass rate {rate:.4} at the threshold boundary exceeded the expected ~{expected:.4} \
         by more than Monte Carlo noise at this simulation count accounts for"
    );
}

// Well inside the dead zone (true effect exactly 0, thresholds symmetric at +-0.05): a false pass
// here requires an even larger anomalous swing than at the boundary case above, so the rate
// should be substantially lower, not just "also small."
//
// Observed once at SIMULATIONS=10,000: false-pass rate was 0.0019 (about 1 in 500) - over an
// order of magnitude below the boundary case's 0.0261, confirming the gate gets substantially
// more conservative moving away from the threshold, not just uniformly small everywhere.
#[test]
fn false_pass_rate_deep_in_the_dead_zone_is_far_below_the_boundary_case() {
    let thresholds = Thresholds::symmetric(0.05).unwrap();
    let boundary_rate = false_pass_rate(0.55, &thresholds);
    let dead_zone_rate = false_pass_rate(0.50, &thresholds);
    assert!(
        dead_zone_rate < boundary_rate,
        "false-pass rate {dead_zone_rate:.4} deep in the dead zone (true_p=0.50) was not lower \
         than the threshold-boundary rate {boundary_rate:.4} (true_p=0.55) - the gate should get \
         more conservative moving away from the threshold, not less"
    );
    assert!(
        dead_zone_rate < 0.01,
        "false-pass rate {dead_zone_rate:.4} deep in the dead zone is higher than expected for a \
         true effect of exactly 0 against a 0.05 threshold"
    );
}
