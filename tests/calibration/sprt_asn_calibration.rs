//! Empirical check of `power::estimate_sprt_expected_trials`'s Wald ASN approximation against a
//! real Monte Carlo simulation (`docs/metrics.md`'s claim - "typically needs somewhat more trials
//! than this number in practice" - checked by simulation, not trusted from the derivation alone):
//! simulate many Wald streams under each hypothesis, stopping as soon as either boundary is
//! crossed, and measure the real mean trials-to-decision. Wald's ASN formula ignores "overshoot"
//! (the LLR's excess past a boundary at the moment of stopping), so the true mean is expected to
//! run *higher* than the formula's prediction - this test quantifies that bias with a real number
//! rather than leaving it as an unmeasured caveat.
//!
//! Mirrors (does not duplicate) `sprt_error_rates.rs`'s `simulate_wald_stream` - that function
//! only returns the stopping `Verdict`; this one additionally needs the trial count, so it's a
//! small variant rather than a shared helper (the two files already don't share a support module -
//! see `tests/calibration.rs`'s doc on why each area lives in its own file).

use rand::rngs::StdRng;
use rand::{RngExt, SeedableRng};
use veridict::power::estimate_sprt_expected_trials;
use veridict::stats::sprt::{bounds, llr_delta, score_from_elo};

const ALPHA: f64 = 0.05;
const BETA: f64 = 0.05;
const SEED: u64 = 0x5EED;
const SIMULATIONS: u64 = 5_000;
const MAX_TRIALS: u64 = 20_000;

/// Simulates one Wald stream from a true win probability `true_p`, stopping as soon as either
/// bound is crossed, and returns the number of trials used (`MAX_TRIALS` if neither bound was
/// crossed - shouldn't happen at the elo gaps used below, but capped defensively rather than
/// looping unbounded).
fn simulate_trials_to_decision(rng: &mut StdRng, true_p: f64, elo0: f64, elo1: f64) -> u64 {
    let b = bounds(ALPHA, BETA);
    let (p0, p1) = (score_from_elo(elo0), score_from_elo(elo1));
    let mut llr = 0.0;
    for trial in 1..=MAX_TRIALS {
        let candidate_won = rng.random::<f64>() < true_p;
        llr += llr_delta(candidate_won, p0, p1);
        if llr >= b.upper || llr <= b.lower {
            return trial;
        }
    }
    MAX_TRIALS
}

fn mean_trials(true_p: f64, elo0: f64, elo1: f64) -> f64 {
    let mut rng = StdRng::seed_from_u64(SEED);
    let total: u64 = (0..SIMULATIONS)
        .map(|_| simulate_trials_to_decision(&mut rng, true_p, elo0, elo1))
        .sum();
    total as f64 / SIMULATIONS as f64
}

// Observed once at SIMULATIONS=5,000, SEED=0x5EED, elo0=0/elo1=20: formula predicted
// expected_trials_under_h0=1601, empirical mean was 1635.6 (+2.2%); expected_trials_under_h1=1603,
// empirical mean was 1631.6 (+1.8%). Both real means ran higher than the formula, the direction
// Wald's overshoot-free approximation predicts (a real run needs somewhat more trials in
// practice) - confirmed at higher precision too (n=100,000 in a scratch run: +0.9%/+1.6%), so this
// is a real, small, consistently-positive bias, not simulation noise in one direction. At
// SIMULATIONS=5,000 specifically, a much smaller (n=2,000) run showed the *empirical* mean
// briefly dip ~1% *below* the formula for H1 alone - real Monte Carlo noise at that sample count,
// not a sign the bias direction is unreliable (confirmed by the larger runs above) - which is why
// the tolerance below is two-sided (allows a small dip) rather than a strict one-sided "must be
// >=" that a single noisy run could fail. The lower bound (-3%) accommodates that kind of noise;
// the upper bound (20%) is a generous ceiling for this specific elo gap, not a general claim about
// every elo0/elo1/alpha/beta combination.
#[test]
fn asn_formula_tracks_the_real_mean_within_a_small_measured_margin() {
    let (elo0, elo1) = (0.0, 20.0);
    let report = estimate_sprt_expected_trials(elo0, elo1, ALPHA, BETA, None).unwrap();

    let empirical_h0 = mean_trials(score_from_elo(elo0), elo0, elo1);
    let empirical_h1 = mean_trials(score_from_elo(elo1), elo0, elo1);

    let formula_h0 = report.expected_trials_under_h0 as f64;
    let formula_h1 = report.expected_trials_under_h1 as f64;

    for (label, empirical, formula) in [
        ("H0", empirical_h0, formula_h0),
        ("H1", empirical_h1, formula_h1),
    ] {
        let ratio = empirical / formula;
        assert!(
            (0.97..=1.20).contains(&ratio),
            "{label}: empirical mean {empirical:.1} vs formula {formula} (ratio {ratio:.4}) fell \
             outside the measured tolerance - either the overshoot bias direction reversed or grew \
             far larger than observed at this elo gap"
        );
    }
}

// A Wald SPRT's expected sample size is unimodal in the true strength and peaks *between* the two
// hypotheses, not at either endpoint - `expected_trials_under_h0`/`_h1` are the two optimistic
// endpoint cases, not "the" expected sample size for a candidate whose true strength is unknown
// (the common case - that's the whole reason to run SPRT). This is the reasoning behind the
// `notes` caveat added to `SprtPowerReport`; this test locks the magnitude in empirically so it
// can't silently regress into an unqualified "expected_trials" claim later.
//
// Observed once at SIMULATIONS=5,000, SEED=0x5EED, elo0=0/elo1=20 (formula's endpoints: 1601/1603):
// true strength at the midpoint (elo=10) had empirical mean trials-to-decision 2631.7 - about
// 1.64x either endpoint, not a small correction like the overshoot bias above.
#[test]
fn asn_expected_trials_is_the_optimistic_endpoint_not_the_worst_case() {
    let (elo0, elo1) = (0.0, 20.0);
    let midpoint_elo = (elo0 + elo1) / 2.0;
    let report = estimate_sprt_expected_trials(elo0, elo1, ALPHA, BETA, None).unwrap();

    let empirical_midpoint = mean_trials(score_from_elo(midpoint_elo), elo0, elo1);
    let formula_h0 = report.expected_trials_under_h0 as f64;

    assert!(
        empirical_midpoint > formula_h0 * 1.3,
        "midpoint empirical mean {empirical_midpoint:.1} was not substantially higher than the \
         H0 endpoint {formula_h0} - expected the well-known SPRT property that expected sample \
         size peaks between the two hypotheses, not at either one"
    );
}
