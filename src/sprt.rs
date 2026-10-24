//! Sequential probability ratio test: accumulate a log-likelihood ratio
//! over decisive trials and stop as soon as it crosses one of Wald's
//! boundaries. Unlike `compare`, there's no confidence interval or
//! threshold to configure - the test's own alpha/beta *are* the guaranteed
//! error rates, by construction (see `stats::sprt` for the math and its
//! documented "decisive games only" assumption).
//!
//! Every variant performs genuine sequential monitoring. Input order is the trial schedule:
//! `Wald` and `Trinomial` evaluate after each usable outcome (or each completed net pair under
//! `--paired-by-id`), while `Pentanomial` evaluates after each completed pair. Reordering an input
//! file can therefore legitimately change its verdict. The report keeps the pre-0.20 full-input
//! `llr`/win/loss/draw fields for compatibility and exposes the analyzed prefix separately through
//! `decision_llr`, `decision_*` counts, and the generic observation/stopping fields.
//!
//! `Pentanomial` recomputes the generalized LLR from the cumulative bucket counts after every pair
//! (a genuine re-tilt of the whole empirical distribution, not an additive accumulation - see
//! `stats::pentanomial_sprt`'s module doc). `--min-paired-ids`/
//! `--max-paired-ids` are folded into that same walk rather than applied as a post-hoc gate on
//! the final aggregate: a boundary crossed before the minimum is reached is never evaluated at
//! all (so there's nothing to "remember" once the minimum is reached), and pairs completed after
//! the walk's own stopping point are never folded into the buckets/LLR a report is built from.
//! See `SprtReport`'s `stopping_pair_count`/`stopping_reason`/`ignored_pairs_after_stop` for how
//! a truncated analysis is surfaced.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::error::VeridictError;
use crate::input::Record;
use crate::metrics::{FailureBreakdown, effective_outcome, tally_status};
use crate::stats::pentanomial_sprt;
use crate::stats::sprt as math;
use crate::stats::trinomial_sprt;
use crate::{FailureCaps, FailurePolicy, Outcome, Promotion, Validity, Verdict};

mod accumulator;
mod types;

pub use accumulator::*;
pub use types::*;

use accumulator::{outcome_llr, require_finite_sprt};

/// Strict pairing for `SprtVariant::Pentanomial`: every id must resolve to exactly two records.
/// Rejecting incomplete groups avoids mixing bias-cancelled pairs with lone observations.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct PentanomialCollector {
    groups: HashMap<String, Vec<(usize, Outcome)>>,
    /// `(completion_line, bucket)` pushed in the order each id's *second* record arrives - i.e.
    /// the order pairs complete while walking the input. This doubles as the sequential test's
    /// replay order (see the module doc and `evaluate_pentanomial_sequential`): sprt has no
    /// `schedule` field of its own, so input order *is* the schedule.
    completed_in_order: Vec<(usize, usize)>,
}

/// Combined candidate score bucket (0..=4) for one pentanomial pair - `win=1`/`draw=0.5`/
/// `loss=0` points each, matching `OutcomeCollector::finish`'s convention and
/// `stats::pentanomial_sprt`'s `i / 4.0` category scale.
fn pentanomial_bucket(a: Outcome, b: Outcome) -> usize {
    let points = |o: Outcome| -> f64 {
        match o {
            Outcome::CandidateWin => 1.0,
            Outcome::Draw => 0.5,
            Outcome::BaselineWin => 0.0,
        }
    };
    ((points(a) + points(b)) * 2.0).round() as usize
}

impl PentanomialCollector {
    fn new() -> Self {
        Self {
            groups: HashMap::new(),
            completed_in_order: Vec::new(),
        }
    }

    fn record(&mut self, line: usize, id: &str, outcome: Outcome) {
        let entry = self.groups.entry(id.to_string()).or_default();
        entry.push((line, outcome));
        if entry.len() == 2 {
            self.completed_in_order
                .push((line, pentanomial_bucket(entry[0].1, entry[1].1)));
        }
    }

    /// `(completed pairs as (completion_line, bucket) in completion order, raw trial count)`.
    /// Validates every id resolved to exactly 2 records - a lone or tripled id is a hard error
    /// regardless of `--require-complete-pairs` (see the struct doc above).
    fn finish(self) -> Result<(Vec<(usize, usize)>, u64), VeridictError> {
        let mut raw_trial_count = 0u64;
        for (id, group) in &self.groups {
            raw_trial_count += group.len() as u64;
            match group.as_slice() {
                [(line, _)] => {
                    return Err(VeridictError::SchemaMismatch {
                        line: *line,
                        context: "pentanomial",
                        detail: format!(
                            "id '{id}' appears once; --sprt-variant pentanomial requires \
                             exactly 2 records per id (a lone game can't cancel the pair's own \
                             bias)"
                        ),
                    });
                }
                [(_, _), (_, _)] => {}
                more => {
                    return Err(VeridictError::SchemaMismatch {
                        line: more[0].0,
                        context: "pentanomial",
                        detail: format!(
                            "id '{id}' appears {} times; pentanomial mode expects exactly 2 \
                             records per id",
                            more.len()
                        ),
                    });
                }
            }
        }
        Ok((self.completed_in_order, raw_trial_count))
    }
}

/// Result of walking a pentanomial run's completed pairs in completion order, recomputing the
/// generalized LLR from cumulative bucket counts after each pair - see the module doc for why
/// this is a genuine sequential replay rather than an additive shortcut, and why
/// `min_paired_ids`/`max_paired_ids` are applied inline rather than as a post-hoc gate.
struct PentanomialWalk {
    /// Bucket counts folded in up to (and including) the stopping point only.
    buckets: [u64; 5],
    /// LLR at the stopping point. Deriving `verdict` from this via the same
    /// `llr`-vs-`bounds` comparison every variant uses is always consistent with how this walk
    /// stopped: a crossing return only ever happens exactly at a boundary, and every other
    /// return (max reached or ran out of data) only happens when `llr` is strictly within
    /// bounds - see the function body.
    llr: f64,
    available_paired_count: u64,
    analyzed_paired_count: u64,
    stopping_pair_count: Option<u64>,
    stopping_reason: &'static str,
}

fn evaluate_pentanomial_sequential(
    completed_in_order: &[(usize, usize)],
    config: &SprtConfig,
    bounds: &math::SprtBounds,
    min_paired_ids: Option<u64>,
    max_paired_ids: Option<u64>,
) -> Result<PentanomialWalk, VeridictError> {
    let available_paired_count = completed_in_order.len() as u64;
    let effective_min = min_paired_ids.unwrap_or(0);
    let mut buckets = [0u64; 5];

    for (index, &(_, bucket)) in completed_in_order.iter().enumerate() {
        buckets[bucket] += 1;
        let paired_count = (index + 1) as u64;
        let llr = pentanomial_sprt::pentanomial_llr(config.elo0, config.elo1, &buckets);
        require_finite_sprt("llr", llr)?;

        if paired_count >= effective_min {
            if llr >= bounds.upper {
                return Ok(PentanomialWalk {
                    buckets,
                    llr,
                    available_paired_count,
                    analyzed_paired_count: paired_count,
                    stopping_pair_count: Some(paired_count),
                    stopping_reason: "upper_bound_crossed",
                });
            }
            if llr <= bounds.lower {
                return Ok(PentanomialWalk {
                    buckets,
                    llr,
                    available_paired_count,
                    analyzed_paired_count: paired_count,
                    stopping_pair_count: Some(paired_count),
                    stopping_reason: "lower_bound_crossed",
                });
            }
        }
        if let Some(max) = max_paired_ids
            && paired_count >= max
        {
            return Ok(PentanomialWalk {
                buckets,
                llr,
                available_paired_count,
                analyzed_paired_count: paired_count,
                stopping_pair_count: Some(paired_count),
                stopping_reason: "max_paired_ids_reached",
            });
        }
    }

    let llr = pentanomial_sprt::pentanomial_llr(config.elo0, config.elo1, &buckets);
    require_finite_sprt("llr", llr)?;
    Ok(PentanomialWalk {
        buckets,
        llr,
        available_paired_count,
        analyzed_paired_count: available_paired_count,
        stopping_pair_count: None,
        stopping_reason: "insufficient_data",
    })
}

/// `(baseline_wins, candidate_wins, draws)` netted from a pentanomial bucket breakdown, the
/// same ">1/=1/<1 total points" convention `OutcomeCollector::finish` and the "Paired
/// testcases" README section already document - keeps `SprtReport`'s existing 3-outcome fields
/// meaningful for `Pentanomial` too, instead of left at `0`.
fn net_pentanomial_buckets(buckets: &[u64; 5]) -> (u64, u64, u64) {
    let baseline_wins = buckets[0] + buckets[1];
    let draws = buckets[2];
    let candidate_wins = buckets[3] + buckets[4];
    (baseline_wins, candidate_wins, draws)
}

/// One Wald/Trinomial analysis unit in schedule order. With `--paired-by-id`, a unit is the
/// net result of two records and is scheduled when the second record completes the pair.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
struct OrderedOutcome {
    completion_line: usize,
    outcome: Outcome,
    raw_record_count: u64,
}

/// Order-preserving counterpart to `metrics::OutcomeCollector`, used only by SPRT because a
/// sequential test must retain its schedule. This deliberately keeps the existing pairing
/// contract: id-less records are single observations, a lone id is tolerated unless
/// `require_complete_pairs` is set, and an id occurring more than twice is invalid.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct SequentialOutcomeCollector {
    paired_by_id: bool,
    require_complete_pairs: bool,
    observations: Vec<OrderedOutcome>,
    groups: HashMap<String, Vec<(usize, Outcome)>>,
}

impl SequentialOutcomeCollector {
    fn new(paired_by_id: bool, require_complete_pairs: bool) -> Self {
        Self {
            paired_by_id,
            require_complete_pairs,
            observations: Vec::new(),
            groups: HashMap::new(),
        }
    }

    fn record(&mut self, line: usize, id: Option<&str>, outcome: Outcome) {
        if self.paired_by_id
            && let Some(id) = id
        {
            let group = self.groups.entry(id.to_string()).or_default();
            group.push((line, outcome));
            if group.len() == 2 {
                self.observations.push(OrderedOutcome {
                    completion_line: line,
                    outcome: net_outcomes(group[0].1, group[1].1),
                    raw_record_count: 2,
                });
            }
        } else {
            self.observations.push(OrderedOutcome {
                completion_line: line,
                outcome,
                raw_record_count: 1,
            });
        }
    }

    fn finish(mut self) -> Result<Vec<OrderedOutcome>, VeridictError> {
        for (id, group) in self.groups {
            match group.as_slice() {
                [(line, outcome)] => {
                    if self.require_complete_pairs {
                        return Err(VeridictError::SchemaMismatch {
                            line: *line,
                            context: "paired-by-id",
                            detail: format!(
                                "id '{id}' appears once; --require-complete-pairs requires \
                                 exactly 2 records per id"
                            ),
                        });
                    }
                    self.observations.push(OrderedOutcome {
                        completion_line: *line,
                        outcome: *outcome,
                        raw_record_count: 1,
                    });
                }
                [(_, _), (_, _)] => {}
                more => {
                    return Err(VeridictError::SchemaMismatch {
                        line: more[0].0,
                        context: "paired-by-id",
                        detail: format!(
                            "id '{id}' appears {} times; paired mode expects at most 2 records per id",
                            more.len()
                        ),
                    });
                }
            }
        }
        self.observations
            .sort_by_key(|observation| observation.completion_line);
        Ok(self.observations)
    }
}

fn net_outcomes(a: Outcome, b: Outcome) -> Outcome {
    let points = |outcome| match outcome {
        Outcome::CandidateWin => 2,
        Outcome::Draw => 1,
        Outcome::BaselineWin => 0,
    };
    match points(a) + points(b) {
        0 | 1 => Outcome::BaselineWin,
        2 => Outcome::Draw,
        3 | 4 => Outcome::CandidateWin,
        _ => unreachable!("two outcomes can only produce 0 through 4 half-points"),
    }
}

fn tally_outcome(
    outcome: Outcome,
    candidate_wins: &mut u64,
    baseline_wins: &mut u64,
    draws: &mut u64,
) {
    match outcome {
        Outcome::CandidateWin => *candidate_wins += 1,
        Outcome::BaselineWin => *baseline_wins += 1,
        Outcome::Draw => *draws += 1,
    }
}

struct OutcomeWalk {
    llr: f64,
    drawelo: Option<f64>,
    candidate_wins: u64,
    baseline_wins: u64,
    draws: u64,
    available_observation_count: u64,
    analyzed_observation_count: u64,
    stopping_observation_count: Option<u64>,
    stopping_reason: &'static str,
    available_record_count: u64,
    analyzed_record_count: u64,
}

struct RunAnalysis {
    /// Historical full-input fields. Pentanomial has always exposed only its analyzed prefix.
    llr: f64,
    drawelo: Option<f64>,
    candidate_wins: u64,
    baseline_wins: u64,
    draws: u64,
    /// Sequential decision prefix.
    decision_llr: f64,
    decision_drawelo: Option<f64>,
    decision_candidate_wins: u64,
    decision_baseline_wins: u64,
    decision_draws: u64,
    analysis_unit: &'static str,
    available_observation_count: u64,
    analyzed_observation_count: u64,
    stopping_observation_count: Option<u64>,
    stopping_observation_reason: &'static str,
    available_record_count: u64,
    analyzed_record_count: u64,
    pentanomial: Option<(PentanomialWalk, u64)>,
}

/// Incremental Wald/Trinomial state for the common unpaired path. Full-input counts continue to
/// advance after the first boundary crossing, while the decision-prefix counts freeze there.
/// This preserves the report contract without retaining one `OrderedOutcome` per trial.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum SequentialStop {
    UpperBoundCrossed,
    LowerBoundCrossed,
}

impl SequentialStop {
    const fn label(self) -> &'static str {
        match self {
            Self::UpperBoundCrossed => "upper_bound_crossed",
            Self::LowerBoundCrossed => "lower_bound_crossed",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
struct UnpairedOutcomeAccumulator {
    candidate_wins: u64,
    baseline_wins: u64,
    draws: u64,
    decision_candidate_wins: u64,
    decision_baseline_wins: u64,
    decision_draws: u64,
    available_observation_count: u64,
    analyzed_observation_count: u64,
    stopping_observation_count: Option<u64>,
    stopping_reason: Option<SequentialStop>,
}

impl UnpairedOutcomeAccumulator {
    fn record(
        &mut self,
        outcome: Outcome,
        config: &SprtConfig,
        variant: SprtVariant,
        bounds: &math::SprtBounds,
    ) -> Result<(), VeridictError> {
        tally_outcome(
            outcome,
            &mut self.candidate_wins,
            &mut self.baseline_wins,
            &mut self.draws,
        );
        self.available_observation_count += 1;

        if self.stopping_reason.is_some() {
            return Ok(());
        }

        tally_outcome(
            outcome,
            &mut self.decision_candidate_wins,
            &mut self.decision_baseline_wins,
            &mut self.decision_draws,
        );
        self.analyzed_observation_count += 1;
        let (llr, _) = outcome_llr(
            variant,
            config,
            self.decision_candidate_wins,
            self.decision_baseline_wins,
            self.decision_draws,
        )?;
        self.stopping_reason = if llr >= bounds.upper {
            Some(SequentialStop::UpperBoundCrossed)
        } else if llr <= bounds.lower {
            Some(SequentialStop::LowerBoundCrossed)
        } else {
            None
        };
        if self.stopping_reason.is_some() {
            self.stopping_observation_count = Some(self.analyzed_observation_count);
        }
        Ok(())
    }

    fn finish(
        self,
        config: &SprtConfig,
        variant: SprtVariant,
    ) -> Result<RunAnalysis, VeridictError> {
        let (llr, drawelo) = outcome_llr(
            variant,
            config,
            self.candidate_wins,
            self.baseline_wins,
            self.draws,
        )?;
        let (decision_llr, decision_drawelo) = outcome_llr(
            variant,
            config,
            self.decision_candidate_wins,
            self.decision_baseline_wins,
            self.decision_draws,
        )?;
        Ok(RunAnalysis {
            llr,
            drawelo,
            candidate_wins: self.candidate_wins,
            baseline_wins: self.baseline_wins,
            draws: self.draws,
            decision_llr,
            decision_drawelo,
            decision_candidate_wins: self.decision_candidate_wins,
            decision_baseline_wins: self.decision_baseline_wins,
            decision_draws: self.decision_draws,
            analysis_unit: "trial",
            available_observation_count: self.available_observation_count,
            analyzed_observation_count: self.analyzed_observation_count,
            stopping_observation_count: self.stopping_observation_count,
            stopping_observation_reason: self
                .stopping_reason
                .map(SequentialStop::label)
                .unwrap_or("insufficient_data"),
            available_record_count: self.available_observation_count,
            analyzed_record_count: self.analyzed_observation_count,
            pentanomial: None,
        })
    }
}

/// Compatibility entry point for callers using the original positional API.
///
/// New integrations should prefer [`run_with_options`]. This wrapper constructs
/// [`SprtRunOptions`] and delegates to the same validation and execution path, so report and error
/// behavior remain identical.
#[allow(clippy::too_many_arguments)]
pub fn run<I>(
    records: I,
    config: &SprtConfig,
    variant: SprtVariant,
    paired_by_id: bool,
    failure_policy: FailurePolicy,
    require_complete_pairs: bool,
    min_paired_ids: Option<u64>,
    max_paired_ids: Option<u64>,
) -> Result<SprtReport, VeridictError>
where
    I: IntoIterator<Item = Result<(usize, Record), VeridictError>>,
{
    run_with_options(
        records,
        config,
        SprtRunOptions {
            variant,
            paired_by_id,
            failure_policy,
            require_complete_pairs,
            min_paired_ids,
            max_paired_ids,
        },
    )
}

/// Runs an SPRT with a typed set of execution policies.
///
/// `paired_by_id`: see `metrics::compute` - two records sharing an `id` are
/// combined into one net observation (by total points across the pair)
/// instead of two independent trials. Unpaired Wald/Trinomial runs update their full-input and
/// decision-prefix counts incrementally. Paired runs retain the reduced schedule until every id
/// has been validated, because incomplete and overfull groups can invalidate the input.
///
/// `SprtVariant::Pentanomial` always requires `paired_by_id`: rejected up front rather than
/// silently ignored, matching `resolve_sprt_hypotheses`'s existing "never silently ignore
/// invalid data" precedent for the `--elo0`/`--belo0` cross-variant flags.
///
/// `failure_policy`: see `metrics::effective_outcome` (the same shared resolver `winrate`/`elo`
/// use) - `FailurePolicy::Loss`'s synthesized outcome flows into whichever collector `variant`
/// uses exactly like a literal `result` would, `Pentanomial` included: a crash on one side of a
/// pair nets against its partner's real result the same way any other outcome pair would.
///
/// `require_complete_pairs`: only affects `Wald`/`Trinomial` (upgrades a lone id under
/// `paired_by_id` from a tolerated unpaired sample into a hard error) -
/// a documented no-op for `Pentanomial`, which is already unconditionally this strict via
/// `PentanomialCollector` regardless of this flag.
///
/// `min_paired_ids`/`max_paired_ids`: only meaningful for `SprtVariant::Pentanomial` (the only
/// variant with a `paired_count` at all) - rejected up front for any other variant, same
/// "never silently ignore invalid data" precedent as the `paired_by_id` check above. Folded
/// directly into the sequential walk (see the module doc and `evaluate_pentanomial_sequential`),
/// not applied as a post-hoc gate on the final aggregate.
pub fn run_with_options<I>(
    records: I,
    config: &SprtConfig,
    options: SprtRunOptions,
) -> Result<SprtReport, VeridictError>
where
    I: IntoIterator<Item = Result<(usize, Record), VeridictError>>,
{
    let mut accumulator = SprtAccumulator::new(*config, options)?;
    for item in records {
        let (line, record) = item?;
        // Preserve the batch API's historical contract: iterator order defines the sequential
        // schedule, while `line` is diagnostic metadata and need not be monotonic. Incremental
        // callers use `SprtAccumulator::push`, which enforces a durable stream order.
        accumulator.push_batch(line, record)?;
    }
    accumulator.finish()
}

/// Applies the same technical-failure gate as fixed-sample reports while retaining SPRT-specific
/// report mutation in this module.
pub fn apply_failure_caps(report: &mut SprtReport, caps: &FailureCaps) {
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

#[cfg(test)]
mod tests;
