//! `TimeSensitiveReport`: this module's own report shape, independent of `report::Report`/
//! `MultiReport`/`sprt::SprtReport` (different questions need different fields), but reusing the
//! crate-wide `report::REPORT_SCHEMA_VERSION` for its `schema_version` field, the same convention
//! every other report struct in this crate follows (`SprtReport`, `VerifyRunReport`).

use serde::Serialize;

use crate::metrics::FailureBreakdown;
use crate::report::serde_str;
use crate::{FailureCaps, Promotion, Validity, Verdict};

#[derive(Debug, Serialize)]
pub struct TimeSensitiveReport {
    pub schema_version: u32,
    /// One-directional: `Pass` (wealth crossed `1/alpha`) or `Inconclusive` (horizon reached, or
    /// a `FailureCaps` breach - see `apply_failure_caps`). Never `Fail` - there is no lower
    /// rejection boundary in this model, so "not yet rejected" is never itself evidence for H0,
    /// only an absence of evidence for H1 so far. See `TimeSensitiveTest`'s doc for why a
    /// horizon-reached non-rejection must not be read as a statistical failure.
    pub verdict: Verdict,
    pub validity: Validity,
    pub promotion: Promotion,
    /// `bellman_grid_approximation` / `edo_stationary_approximation` / `gro` - deliberately never
    /// "optimal": see `bellman.rs`/`edo.rs`'s module docs for why each is a named approximation,
    /// not a proof of optimality.
    pub method: &'static str,
    /// The `--policy` selection itself (`bellman`/`edo`/`gro`) - kept separate from `method` so a
    /// machine consumer can filter/group by policy choice without string-matching `method`'s more
    /// descriptive, caveat-laden label.
    pub policy_kind: &'static str,
    /// Decisive-observation-conditional success probabilities - NOT unconditional per-trial
    /// probabilities. A `Draw` (see `TimeSensitiveTest::update`) advances `trial_count` without
    /// being a Bernoulli(p0)/Bernoulli(p1) observation at all.
    pub p0: f64,
    pub p1: f64,
    pub alpha: f64,
    pub reward_kind: &'static str,
    pub reward_parameters: serde_json::Value,
    /// Every observation that reached `TimeSensitiveTest::update` (decisive or draw) - see the
    /// module doc for why a record that resolves to no `Outcome` at all under the active
    /// `FailurePolicy` never reaches `update` and so isn't counted here either.
    pub trial_count: u64,
    pub decisive_count: u64,
    /// Draws advance `trial_count` (and the reward clock) without moving `log_wealth` - a draw is
    /// still a consumed trial, not a free one (see `TimeSensitiveTest::update`'s doc).
    pub draw_count: u64,
    pub log_wealth: f64,
    /// `1/alpha` (not `log_wealth`'s own threshold, `ln(1/alpha)`) - the wealth-space number this
    /// module's CLI help text and the paper both state the rejection rule in.
    pub wealth_threshold: f64,
    /// `Some` only once wealth has crossed `wealth_threshold`; `null` otherwise. Never treat a
    /// `null` here as "failed" - see `verdict`'s doc.
    pub rejection_time: Option<u64>,
    pub reward_at_rejection: Option<f64>,
    /// Expected reward under `p1` of running this policy to completion, computed *exactly* (via
    /// `grid::evaluate_fixed_action`/`grid::maximize`'s own value, not simulated) under an
    /// all-decisive idealization: the value recursion only has success/failure transitions
    /// weighted by `p1` and has no draw-rate input, so on a draw-heavy input stream the realized
    /// reward will lag this number - draws burn time (and therefore reward-schedule budget)
    /// without the recursion ever seeing them coming.
    pub planned_expected_reward_under_p1: f64,
    pub action_grid_size: usize,
    pub wealth_grid_size: usize,
    /// The log-wealth grid's step size - the dominant source of numerical imprecision in
    /// `planned_expected_reward_under_p1` (and, for `bellman`, in the executed policy itself)
    /// across all three policy kinds uniformly. `edo` additionally solves its stationary action
    /// via bisection to a separate, much tighter tolerance (see `edo.rs`'s `BISECTION_TOLERANCE`)
    /// - negligible next to this grid-driven figure, so not surfaced as its own field.
    pub numerical_tolerance: f64,
    pub timeouts: u64,
    pub crashes: u64,
    pub invalid: u64,
    pub failure_breakdown: FailureBreakdown,
    pub reason: String,
    pub notes: Vec<String>,
}

/// Mirrors `sprt::apply_failure_caps`/`verdict::apply_failure_caps` - its own copy for this
/// report type rather than a shared trait (same repo-wide precedent: one small gate, one copy
/// per report shape). A cap breach forces `Inconclusive` (never `Fail` - see `verdict`'s doc) and
/// `Validity::Invalid`, and always lands on the CLI's own exit code 2 through the ordinary
/// verdict mapping (there is no separate "invalid" exit code here, unlike `verify-run`'s binary
/// valid/invalid scheme).
pub fn apply_failure_caps(report: &mut TimeSensitiveReport, caps: &FailureCaps) {
    let (validity, reason) = caps.check(report.timeouts, report.crashes, report.invalid);
    report.validity = validity;
    if validity == Validity::Invalid {
        report.verdict = Verdict::Inconclusive;
        if let Some(reason) = reason {
            report.reason = format!("INVALID: {reason}. Strength not evaluated.");
        }
    }
    report.promotion = Promotion::decide(report.validity, report.verdict);
}

impl TimeSensitiveReport {
    pub fn to_json_pretty(&self) -> String {
        serde_json::to_string_pretty(self).expect(
            "TimeSensitiveReport contains only finite fields and strings; serialization cannot fail",
        )
    }

    pub fn to_markdown(&self) -> String {
        let mut notes = String::new();
        for n in &self.notes {
            notes.push_str(&format!("- {n}\n"));
        }
        format!(
            "# Veridict Time-Sensitive Report\n\n\
             Verdict: {verdict} (validity: {validity}, promotion: {promotion})\n\n\
             {reason}\n\n\
             Method: {method} (policy: {policy_kind})\n\
             p0={p0}, p1={p1}, alpha={alpha}\n\
             Reward: {reward_kind} {reward_parameters}\n\n\
             Trials: {trial_count} (decisive: {decisive_count}, draws: {draw_count})\n\
             log_wealth={log_wealth:.4}, wealth_threshold={wealth_threshold:.4}\n\
             rejection_time={rejection_time:?}, reward_at_rejection={reward_at_rejection:?}\n\
             planned_expected_reward_under_p1={planned_expected_reward_under_p1:.6}\n\n\
             Grid: action_grid_size={action_grid_size}, wealth_grid_size={wealth_grid_size}, \
             numerical_tolerance={numerical_tolerance:.3e}\n\n\
             Status counts: timeout={timeouts}, crash={crashes}, invalid={invalid}\n\n\
             Notes:\n{notes}",
            verdict = serde_str(&self.verdict),
            validity = serde_str(&self.validity),
            promotion = serde_str(&self.promotion),
            reason = self.reason,
            method = self.method,
            policy_kind = self.policy_kind,
            p0 = self.p0,
            p1 = self.p1,
            alpha = self.alpha,
            reward_kind = self.reward_kind,
            reward_parameters = self.reward_parameters,
            trial_count = self.trial_count,
            decisive_count = self.decisive_count,
            draw_count = self.draw_count,
            log_wealth = self.log_wealth,
            wealth_threshold = self.wealth_threshold,
            rejection_time = self.rejection_time,
            reward_at_rejection = self.reward_at_rejection,
            planned_expected_reward_under_p1 = self.planned_expected_reward_under_p1,
            action_grid_size = self.action_grid_size,
            wealth_grid_size = self.wealth_grid_size,
            numerical_tolerance = self.numerical_tolerance,
            timeouts = self.timeouts,
            crashes = self.crashes,
            invalid = self.invalid,
            notes = notes,
        )
    }
}
