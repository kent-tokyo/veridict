//! Exact enumeration, not Monte Carlo: for a small `T`, every one of the `2^T` decisive outcome
//! paths is walked exactly once, weighted by its exact probability under H0 (`p0`), and the
//! resulting sums are checked against the theory directly - no sampling noise, no simulation
//! tolerance to argue about. This is the anytime-validity proof this crate can actually run, for
//! every policy kind (`gro`, `bellman`, `edo`), not just the theoretical claim in a doc comment.
//!
//! Two checks, both required (see this crate's plan/spec): the practical inequality
//! `P_p0(reject within T) <= alpha`, and the stronger exact identity `E_p0[W_T] = 1` with
//! stopping *disabled* - the real martingale identity, which a subtly broken e-variable or
//! action-selection bug could violate even while still passing the weaker inequality by luck.

use super::*;

fn build(
    hypotheses: BernoulliHypotheses,
    alpha: f64,
    reward: RewardSchedule,
    policy: TimeSensitivePolicyKind,
    wealth_grid_size: usize,
    action_grid_size: usize,
) -> TimeSensitiveTest {
    TimeSensitiveTest::new(TimeSensitiveConfig {
        hypotheses,
        alpha,
        reward,
        policy,
        action_grid_size,
        wealth_grid_size,
    })
    .unwrap()
}

/// All `2^t` boolean paths (`true` = candidate win) in a deterministic, exhaustive order.
fn all_paths(t: usize) -> impl Iterator<Item = Vec<bool>> {
    assert!(
        t <= 20,
        "enumeration size must stay small - this is exhaustive, not sampled"
    );
    (0..(1u32 << t)).map(move |mask| (0..t).map(|bit| (mask >> bit) & 1 == 1).collect())
}

fn path_probability(path: &[bool], p: f64) -> f64 {
    path.iter()
        .map(|&success| if success { p } else { 1.0 - p })
        .product()
}

/// Replays one path against an already-built policy without reconstructing it (see
/// `compiled_action`'s doc for why that matters for `bellman`). `stop_at_threshold`: `true`
/// freezes wealth the instant it crosses (the real `TimeSensitiveTest::update` semantics, for the
/// `P(reject) <= alpha` check); `false` lets the process keep betting for the full path
/// regardless of an earlier crossing (for the unstopped `E_p0[W_T] = 1` martingale identity).
/// Returns `(final_log_wealth, rejected_at)`.
fn replay(test: &TimeSensitiveTest, path: &[bool], stop_at_threshold: bool) -> (f64, Option<u64>) {
    let log_threshold = (1.0 / test.config.alpha).ln();
    let mut log_wealth = 0.0_f64;
    let mut rejected_at = None;
    for (i, &success) in path.iter().enumerate() {
        if stop_at_threshold && rejected_at.is_some() {
            break;
        }
        let t = i as u64;
        let a = compiled_action(&test.policy, &test.config.hypotheses, t, log_wealth);
        log_wealth += e_variable::log_phi(a, test.config.hypotheses.p0, success);
        if rejected_at.is_none() && log_wealth >= log_threshold {
            rejected_at = Some(t + 1);
        }
    }
    (log_wealth, rejected_at)
}

/// A signal strong enough that a handful of early wins already crosses the threshold within `T`
/// (see this file's module doc: exhaustive coverage should exercise real crossings, not only
/// paths that trivially can't reach the boundary at all) - not meant to resemble a realistic
/// engine-testing config, only to make the enumeration non-vacuous at a small, fast `T`.
const P0: f64 = 0.3;
const P1: f64 = 0.7;
const ALPHA: f64 = 0.05;
const T: usize = 14;

fn assert_null_rejection_probability_and_martingale_identity(test: &TimeSensitiveTest) {
    let hyp = test.config.hypotheses;
    let mut reject_probability = 0.0_f64;
    let mut expected_wealth = 0.0_f64;
    let mut any_rejected = false;

    for path in all_paths(T) {
        let weight = path_probability(&path, hyp.p0);

        let (_, rejected_at) = replay(test, &path, true);
        if rejected_at.is_some() {
            any_rejected = true;
            reject_probability += weight;
        }

        let (log_wealth_unstopped, _) = replay(test, &path, false);
        expected_wealth += weight * log_wealth_unstopped.exp();
    }

    assert!(
        any_rejected,
        "no path crossed the threshold within T={T} trials - this config is too conservative to \
         exercise real crossings; the P(reject) <= alpha check below would be vacuously true"
    );
    assert!(
        reject_probability <= ALPHA + 1e-9,
        "P_p0(reject within {T}) = {reject_probability}, expected <= alpha = {ALPHA}"
    );
    assert!(
        (expected_wealth - 1.0).abs() < 1e-9,
        "E_p0[W_{T}] (unstopped) = {expected_wealth}, expected exactly 1.0"
    );
}

#[test]
fn gro_is_anytime_valid_under_exhaustive_enumeration() {
    let hyp = BernoulliHypotheses { p0: P0, p1: P1 };
    let reward = RewardSchedule::HardDeadline { deadline: T as u64 };
    let test = build(hyp, ALPHA, reward, TimeSensitivePolicyKind::Gro, 200, 2);
    assert_null_rejection_probability_and_martingale_identity(&test);
}

#[test]
fn bellman_is_anytime_valid_under_exhaustive_enumeration() {
    let hyp = BernoulliHypotheses { p0: P0, p1: P1 };
    let reward = RewardSchedule::HardDeadline { deadline: T as u64 };
    let test = build(
        hyp,
        ALPHA,
        reward,
        TimeSensitivePolicyKind::BellmanGrid,
        120,
        41,
    );
    assert_null_rejection_probability_and_martingale_identity(&test);
}

#[test]
fn edo_is_anytime_valid_under_exhaustive_enumeration() {
    let hyp = BernoulliHypotheses { p0: P0, p1: P1 };
    // min_time_scale = 1/ln(p1/p0) ~= 1.18; comfortably above it.
    let reward = RewardSchedule::ExponentialDecay {
        time_scale: 20.0,
        horizon: T as u64,
    };
    let test = build(hyp, ALPHA, reward, TimeSensitivePolicyKind::Edo, 200, 2);
    assert_null_rejection_probability_and_martingale_identity(&test);
}
