//! Time-sensitive anytime-valid testing: an early rejection is worth more than a late one, and
//! this module lets a caller say so directly, as a reward assigned to *when* the wealth process
//! crosses `1/alpha`, rather than only *whether* it does.
//!
//! This is not a replacement for [`crate::sprt`]. `sprt` answers "is the candidate decisively
//! better?" `time_sensitive` answers "which betting policy maximizes expected reward under the
//! alternative, given a computational/decision budget?" - a genuinely different question,
//! answered independently (see [`e_variable::gro_action`]'s doc for why GRO, despite being
//! betting-equivalent to `sprt`'s classical Wald walk, is implemented separately rather than
//! calling into it). v1 scope is deliberately narrow: **Bernoulli simple-vs-simple only** (a
//! fixed null `p0` and alternative `p1`, one-sided rejection). See `docs/research-map.md` for
//! what's explicitly out of scope and why.
//!
//! ## Attribution
//!
//! This implementation is independently derived from the mathematical framework in:
//!
//! > E. Clerico, T. Wegel, I. Azangulov, and P. Rebeschini, "Time-sensitive anytime-valid
//! > testing," arXiv:2605.06521v1, 2026. Paper licensed under CC BY 4.0.
//!
//! The implementation is modified and independent, built from the paper's equations re-derived
//! by hand (see `edo.rs`'s module doc for the EDO closed form's own derivation) rather than
//! transcribed from its text or figures. The paper's authors do not endorse or maintain this
//! software.
//!
//! ## Why anytime-validity survives every approximation in this module
//!
//! Every policy here - `gro`, `bellman`, `edo` - reduces to picking an action `a in (0,1)` each
//! trial and multiplying `log_wealth` by `log(phi(a, outcome))`. [`e_variable::phi`] is a valid
//! e-variable under H0 (`Ber(p0)`) for *every* `a`, not just the "correct" one, so the
//! accumulated wealth is a nonnegative H0-martingale regardless of which policy chose `a` or how
//! well it approximated the true optimum. Grid resolution, floor truncation, and closed-form
//! approximation error (see `grid.rs`, `bellman.rs`, `edo.rs`) therefore can only ever change how
//! *fast* wealth tends to grow under the true alternative (optimality) - never whether crossing
//! `1/alpha` under the null happens at rate `<= alpha` (validity). That separation is what lets
//! this module ship a numerically-approximated Bellman policy at all: an approximation bug in
//! *choosing* `a` is a wasted-opportunity bug, not a false-positive-rate bug. A misspecified
//! `p1` (the true alternative isn't actually `p1`) is the same kind of bug for the same reason:
//! it changes which policy is actually growth-optimal, never the alpha-level guarantee, which is
//! a statement about H0 (`p0`) alone.

mod bellman;
mod e_variable;
mod edo;
#[cfg(test)]
mod exact_enumeration_tests;
mod grid;
mod report;
pub mod reward;

pub use e_variable::BernoulliHypotheses;
pub use report::{TimeSensitiveReport, apply_failure_caps};
pub use reward::RewardSchedule;

use crate::error::VeridictError;
use crate::input::Record;
use crate::metrics::{FailureBreakdown, effective_outcome, tally_status};
use crate::{FailurePolicy, Outcome, Promotion, Validity, Verdict};

/// Backward-induction cost cap (`horizon * wealth_grid_size * action_grid_size`) checked before
/// any grid is built - large enough for every configuration this module's README documents, small
/// enough that a misconfigured run fails fast (a `VeridictError`, checked in
/// `TimeSensitiveConfig::validate`) rather than silently running for minutes. See `grid.rs`'s
/// `estimate_ops` for the estimate this is compared against, and `docs/research-map.md`/this
/// module's own doc for the documented `O(horizon * wealth_grid_size * action_grid_size)`
/// time / `O(horizon * wealth_grid_size)` policy-table-memory cost this bounds.
const MAX_GRID_OPS: u128 = 200_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeSensitivePolicyKind {
    BellmanGrid,
    Edo,
    Gro,
}

impl TimeSensitivePolicyKind {
    fn policy_label(self) -> &'static str {
        match self {
            TimeSensitivePolicyKind::BellmanGrid => "bellman",
            TimeSensitivePolicyKind::Edo => "edo",
            TimeSensitivePolicyKind::Gro => "gro",
        }
    }

    /// Deliberately never "optimal" - see `bellman.rs`/`edo.rs`'s module docs.
    fn method_label(self) -> &'static str {
        match self {
            TimeSensitivePolicyKind::BellmanGrid => "bellman_grid_approximation",
            TimeSensitivePolicyKind::Edo => "edo_stationary_approximation",
            TimeSensitivePolicyKind::Gro => "gro",
        }
    }
}

#[derive(Debug, Clone)]
pub struct TimeSensitiveConfig {
    pub hypotheses: BernoulliHypotheses,
    pub alpha: f64,
    pub reward: RewardSchedule,
    pub policy: TimeSensitivePolicyKind,
    pub action_grid_size: usize,
    pub wealth_grid_size: usize,
}

impl TimeSensitiveConfig {
    fn validate(&self) -> Result<(), VeridictError> {
        self.hypotheses.validate()?;
        if !self.alpha.is_finite() || self.alpha <= 0.0 || self.alpha >= 1.0 {
            return Err(VeridictError::InvalidThreshold(format!(
                "alpha must be finite and in (0, 1), got {}",
                self.alpha
            )));
        }
        self.reward.validate()?;
        if self.wealth_grid_size < 2 {
            return Err(VeridictError::InvalidThreshold(
                "wealth_grid_size must be >= 2".to_string(),
            ));
        }
        if self.action_grid_size < 2 {
            return Err(VeridictError::InvalidThreshold(
                "action_grid_size must be >= 2".to_string(),
            ));
        }
        // EDO's moment equation (see edo.rs's module doc) is derived specifically for
        // R(t) = exp(-t/time_scale); it has no meaning for a schedule with no time_scale.
        if self.policy == TimeSensitivePolicyKind::Edo
            && !matches!(self.reward, RewardSchedule::ExponentialDecay { .. })
        {
            return Err(VeridictError::InvalidThreshold(
                "--policy edo requires --reward exponential (a time_scale): EDO's moment \
                 equation is derived specifically for R(t) = exp(-t/time_scale), see this \
                 crate's time_sensitive::edo module doc"
                    .to_string(),
            ));
        }
        let horizon = self.reward.horizon();
        let estimated_ops =
            grid::estimate_ops(horizon, self.wealth_grid_size, self.action_grid_size);
        if estimated_ops > MAX_GRID_OPS {
            return Err(VeridictError::TimeSensitiveGridTooLarge {
                horizon,
                wealth_grid_size: self.wealth_grid_size,
                action_grid_size: self.action_grid_size,
                estimated_ops,
                cap: MAX_GRID_OPS,
            });
        }
        Ok(())
    }
}

enum CompiledPolicy {
    Gro,
    Bellman(bellman::BellmanGrid),
    Edo(edo::EdoPolicy),
}

/// Free-function form of the policy-to-action dispatch, so a test harness (see
/// `exact_enumeration_tests`) can replay a decisive path against an already-built policy without
/// needing a live `&mut TimeSensitiveTest` per path - rebuilding `bellman`'s policy table per
/// path would be wastefully expensive across thousands of enumerated paths.
fn compiled_action(
    policy: &CompiledPolicy,
    hypotheses: &BernoulliHypotheses,
    t: u64,
    log_wealth: f64,
) -> f64 {
    match policy {
        CompiledPolicy::Gro => e_variable::gro_action(hypotheses),
        CompiledPolicy::Bellman(b) => b.action(t, log_wealth),
        CompiledPolicy::Edo(e) => e.action(),
    }
}

/// One `update()` call's resulting state - the same shape whether or not this call actually
/// changed anything (see `TimeSensitiveTest::update`'s doc on idempotence after rejection).
#[derive(Debug, Clone, Copy)]
pub struct TimeSensitiveStep {
    pub trial_count: u64,
    pub log_wealth: f64,
    pub rejected: bool,
    pub rejection_time: Option<u64>,
    pub reward_at_rejection: Option<f64>,
}

/// A live, resumable time-sensitive test. An external runner calls [`Self::update`] once per
/// trial (in whatever order trials actually complete) and can act the instant [`TimeSensitiveStep::rejected`]
/// turns `true` - that's the point of an anytime-valid procedure. [`Self::report`] can be called
/// at any point, decided or not.
pub struct TimeSensitiveTest {
    config: TimeSensitiveConfig,
    policy: CompiledPolicy,
    planned_value: f64,
    trial_count: u64,
    decisive_count: u64,
    draw_count: u64,
    log_wealth: f64,
    rejection_time: Option<u64>,
    reward_at_rejection: Option<f64>,
}

impl TimeSensitiveTest {
    pub fn new(config: TimeSensitiveConfig) -> Result<Self, VeridictError> {
        config.validate()?;
        let policy = match config.policy {
            TimeSensitivePolicyKind::Gro => CompiledPolicy::Gro,
            TimeSensitivePolicyKind::BellmanGrid => {
                CompiledPolicy::Bellman(bellman::BellmanGrid::build(
                    &config.hypotheses,
                    config.alpha,
                    &config.reward,
                    config.wealth_grid_size,
                    config.action_grid_size,
                ))
            }
            TimeSensitivePolicyKind::Edo => CompiledPolicy::Edo(edo::EdoPolicy::build(
                &config.hypotheses,
                config.alpha,
                &config.reward,
                config.wealth_grid_size,
            )?),
        };
        let planned_value = match &policy {
            CompiledPolicy::Gro => {
                let grid = grid::WealthGrid::new(config.alpha, config.wealth_grid_size);
                grid::evaluate_fixed_action(
                    config.hypotheses.p0,
                    config.hypotheses.p1,
                    &config.reward,
                    &grid,
                    config.hypotheses.p1,
                )
            }
            CompiledPolicy::Bellman(b) => b.planned_value(),
            CompiledPolicy::Edo(e) => e.planned_value(),
        };
        Ok(Self {
            config,
            policy,
            planned_value,
            trial_count: 0,
            decisive_count: 0,
            draw_count: 0,
            log_wealth: 0.0,
            rejection_time: None,
            reward_at_rejection: None,
        })
    }

    fn action_for(&self, t: u64, log_wealth: f64) -> f64 {
        compiled_action(&self.policy, &self.config.hypotheses, t, log_wealth)
    }

    /// Feeds one trial's outcome to the wealth process.
    ///
    /// `Outcome::Draw` advances `trial_count` (and so the reward-schedule clock the active policy
    /// reads) without moving `log_wealth` at all: a draw is still a consumed trial - real
    /// computational/wall-clock budget spent - not a free one, so silently discarding it would
    /// let a draw-heavy stream's reward-schedule deadline arrive "for free" relative to a
    /// decisive-heavy stream of the same length. It carries no information about which Bernoulli
    /// hypothesis is true either, so it correctly leaves `log_wealth` untouched - matching
    /// `sprt`'s own "decisive games only" convention for what counts as evidence, while still
    /// counting toward *time*, which `sprt` has no notion of at all.
    ///
    /// A record that resolves to no `Outcome` under the active `FailurePolicy` (see
    /// `metrics::effective_outcome`) never reaches this method at all (see `run`'s loop) - reused
    /// unchanged from `sprt`'s own failure-handling semantics, not a new time_sensitive-specific
    /// rule.
    ///
    /// Idempotent once rejected, or once the reward schedule's horizon is reached without
    /// rejecting: further calls return the same frozen step. This is what makes "stop calling
    /// `update` the instant you can act" and "keep calling it for every remaining record in a
    /// static file" both correct without the caller needing to track which regime it's in -
    /// exactly the property the optional-stopping test in this module's test suite checks.
    pub fn update(&mut self, outcome: Outcome) -> Result<TimeSensitiveStep, VeridictError> {
        let horizon = self.config.reward.horizon();
        if self.rejection_time.is_none() && self.trial_count < horizon {
            match outcome {
                Outcome::Draw => {
                    self.trial_count += 1;
                    self.draw_count += 1;
                }
                Outcome::CandidateWin | Outcome::BaselineWin => {
                    let t = self.trial_count;
                    let a = self.action_for(t, self.log_wealth);
                    let success = matches!(outcome, Outcome::CandidateWin);
                    self.log_wealth += e_variable::log_phi(a, self.config.hypotheses.p0, success);
                    self.trial_count += 1;
                    self.decisive_count += 1;
                    let log_threshold = (1.0 / self.config.alpha).ln();
                    if self.log_wealth >= log_threshold {
                        self.rejection_time = Some(self.trial_count);
                        self.reward_at_rejection = Some(self.config.reward.at(self.trial_count));
                    }
                }
            }
        }
        Ok(self.step())
    }

    fn step(&self) -> TimeSensitiveStep {
        TimeSensitiveStep {
            trial_count: self.trial_count,
            log_wealth: self.log_wealth,
            rejected: self.rejection_time.is_some(),
            rejection_time: self.rejection_time,
            reward_at_rejection: self.reward_at_rejection,
        }
    }

    /// Snapshot report, callable at any point. `timeouts`/`crashes`/`invalid`/`failure_breakdown`
    /// are always `0`/`default` here - this state machine only ever sees resolved `Outcome`s, not
    /// raw per-side status fields, so it has nothing to report on that front; `run` (the
    /// CLI-facing convenience wrapper, which does see raw records) fills those in afterward.
    pub fn report(&self) -> TimeSensitiveReport {
        let h = self.config.hypotheses;
        let horizon = self.config.reward.horizon();
        let wealth_threshold = 1.0 / self.config.alpha;
        let log_threshold = wealth_threshold.ln();

        let (verdict, reason) = match self.rejection_time {
            Some(t) => (
                Verdict::Pass,
                format!(
                    "log_wealth {:.4} crossed log_threshold {:.4} (wealth >= 1/alpha = {:.3}) at \
                     trial {t}: reject H0 (p <= {:.4}), accept H1 (p >= {:.4})",
                    self.log_wealth, log_threshold, wealth_threshold, h.p0, h.p1
                ),
            ),
            None if self.trial_count >= horizon => (
                Verdict::Inconclusive,
                format!(
                    "reached horizon ({horizon}) without crossing log_threshold {log_threshold:.4} \
                     (log_wealth={:.4}): inconclusive - not evidence for H0, just an absence of \
                     evidence for H1 within the allotted trials/reward-schedule budget",
                    self.log_wealth
                ),
            ),
            None => (
                Verdict::Inconclusive,
                format!(
                    "log_wealth {:.4} has not yet crossed log_threshold {log_threshold:.4} \
                     ({}/{horizon} trials used): keep testing",
                    self.log_wealth, self.trial_count
                ),
            ),
        };

        let numerical_tolerance =
            grid::WealthGrid::new(self.config.alpha, self.config.wealth_grid_size).step();

        let mut notes = vec![
            "simple-vs-simple Bernoulli model: fixed null p0 and alternative p1, one-sided \
             rejection only (0 < p0 < p1 < 1)"
                .to_string(),
            "p0/p1 are decisive-observation-conditional success probabilities; a Draw advances \
             trial_count and the reward-schedule clock without being a Bernoulli(p0)/Bernoulli(p1) \
             observation at all - see TimeSensitiveTest::update's doc"
                .to_string(),
            self.config.reward.truncation_note(),
            "anytime-validity (the alpha-level type-I error guarantee) is exact for any action \
             sequence; only the *choice* of action (bellman/edo) is a numerical approximation of \
             the growth-optimal policy - see this module's own doc for why the two are provably \
             separable"
                .to_string(),
            "optimality assumes the true alternative matches p1; a misspecified p1 changes which \
             policy is actually growth-optimal, never the alpha-level validity guarantee, which is \
             a statement about p0 alone"
                .to_string(),
            "planned_expected_reward_under_p1 is computed under an all-decisive idealization: the \
             value recursion has only success/failure transitions weighted by p1 and no draw-rate \
             input, so on a draw-heavy stream the realized reward will lag this number - draws \
             consume reward-schedule time the recursion never modeled"
                .to_string(),
        ];
        match &self.policy {
            CompiledPolicy::Bellman(_) => notes.push(
                "bellman is a numerical approximation on a finite (action_grid_size, \
                 wealth_grid_size) grid, not a proof of optimality on the continuous action/wealth \
                 space"
                    .to_string(),
            ),
            CompiledPolicy::Edo(e) => notes.push(format!(
                "edo is a stationary (time-independent) approximation valid specifically for \
                 exponential-decay rewards, not the true (generally time-varying) Bellman-optimal \
                 policy; solved eta*={:.6} (see time_sensitive::edo's module doc for the moment \
                 equation this solves)",
                e.eta()
            )),
            CompiledPolicy::Gro => notes.push(
                "gro ignores the reward schedule entirely (it is the classical anytime-valid, \
                 patient baseline) - it is the large-time-scale limit of edo, not a competing \
                 time-sensitive optimization"
                    .to_string(),
            ),
        }

        TimeSensitiveReport {
            schema_version: crate::report::REPORT_SCHEMA_VERSION,
            verdict,
            validity: Validity::Valid,
            promotion: Promotion::decide(Validity::Valid, verdict),
            method: self.config.policy.method_label(),
            policy_kind: self.config.policy.policy_label(),
            p0: h.p0,
            p1: h.p1,
            alpha: self.config.alpha,
            reward_kind: self.config.reward.kind_label(),
            reward_parameters: self.config.reward.parameters(),
            trial_count: self.trial_count,
            decisive_count: self.decisive_count,
            draw_count: self.draw_count,
            log_wealth: self.log_wealth,
            wealth_threshold,
            rejection_time: self.rejection_time,
            reward_at_rejection: self.reward_at_rejection,
            planned_expected_reward_under_p1: self.planned_value,
            action_grid_size: self.config.action_grid_size,
            wealth_grid_size: self.config.wealth_grid_size,
            numerical_tolerance,
            timeouts: 0,
            crashes: 0,
            invalid: 0,
            failure_breakdown: FailureBreakdown::default(),
            reason,
            notes,
        }
    }
}

/// CLI-facing convenience entry point: builds a [`TimeSensitiveTest`], replays every input
/// record through it in order (mirroring `sprt::run`'s own classify-then-update loop, including
/// its `SchemaMismatch` guard against a record with neither a status nor a result field), and
/// merges in the failure counts `TimeSensitiveTest` itself never sees (see `report`'s doc).
///
/// Unlike `sprt::run` (which - for `Wald`/`Trinomial` - computes its LLR once from final
/// aggregate counts, see `sprt.rs`'s own doc on why that isn't true sequential replay), this
/// function performs genuine trial-by-trial replay: `rejection_time` and optional-stopping
/// idempotence are load-bearing outputs here, not incidental ones, so there is no final-aggregate
/// shortcut available.
pub fn run<I>(
    records: I,
    config: TimeSensitiveConfig,
    failure_policy: FailurePolicy,
) -> Result<TimeSensitiveReport, VeridictError>
where
    I: IntoIterator<Item = Result<(usize, Record), VeridictError>>,
{
    let mut test = TimeSensitiveTest::new(config)?;
    let mut failures = FailureBreakdown::default();
    let mut any_record = false;

    for item in records {
        any_record = true;
        let (line, record) = item?;
        let mut baseline_status = None;
        let mut candidate_status = None;

        if let Some(status) = record.baseline_status.as_deref() {
            baseline_status = Some(tally_status(
                status,
                line,
                "baseline_status",
                &mut failures.baseline,
            )?);
        }
        if let Some(status) = record.candidate_status.as_deref() {
            candidate_status = Some(tally_status(
                status,
                line,
                "candidate_status",
                &mut failures.candidate,
            )?);
        }
        let used =
            baseline_status.is_some() || candidate_status.is_some() || record.result.is_some();

        if let Some(outcome) = effective_outcome(
            failure_policy,
            baseline_status,
            candidate_status,
            record.result.as_deref(),
            line,
        )? {
            test.update(outcome)?;
        }

        if !used {
            return Err(VeridictError::SchemaMismatch {
                line,
                context: "time_sensitive",
                detail: "record has no result and no status fields".to_string(),
            });
        }
    }

    if !any_record {
        return Err(VeridictError::EmptyInput);
    }

    let mut report = test.report();
    report.timeouts = failures.baseline.timeout + failures.candidate.timeout;
    report.crashes = failures.baseline.crash + failures.candidate.crash;
    report.invalid = failures.baseline.invalid + failures.candidate.invalid;
    report.failure_breakdown = failures;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hyp() -> BernoulliHypotheses {
        BernoulliHypotheses { p0: 0.5, p1: 0.55 }
    }

    fn config(policy: TimeSensitivePolicyKind, reward: RewardSchedule) -> TimeSensitiveConfig {
        TimeSensitiveConfig {
            hypotheses: hyp(),
            alpha: 0.05,
            reward,
            policy,
            action_grid_size: 21,
            wealth_grid_size: 100,
        }
    }

    fn rec(id: &str, result: &str) -> (usize, Record) {
        (
            0,
            Record {
                id: Some(id.to_string()),
                baseline: None,
                candidate: None,
                result: Some(result.to_string()),
                baseline_status: None,
                candidate_status: None,
            },
        )
    }

    // --- draw time-cost ---

    #[test]
    fn draw_advances_time_but_not_wealth() {
        let mut test = TimeSensitiveTest::new(config(
            TimeSensitivePolicyKind::Gro,
            RewardSchedule::HardDeadline { deadline: 100 },
        ))
        .unwrap();
        let step = test.update(Outcome::Draw).unwrap();
        assert_eq!(step.trial_count, 1);
        assert_eq!(step.log_wealth, 0.0);
        assert_eq!(test.decisive_count, 0);
        assert_eq!(test.draw_count, 1);
    }

    #[test]
    fn decisive_outcome_advances_wealth_and_time() {
        let mut test = TimeSensitiveTest::new(config(
            TimeSensitivePolicyKind::Gro,
            RewardSchedule::HardDeadline { deadline: 100 },
        ))
        .unwrap();
        let step = test.update(Outcome::CandidateWin).unwrap();
        assert_eq!(step.trial_count, 1);
        assert_eq!(test.decisive_count, 1);
        assert!(step.log_wealth > 0.0);
    }

    // --- optional stopping / idempotence ---

    #[test]
    fn update_after_rejection_is_a_no_op() {
        // alpha close enough to 1 that a single candidate win crosses immediately:
        // ln(p1/p0) = ln(1.1) ~= 0.0953 must exceed ln(1/alpha).
        let mut test = TimeSensitiveTest::new(TimeSensitiveConfig {
            alpha: 0.99,
            ..config(
                TimeSensitivePolicyKind::Gro,
                RewardSchedule::HardDeadline { deadline: 1000 },
            )
        })
        .unwrap();
        let first = test.update(Outcome::CandidateWin).unwrap();
        assert!(
            first.rejected,
            "expected an immediate crossing at alpha=0.99"
        );
        let after_1 = test.update(Outcome::CandidateWin).unwrap();
        let after_2 = test.update(Outcome::BaselineWin).unwrap();
        let after_3 = test.update(Outcome::Draw).unwrap();
        assert_eq!(after_1.log_wealth, first.log_wealth);
        assert_eq!(after_1.trial_count, first.trial_count);
        assert_eq!(after_1.rejection_time, first.rejection_time);
        assert_eq!(after_2.log_wealth, first.log_wealth);
        assert_eq!(after_3.trial_count, first.trial_count);
    }

    #[test]
    fn update_after_horizon_reached_is_a_no_op() {
        let mut test = TimeSensitiveTest::new(config(
            TimeSensitivePolicyKind::Gro,
            RewardSchedule::HardDeadline { deadline: 2 },
        ))
        .unwrap();
        test.update(Outcome::BaselineWin).unwrap();
        let at_horizon = test.update(Outcome::BaselineWin).unwrap();
        assert_eq!(at_horizon.trial_count, 2);
        assert!(!at_horizon.rejected);
        let after = test.update(Outcome::CandidateWin).unwrap();
        assert_eq!(after.trial_count, 2);
        assert_eq!(after.log_wealth, at_horizon.log_wealth);
        assert_eq!(test.report().verdict, Verdict::Inconclusive);
    }

    // --- verdict is never Fail (one-sided test) ---

    #[test]
    fn verdict_is_never_fail() {
        let mut losing = TimeSensitiveTest::new(config(
            TimeSensitivePolicyKind::Gro,
            RewardSchedule::HardDeadline { deadline: 20 },
        ))
        .unwrap();
        for _ in 0..20 {
            losing.update(Outcome::BaselineWin).unwrap();
        }
        assert_ne!(losing.report().verdict, Verdict::Fail);

        let mut winning = TimeSensitiveTest::new(config(
            TimeSensitivePolicyKind::Gro,
            RewardSchedule::HardDeadline { deadline: 20 },
        ))
        .unwrap();
        for _ in 0..20 {
            winning.update(Outcome::CandidateWin).unwrap();
        }
        assert_ne!(winning.report().verdict, Verdict::Fail);
    }

    // --- reproducibility ---

    #[test]
    fn same_config_and_input_produce_byte_identical_json() {
        let outcomes = [
            Outcome::CandidateWin,
            Outcome::Draw,
            Outcome::BaselineWin,
            Outcome::CandidateWin,
            Outcome::CandidateWin,
        ];
        let run_once = || {
            let mut test = TimeSensitiveTest::new(config(
                TimeSensitivePolicyKind::BellmanGrid,
                RewardSchedule::HardDeadline { deadline: 50 },
            ))
            .unwrap();
            for &o in &outcomes {
                test.update(o).unwrap();
            }
            test.report().to_json_pretty()
        };
        assert_eq!(run_once(), run_once());
    }

    // --- GRO / SPRT betting equivalence (independent implementation, same bet) ---

    #[test]
    fn gro_wealth_update_matches_sprt_llr_delta() {
        let h = hyp();
        let candidate_win_log_phi = e_variable::log_phi(e_variable::gro_action(&h), h.p0, true);
        let baseline_win_log_phi = e_variable::log_phi(e_variable::gro_action(&h), h.p0, false);
        assert!(
            (candidate_win_log_phi - crate::stats::sprt::llr_delta(true, h.p0, h.p1)).abs() < 1e-12
        );
        assert!(
            (baseline_win_log_phi - crate::stats::sprt::llr_delta(false, h.p0, h.p1)).abs() < 1e-12
        );
    }

    // --- numerical boundaries ---

    #[test]
    fn rejects_invalid_alpha() {
        for &alpha in &[0.0, 1.0, -0.1, 1.1, f64::NAN, f64::INFINITY] {
            let mut c = config(
                TimeSensitivePolicyKind::Gro,
                RewardSchedule::HardDeadline { deadline: 10 },
            );
            c.alpha = alpha;
            assert!(
                TimeSensitiveTest::new(c).is_err(),
                "alpha={alpha} should be rejected"
            );
        }
    }

    #[test]
    fn rejects_undersized_grids() {
        let mut c = config(
            TimeSensitivePolicyKind::Gro,
            RewardSchedule::HardDeadline { deadline: 10 },
        );
        c.wealth_grid_size = 1;
        assert!(TimeSensitiveTest::new(c).is_err());

        let mut c = config(
            TimeSensitivePolicyKind::Gro,
            RewardSchedule::HardDeadline { deadline: 10 },
        );
        c.action_grid_size = 0;
        assert!(TimeSensitiveTest::new(c).is_err());
    }

    #[test]
    fn rejects_edo_with_non_exponential_reward() {
        let c = config(
            TimeSensitivePolicyKind::Edo,
            RewardSchedule::HardDeadline { deadline: 10 },
        );
        assert!(matches!(
            TimeSensitiveTest::new(c),
            Err(VeridictError::InvalidThreshold(_))
        ));
    }

    #[test]
    fn rejects_a_grid_cost_estimate_above_the_cap() {
        let mut c = config(
            TimeSensitivePolicyKind::BellmanGrid,
            RewardSchedule::HardDeadline {
                deadline: 10_000_000,
            },
        );
        c.wealth_grid_size = 10_000;
        c.action_grid_size = 10_000;
        assert!(matches!(
            TimeSensitiveTest::new(c),
            Err(VeridictError::TimeSensitiveGridTooLarge { .. })
        ));
    }

    #[test]
    fn rejects_edo_time_scale_below_existence_threshold() {
        let c = config(
            TimeSensitivePolicyKind::Edo,
            RewardSchedule::ExponentialDecay {
                time_scale: 0.01,
                horizon: 100,
            },
        );
        assert!(matches!(
            TimeSensitiveTest::new(c),
            Err(VeridictError::EdoRootDoesNotExist { .. })
        ));
    }

    // --- run(): failure caps, schema guard, empty input ---

    #[test]
    fn run_applies_failure_caps_and_forces_inconclusive() {
        let records = vec![rec("a", "candidate_win"), rec("b", "candidate_win")];
        let report = run(
            records.into_iter().map(Ok),
            config(
                TimeSensitivePolicyKind::Gro,
                RewardSchedule::HardDeadline { deadline: 10 },
            ),
            FailurePolicy::ReportOnly,
        )
        .unwrap();
        assert_eq!(report.verdict, Verdict::Inconclusive);
    }

    #[test]
    fn run_rejects_a_record_with_no_status_or_result() {
        let records = vec![(
            1,
            Record {
                id: Some("x".to_string()),
                baseline: None,
                candidate: None,
                result: None,
                baseline_status: None,
                candidate_status: None,
            },
        )];
        let result = run(
            records.into_iter().map(Ok),
            config(
                TimeSensitivePolicyKind::Gro,
                RewardSchedule::HardDeadline { deadline: 10 },
            ),
            FailurePolicy::ReportOnly,
        );
        assert!(matches!(result, Err(VeridictError::SchemaMismatch { .. })));
    }

    #[test]
    fn run_rejects_empty_input() {
        let records: Vec<Result<(usize, Record), VeridictError>> = Vec::new();
        let result = run(
            records,
            config(
                TimeSensitivePolicyKind::Gro,
                RewardSchedule::HardDeadline { deadline: 10 },
            ),
            FailurePolicy::ReportOnly,
        );
        assert!(matches!(result, Err(VeridictError::EmptyInput)));
    }
}
