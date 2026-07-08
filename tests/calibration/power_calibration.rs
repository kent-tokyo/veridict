//! Empirical verification that `power::estimate_trials`'s search actually achieves
//! `target_power` (`docs/metrics.md`'s claim, checked by simulation, not trusted from the
//! derivation alone): draw many `Binomial(estimated_trials, p1)` samples from the *known*
//! `assume_effect`-implied true proportion, compute the same CI method's lower bound for each,
//! and measure what fraction actually clears `min_effect`'s pass bar. That empirical pass rate is
//! what "target_power" means - it's the thing the search is supposed to guarantee, and the one
//! place the "sawtooth" non-monotonicity (`src/power.rs`'s module doc, citing Chernick & Liu 2002)
//! could have produced a lucky-but-unstable answer instead of a real one.

use rand::rngs::StdRng;
use rand::{RngExt, SeedableRng};
use veridict::CiMethod;
use veridict::power::{PowerMetric, estimate_trials};
use veridict::stats::exact::clopper_pearson_ci;
use veridict::stats::jeffreys::jeffreys_ci;
use veridict::stats::sprt::score_from_elo;
use veridict::stats::wilson::wilson_ci;

const SIMULATIONS: u64 = 5_000;
const SEED: u64 = 0x5EED;

fn effect_to_proportion(metric_is_elo: bool, effect: f64) -> f64 {
    if metric_is_elo {
        score_from_elo(effect)
    } else {
        0.5 + effect
    }
}

fn ci_low(ci_method: CiMethod, successes: u64, n: u64, confidence: f64) -> f64 {
    match ci_method {
        CiMethod::Wilson => wilson_ci(successes, n, confidence).unwrap().0,
        CiMethod::Exact => clopper_pearson_ci(successes, n, confidence).unwrap().0,
        CiMethod::Jeffreys => jeffreys_ci(successes, n, confidence).unwrap().0,
    }
}

/// Empirical pass rate of an `n`-trial experiment drawn from the true proportion `p1`, checked
/// against the pass bar `p0` - the same quantity `power::estimate_trials` claims to hit
/// `target_power` for, measured independently via simulation instead of recomputed analytically
/// (which would just re-run the same code under test).
fn empirical_power(n: u64, p0: f64, p1: f64, ci_method: CiMethod, confidence: f64) -> f64 {
    let mut rng = StdRng::seed_from_u64(SEED);
    let mut passed = 0u64;
    for _ in 0..SIMULATIONS {
        let successes = (0..n).filter(|_| rng.random::<f64>() < p1).count() as u64;
        if ci_low(ci_method, successes, n, confidence) >= p0 {
            passed += 1;
        }
    }
    passed as f64 / SIMULATIONS as f64
}

// Observed once at SIMULATIONS=5,000, SEED=0x5EED (recorded so a future reader can see the margin
// without re-running): min=0.0/assume=0.05/target=0.80 -> n=796, empirical 0.8154 (exact
// achieved_power 0.8096); min=0.02/assume=0.10/target=0.90 -> n=413, empirical 0.9150 (exact
// 0.9088) - both within ~0.006-0.01 of the exact computation, tracking the target closely, not
// just clearing a generous floor. 0.05 is a wide margin purely for Monte Carlo noise at this
// simulation count (a true miss - the search landing on a lucky sawtooth spike that doesn't hold
// empirically - would show up as a much larger gap, not a borderline one).
#[test]
fn winrate_achieved_power_matches_target_power_empirically() {
    for (min_effect, assume_effect, target_power) in [(0.0, 0.05, 0.80), (0.02, 0.10, 0.90)] {
        let metric = PowerMetric::WinRate {
            ci_method: CiMethod::Wilson,
        };
        let report =
            estimate_trials(metric, min_effect, assume_effect, 0.95, target_power, false).unwrap();
        let p0 = effect_to_proportion(false, min_effect);
        let p1 = effect_to_proportion(false, assume_effect);
        let empirical = empirical_power(report.estimated_trials, p0, p1, CiMethod::Wilson, 0.95);
        assert!(
            empirical >= target_power - 0.05,
            "winrate min={min_effect} assume={assume_effect}: empirical power {empirical:.4} at \
             n={} fell more than 0.05 below target {target_power}",
            report.estimated_trials
        );
    }
}

// Observed once at SIMULATIONS=5,000, SEED=0x5EED: min=20/assume=35/target=0.80 -> n=4281,
// empirical 0.8036 (exact 0.8044); min=10/assume=60/target=0.90 -> n=527, empirical 0.9166 (exact
// 0.9082).
#[test]
fn elo_achieved_power_matches_target_power_empirically() {
    for (min_effect, assume_effect, target_power) in [(20.0, 35.0, 0.80), (10.0, 60.0, 0.90)] {
        let metric = PowerMetric::Elo;
        let report =
            estimate_trials(metric, min_effect, assume_effect, 0.95, target_power, false).unwrap();
        let p0 = effect_to_proportion(true, min_effect);
        let p1 = effect_to_proportion(true, assume_effect);
        let empirical = empirical_power(report.estimated_trials, p0, p1, CiMethod::Wilson, 0.95);
        assert!(
            empirical >= target_power - 0.05,
            "elo min={min_effect} assume={assume_effect}: empirical power {empirical:.4} at \
             n={} fell more than 0.05 below target {target_power}",
            report.estimated_trials
        );
    }
}

// Clopper-Pearson/Jeffreys are the exact-discrete methods the "sawtooth" finding is specifically
// about (Wilson is a smooth normal approximation, less prone to it) - this is the test that most
// directly exercises the stability-window logic in `estimate_smallest_n`. Observed once at
// SIMULATIONS=5,000, SEED=0x5EED: min=0.0/assume=0.08/target=0.80 -> n=325, empirical 0.8240
// (exact 0.8158).
#[test]
fn exact_ci_method_achieved_power_matches_target_power_empirically() {
    let metric = PowerMetric::WinRate {
        ci_method: CiMethod::Exact,
    };
    let report = estimate_trials(metric, 0.0, 0.08, 0.95, 0.80, false).unwrap();
    let p0 = effect_to_proportion(false, 0.0);
    let p1 = effect_to_proportion(false, 0.08);
    let empirical = empirical_power(report.estimated_trials, p0, p1, CiMethod::Exact, 0.95);
    assert!(
        empirical >= 0.80 - 0.05,
        "exact-CI winrate: empirical power {empirical:.4} at n={} fell more than 0.05 below \
         target 0.80",
        report.estimated_trials
    );
}
