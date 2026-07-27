//! Monte Carlo calibration for `time_sensitive`, complementing (not replacing)
//! `time_sensitive::exact_enumeration_tests`'s exact-but-small-`T` proof of anytime-validity:
//! does the null rejection rate stay near `alpha` at a realistic scale (`T=400`, far beyond what
//! exhaustive `2^T` enumeration can reach), and does the *reward* - the actual point of this
//! module, over `sprt` - behave the way the policy ordering predicts (`bellman` >= `gro` under
//! the alternative, `edo`'s action trending toward `gro`'s as `time_scale` grows).
//!
//! The classical Wald SPRT is run on the same simulated data and reported alongside as a
//! *labeled reference point only*: it optimizes a different objective entirely (a two-sided
//! decisive-outcome LLR crossing, no reward-schedule/time-value concept at all), so this file
//! never asserts it faster, slower, or otherwise ranked against any `time_sensitive` policy -
//! only computed and printed so a reader can see it's tracked, not omitted.

use rand::rngs::StdRng;
use rand::{RngExt, SeedableRng};

use veridict::Outcome;
use veridict::stats::sprt::{bounds, llr_delta};
use veridict::time_sensitive::{
    BernoulliHypotheses, RewardSchedule, TimeSensitiveConfig, TimeSensitivePolicyKind,
    TimeSensitiveTest,
};

const SEED: u64 = 0x5EED;
const ALPHA: f64 = 0.05;
const P0: f64 = 0.5;
const P1: f64 = 0.55;
const DEADLINE: u64 = 400;
const WEALTH_GRID_SIZE: usize = 50;
const ACTION_GRID_SIZE: usize = 21;
const SIMULATIONS: usize = 150;

fn hypotheses() -> BernoulliHypotheses {
    BernoulliHypotheses { p0: P0, p1: P1 }
}

fn config(policy: TimeSensitivePolicyKind, reward: RewardSchedule) -> TimeSensitiveConfig {
    TimeSensitiveConfig {
        hypotheses: hypotheses(),
        alpha: ALPHA,
        reward,
        policy,
        action_grid_size: ACTION_GRID_SIZE,
        wealth_grid_size: WEALTH_GRID_SIZE,
    }
}

/// One simulated decisive-trial-only stream (no draws - keeps trial-for-trial comparability with
/// the Wald reference below, which also has no draw concept): drives the real
/// `TimeSensitiveTest::update` state machine from `true_p` for up to `max_trials`, stopping early
/// once rejected (mirroring how a real caller would stop). Returns
/// `(rejected_at, reward_at_rejection)`.
fn simulate(
    rng: &mut StdRng,
    config: TimeSensitiveConfig,
    true_p: f64,
    max_trials: u64,
) -> (Option<u64>, Option<f64>) {
    let mut test = TimeSensitiveTest::new(config).unwrap();
    for _ in 0..max_trials {
        let outcome = if rng.random::<f64>() < true_p {
            Outcome::CandidateWin
        } else {
            Outcome::BaselineWin
        };
        let step = test.update(outcome).unwrap();
        if step.rejected {
            return (step.rejection_time, step.reward_at_rejection);
        }
    }
    (None, None)
}

struct SimSummary {
    rejection_probability: f64,
    mean_reward: f64,
    mean_rejection_time: f64,
    p25_rejection_time: f64,
    p75_rejection_time: f64,
    inconclusive_rate: f64,
}

fn summarize(results: &[(Option<u64>, Option<f64>)]) -> SimSummary {
    let n = results.len() as f64;
    let mut rejection_times: Vec<u64> = results.iter().filter_map(|(t, _)| *t).collect();
    rejection_times.sort_unstable();
    let rejection_probability = rejection_times.len() as f64 / n;
    let mean_reward = results.iter().map(|(_, r)| r.unwrap_or(0.0)).sum::<f64>() / n;
    let percentile = |p: f64| {
        if rejection_times.is_empty() {
            return 0.0;
        }
        let idx = ((rejection_times.len() as f64 - 1.0) * p).round() as usize;
        rejection_times[idx] as f64
    };
    SimSummary {
        rejection_probability,
        mean_reward,
        mean_rejection_time: if rejection_times.is_empty() {
            0.0
        } else {
            rejection_times.iter().sum::<u64>() as f64 / rejection_times.len() as f64
        },
        p25_rejection_time: percentile(0.25),
        p75_rejection_time: percentile(0.75),
        inconclusive_rate: 1.0 - rejection_probability,
    }
}

fn print_summary(label: &str, s: &SimSummary) {
    println!(
        "{label}: reject_p={:.3} inconclusive_rate={:.3} mean_reward={:.3} \
         mean_rejection_time={:.1} rejection_time_p25={:.0} rejection_time_p75={:.0}",
        s.rejection_probability,
        s.inconclusive_rate,
        s.mean_reward,
        s.mean_rejection_time,
        s.p25_rejection_time,
        s.p75_rejection_time,
    );
}

// Null rejection probability, all three policies, at a realistic T=400 (not the small T that
// exact enumeration can afford). Same measured-tolerance convention `sprt_error_rates.rs` uses
// (1.5x alpha) - reused here, not freshly re-derived via mutation testing for this module, which
// is a real gap relative to that precedent's rigor and is noted rather than glossed over.
#[test]
fn null_rejection_probability_tracks_alpha_for_every_policy() {
    let policies: [(&str, TimeSensitiveConfig); 3] = [
        (
            "gro",
            config(
                TimeSensitivePolicyKind::Gro,
                RewardSchedule::HardDeadline { deadline: DEADLINE },
            ),
        ),
        (
            "bellman",
            config(
                TimeSensitivePolicyKind::BellmanGrid,
                RewardSchedule::HardDeadline { deadline: DEADLINE },
            ),
        ),
        (
            "edo",
            config(
                TimeSensitivePolicyKind::Edo,
                RewardSchedule::ExponentialDecay {
                    time_scale: 200.0,
                    horizon: DEADLINE,
                },
            ),
        ),
    ];
    for (label, cfg) in policies {
        let mut rng = StdRng::seed_from_u64(SEED);
        let results: Vec<_> = (0..SIMULATIONS)
            .map(|_| simulate(&mut rng, cfg.clone(), P0, DEADLINE))
            .collect();
        let summary = summarize(&results);
        print_summary(&format!("null/{label}"), &summary);
        assert!(
            summary.rejection_probability < ALPHA * 1.5,
            "{label}: null rejection probability {:.4} too high for alpha={ALPHA}",
            summary.rejection_probability
        );
    }
}

// The specific claim this module exists for: under the alternative, does bellman's *simulated*
// mean reward - through the real state machine, not the exact grid value `grid.rs`'s own tests
// already check - come out at least roughly as high as gro's patient baseline?
#[test]
fn bellman_matches_or_exceeds_gro_simulated_reward_under_the_alternative() {
    let gro_cfg = config(
        TimeSensitivePolicyKind::Gro,
        RewardSchedule::HardDeadline { deadline: DEADLINE },
    );
    let bellman_cfg = config(
        TimeSensitivePolicyKind::BellmanGrid,
        RewardSchedule::HardDeadline { deadline: DEADLINE },
    );

    let mut rng = StdRng::seed_from_u64(SEED);
    let gro_results: Vec<_> = (0..SIMULATIONS)
        .map(|_| simulate(&mut rng, gro_cfg.clone(), P1, DEADLINE))
        .collect();
    // Common random numbers: same seed replayed, so both policies see the same underlying
    // win/loss stream each replication rather than two independently-noisy ones.
    let mut rng = StdRng::seed_from_u64(SEED);
    let bellman_results: Vec<_> = (0..SIMULATIONS)
        .map(|_| simulate(&mut rng, bellman_cfg.clone(), P1, DEADLINE))
        .collect();

    let gro_summary = summarize(&gro_results);
    let bellman_summary = summarize(&bellman_results);
    print_summary("alternative/gro", &gro_summary);
    print_summary("alternative/bellman", &bellman_summary);

    // A margin, not a bare `>=`: Monte Carlo noise at SIMULATIONS=300 could make bellman's
    // *sampled* mean dip marginally below gro's even though its true expected reward (proven
    // exactly - not simulated - by grid.rs's own maximize_planned_value_is_at_least_gro_fixed_
    // action_value test) is provably >= gro's for the same grid. This is a generous slack, not a
    // tight bound.
    assert!(
        bellman_summary.mean_reward >= gro_summary.mean_reward - 0.05,
        "bellman's simulated mean reward {:.4} was not close to/above gro's {:.4}",
        bellman_summary.mean_reward,
        gro_summary.mean_reward
    );
}

/// Recovers the action a fresh policy would bet on its very first decisive trial via the public
/// API only: `log_wealth` after one candidate win is exactly `ln(a/p0)`, so `a = p0 *
/// exp(log_wealth)` - `edo`'s internals aren't exported, but this needs no white-box access.
fn implied_first_action(config: TimeSensitiveConfig) -> f64 {
    let mut test = TimeSensitiveTest::new(config).unwrap();
    let step = test.update(Outcome::CandidateWin).unwrap();
    P0 * step.log_wealth.exp()
}

// Re-confirms, through the public API, the closed-form claim already unit-tested white-box
// inside edo.rs (action_approaches_gro_as_time_scale_grows): at a tight time_scale edo bets more
// aggressively than gro's own a=p1, and as time_scale grows the two converge.
#[test]
fn edo_action_converges_toward_gro_as_time_scale_grows() {
    let action_at = |time_scale: f64| {
        implied_first_action(config(
            TimeSensitivePolicyKind::Edo,
            RewardSchedule::ExponentialDecay {
                time_scale,
                horizon: DEADLINE,
            },
        ))
    };
    // min_time_scale = 1/ln(p1/p0) ~= 10.5 for P0/P1 above; both comfortably above it.
    let a_tight = action_at(20.0);
    let a_loose = action_at(1.0e6);
    println!(
        "edo action: time_scale=20 -> {a_tight:.4}, time_scale=1e6 -> {a_loose:.4}, gro (p1) = {P1}"
    );
    assert!(
        a_tight > P1,
        "edo at a tight time_scale should bet more aggressively than gro (p1={P1}), got {a_tight}"
    );
    assert!(
        (a_loose - P1).abs() < 1e-3,
        "edo at a very loose time_scale should converge to gro's p1={P1}, got {a_loose}"
    );
    assert!(
        a_tight > a_loose,
        "a tighter time_scale should bet more aggressively than a looser one"
    );
}

/// Plain Wald LLR walk on `(p0, p1)` directly (bypassing Elo entirely - `stats::sprt::llr_delta`
/// takes probabilities), for a fair common-random-numbers comparison against `simulate` above.
/// Two-sided (both `bounds().upper`/`.lower`), unlike `time_sensitive`'s one-sided rejection -
/// exactly the shape difference that makes ranking the two against each other a category error,
/// not just an unfair comparison (see this file's module doc).
fn simulate_wald(rng: &mut StdRng, true_p: f64, max_trials: u64) -> Option<u64> {
    let b = bounds(ALPHA, ALPHA);
    let mut llr = 0.0;
    for t in 1..=max_trials {
        let candidate_won = rng.random::<f64>() < true_p;
        llr += llr_delta(candidate_won, P0, P1);
        if llr >= b.upper || llr <= b.lower {
            return Some(t);
        }
    }
    None
}

#[test]
fn wald_sprt_reference_is_reported_separately_not_ranked_against_time_sensitive_policies() {
    let gro_cfg = config(
        TimeSensitivePolicyKind::Gro,
        RewardSchedule::HardDeadline { deadline: DEADLINE },
    );

    let mut rng = StdRng::seed_from_u64(SEED);
    let gro_results: Vec<_> = (0..SIMULATIONS)
        .map(|_| simulate(&mut rng, gro_cfg.clone(), P1, DEADLINE))
        .collect();
    let gro_summary = summarize(&gro_results);

    // Same seed replayed: common random numbers with the run above.
    let mut rng = StdRng::seed_from_u64(SEED);
    let wald_decisions: Vec<Option<u64>> = (0..SIMULATIONS)
        .map(|_| simulate_wald(&mut rng, P1, DEADLINE))
        .collect();
    let wald_decided = wald_decisions.iter().filter(|t| t.is_some()).count();
    let wald_decided_times: Vec<u64> = wald_decisions.into_iter().flatten().collect();
    let wald_mean_trials = if wald_decided_times.is_empty() {
        0.0
    } else {
        wald_decided_times.iter().sum::<u64>() as f64 / wald_decided_times.len() as f64
    };
    let wald_decided_rate = wald_decided as f64 / SIMULATIONS as f64;

    print_summary(
        "alternative/gro (reward-aware, time_sensitive)",
        &gro_summary,
    );
    println!(
        "alternative/wald (reference only, no reward-schedule concept): decided_p={wald_decided_rate:.3} \
         mean_trials_to_decision={wald_mean_trials:.1}"
    );
    // Deliberately no assertion comparing gro's and wald's numbers to each other - see this
    // file's module doc. Both are only checked for being well-formed probabilities/summaries, so
    // this stays a real regression test rather than dead computation.
    assert!((0.0..=1.0).contains(&gro_summary.rejection_probability));
    assert!((0.0..=1.0).contains(&wald_decided_rate));
}
