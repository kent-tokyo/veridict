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

use serde::Serialize;

use crate::error::VeridictError;
use crate::input::Record;
use crate::metrics::{FailureBreakdown, effective_outcome, tally_status};
use crate::report::serde_str;
use crate::stats::pentanomial_sprt;
use crate::stats::sprt as math;
use crate::stats::trinomial_sprt;
use crate::{FailureCaps, FailurePolicy, Outcome, Promotion, Validity, Verdict};

/// Which SPRT is run. `Wald` (default): classic two-outcome test, draws
/// excluded, `elo0`/`elo1` are logistic Elo (`stats::sprt::score_from_elo`).
/// `Trinomial`: draw rate estimated as a nuisance parameter from the pooled
/// counts (see `stats::trinomial_sprt`), `elo0`/`elo1` are BayesElo instead,
/// a different scale whenever the estimated draw rate is nonzero; that's
/// why the CLI exposes this through separate `--belo0`/`--belo1` flags
/// rather than reinterpreting `--elo0`/`--elo1`. `Pentanomial`: paired-game
/// (two games sharing an id, e.g. same opening with colors swapped) test
/// over the pair's 5-value combined score instead of two individual
/// win/loss/draw outcomes (see `stats::pentanomial_sprt`'s doc for why this
/// isn't just trinomial run on twice as many games) - `elo0`/`elo1` are
/// logistic Elo, the same scale as `Wald` (this model has no drawelo-style
/// nuisance parameter to make BayesElo meaningful), and it always requires
/// `--paired-by-id` (a 5-value pair score has no meaning for a lone game).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SprtVariant {
    Wald,
    Trinomial,
    Pentanomial,
}

impl SprtVariant {
    fn label(self) -> &'static str {
        match self {
            SprtVariant::Wald => "wald",
            SprtVariant::Trinomial => "trinomial",
            SprtVariant::Pentanomial => "pentanomial",
        }
    }
}

pub struct SprtConfig {
    /// H0 hypothesis gap. Logistic Elo for `SprtVariant::Wald`/`Pentanomial`,
    /// BayesElo for `SprtVariant::Trinomial` - see `SprtVariant`'s doc for
    /// why trinomial alone is a different scale.
    pub elo0: f64,
    pub elo1: f64,
    pub alpha: f64,
    pub beta: f64,
}

impl SprtConfig {
    pub fn new(elo0: f64, elo1: f64, alpha: f64, beta: f64) -> Result<Self, VeridictError> {
        if !elo0.is_finite() || !elo1.is_finite() {
            return Err(VeridictError::InvalidThreshold(
                "elo0/elo1 must be finite".to_string(),
            ));
        }
        if elo0 >= elo1 {
            return Err(VeridictError::InvalidThreshold(format!(
                "elo0 ({elo0}) must be less than elo1 ({elo1})"
            )));
        }
        for (name, v) in [("alpha", alpha), ("beta", beta)] {
            if !v.is_finite() || v <= 0.0 || v >= 1.0 {
                return Err(VeridictError::InvalidThreshold(format!(
                    "{name} must be finite and in (0, 1), got {v}"
                )));
            }
        }
        Ok(Self {
            elo0,
            elo1,
            alpha,
            beta,
        })
    }
}

/// Breakdown of pentanomial pairs by combined candidate score across a pair's two games -
/// `Some` only for `SprtVariant::Pentanomial`. Field names spell out the score rather than
/// using an array/index, since "which bucket is index 2" isn't self-describing in JSON the way
/// a named field is.
#[derive(Debug, Serialize)]
pub struct PentanomialCounts {
    pub score_0_0: u64,
    pub score_0_5: u64,
    pub score_1_0: u64,
    pub score_1_5: u64,
    pub score_2_0: u64,
}

impl PentanomialCounts {
    fn from_buckets(buckets: [u64; 5]) -> Self {
        Self {
            score_0_0: buckets[0],
            score_0_5: buckets[1],
            score_1_0: buckets[2],
            score_1_5: buckets[3],
            score_2_0: buckets[4],
        }
    }
}

#[derive(Debug, Serialize)]
pub struct SprtReport {
    pub schema_version: u32,
    pub verdict: Verdict,
    /// Same meaning and default (`Valid`) as `Report::validity` - see
    /// `apply_failure_caps` in this module.
    pub validity: Validity,
    /// Same meaning as `Report::promotion`.
    pub promotion: Promotion,
    /// For `Wald`/`Trinomial`, this retains its historical full-input aggregate meaning; use
    /// `decision_llr` for the sequential verdict prefix. For `SprtVariant::Pentanomial`, this is
    /// the LLR *at the sequential stopping point* (see the
    /// module doc) - never the value a naive final-aggregate recompute over the whole input
    /// would give, which can differ whenever the true stopping point falls before the input's
    /// end. When `stopping_pair_count` is `None` (no stopping point was reached - either the
    /// input ran out first, or `min_paired_ids` was never satisfied), this is instead the raw
    /// running LLR over all available pairs, which was **never compared against the bounds** -
    /// it can sit past `lower_bound`/`upper_bound` while `verdict` is still `Inconclusive`.
    /// Callers must read `verdict`/`promotion`, never re-derive a decision from `llr` vs the
    /// bounds directly.
    pub llr: f64,
    /// LLR at the prefix that actually determined `verdict`. Equal to `llr` for Pentanomial;
    /// may differ for Wald/Trinomial because their legacy `llr` field intentionally remains the
    /// full-input aggregate for backward compatibility.
    pub decision_llr: f64,
    pub lower_bound: f64,
    pub upper_bound: f64,
    pub elo0: f64,
    pub elo1: f64,
    pub alpha: f64,
    pub beta: f64,
    pub candidate_wins: u64,
    pub baseline_wins: u64,
    pub draws: u64,
    /// Outcome counts in the prefix used by `decision_llr`. The unprefixed fields above retain
    /// their historical full-input meaning for Wald/Trinomial.
    pub decision_candidate_wins: u64,
    pub decision_baseline_wins: u64,
    pub decision_draws: u64,
    pub timeouts: u64,
    pub crashes: u64,
    pub invalid: u64,
    pub failure_breakdown: FailureBreakdown,
    pub reason: String,
    /// The full-input estimated draw-rate nuisance parameter, `Some` only for
    /// `SprtVariant::Trinomial`. Use `decision_drawelo` for the sequential verdict prefix.
    /// `None` for `Wald`/`Pentanomial`, neither of which models a draw rate.
    pub drawelo: Option<f64>,
    /// Trinomial nuisance estimate at the sequential stopping prefix. The unprefixed `drawelo`
    /// remains the full-input estimate for compatibility. `None` for Wald/Pentanomial.
    pub decision_drawelo: Option<f64>,
    /// Which variant produced this report - additive alongside the fields
    /// above (present for every variant, not just `Pentanomial`) so a
    /// consumer never has to infer it from `drawelo`'s presence.
    pub sprt_variant: &'static str,
    /// `Some` only for `SprtVariant::Pentanomial`: the 5-value breakdown the
    /// LLR was actually computed from (`candidate_wins`/`baseline_wins`/
    /// `draws` above are still populated too, netted from these same 5
    /// buckets, for compatibility with tooling that only understands the
    /// 3-outcome shape).
    pub pentanomial_counts: Option<PentanomialCounts>,
    /// `Some` only for `SprtVariant::Pentanomial`: total input records
    /// (before pairing) - always `2 * available_paired_count`, since an incomplete pair is
    /// rejected before a report is ever produced (see `PentanomialCollector::finish`). Reported
    /// anyway for a reader to self-check the pairing math without cross-referencing the input.
    pub raw_trial_count: Option<u64>,
    /// `Some` only for `SprtVariant::Pentanomial`: number of complete pairs actually analyzed,
    /// i.e. folded into `llr`/`pentanomial_counts` up to the sequential stopping point (see the
    /// module doc). May be less than `available_paired_count` when the walk stopped early -
    /// see `stopping_pair_count`/`ignored_pairs_after_stop`.
    pub paired_count: Option<u64>,
    /// `Some` only for `SprtVariant::Pentanomial`: total complete pairs present in the input,
    /// before any sequential truncation. Equal to `paired_count` unless the walk stopped early.
    pub available_paired_count: Option<u64>,
    /// `Some` only for `SprtVariant::Pentanomial`: the pair count at which the sequential walk
    /// actually stopped - a decisive verdict or a `--max-paired-ids` cutoff. `None` means the
    /// walk consumed every available pair without either (the ordinary "keep testing" state);
    /// in that case `paired_count == available_paired_count`.
    pub stopping_pair_count: Option<u64>,
    /// `Some` only for `SprtVariant::Pentanomial`: one of `"upper_bound_crossed"` /
    /// `"lower_bound_crossed"` / `"max_paired_ids_reached"` / `"insufficient_data"`.
    pub stopping_reason: Option<&'static str>,
    /// `Some` only for `SprtVariant::Pentanomial`: `available_paired_count - paired_count` -
    /// pairs present in the input but never folded into the analysis because they completed
    /// after the sequential stopping point. `Some(0)` (not `None`) when nothing was ignored, so
    /// a consumer can tell "no pairs ignored" from "not a pentanomial run" (the latter is
    /// `None`).
    pub ignored_pairs_after_stop: Option<u64>,
    /// Unit used by the generic sequential-replay counts: `"trial"` for unpaired Wald/Trinomial,
    /// `"pair"` for `--paired-by-id` Wald/Trinomial and for Pentanomial.
    pub analysis_unit: &'static str,
    /// Total reduced observations available in the input before sequential truncation.
    pub available_observation_count: u64,
    /// Reduced observations folded into `decision_llr` through the stopping prefix.
    pub analyzed_observation_count: u64,
    /// Observation count at a decisive boundary crossing; `None` when input ended inconclusively.
    pub stopping_observation_count: Option<u64>,
    /// `upper_bound_crossed`, `lower_bound_crossed`, `max_paired_ids_reached`, or
    /// `insufficient_data`, expressed generically for all variants.
    pub stopping_observation_reason: &'static str,
    /// Available observations not analyzed because they occur after the stopping prefix.
    pub ignored_observations_after_stop: u64,
    /// Raw input records that produced the available/analyzed observations. Under pairing, one
    /// observation usually represents two records; a tolerated lone/id-less trial represents one.
    pub available_record_count: u64,
    pub analyzed_record_count: u64,
    pub ignored_records_after_stop: u64,
    /// Echoes `--min-paired-ids`, `Some` only when it was passed.
    pub min_paired_ids: Option<u64>,
    /// Echoes `--max-paired-ids`, `Some` only when it was passed.
    pub max_paired_ids: Option<u64>,
    /// Echoes `--require-complete-pairs` - a report produced under strict pairing must be
    /// distinguishable from one produced without it, the same provenance concern
    /// `verify-run`'s manifest/record consistency checks exist for.
    pub require_complete_pairs: bool,
}

/// Strict pairing for `SprtVariant::Pentanomial`: unlike `OutcomeCollector` (which tolerates a
/// lone id as an ordinary unpaired sample), every id here must resolve to *exactly* 2 records.
/// A pentanomial pair's whole statistical value is the cancellation between two games sharing
/// an opening (see `stats::pentanomial_sprt`'s module doc) - a lone game has no partner to
/// cancel bias against, so treating it as a substitute single-game observation (the way
/// `OutcomeCollector` does for other variants) would silently mix bias-cancelled and
/// bias-uncancelled observations into the same LLR sum. Rejecting it outright is the
/// conservative choice: an ambiguous pairing structure should fail loudly, not get judged
/// anyway (per this project's "false pass is worse than inconclusive" bias).
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
) -> PentanomialWalk {
    let available_paired_count = completed_in_order.len() as u64;
    let effective_min = min_paired_ids.unwrap_or(0);
    let mut buckets = [0u64; 5];

    for (index, &(_, bucket)) in completed_in_order.iter().enumerate() {
        buckets[bucket] += 1;
        let paired_count = (index + 1) as u64;
        let llr = pentanomial_sprt::pentanomial_llr(config.elo0, config.elo1, &buckets);

        if paired_count >= effective_min {
            if llr >= bounds.upper {
                return PentanomialWalk {
                    buckets,
                    llr,
                    available_paired_count,
                    analyzed_paired_count: paired_count,
                    stopping_pair_count: Some(paired_count),
                    stopping_reason: "upper_bound_crossed",
                };
            }
            if llr <= bounds.lower {
                return PentanomialWalk {
                    buckets,
                    llr,
                    available_paired_count,
                    analyzed_paired_count: paired_count,
                    stopping_pair_count: Some(paired_count),
                    stopping_reason: "lower_bound_crossed",
                };
            }
        }
        if let Some(max) = max_paired_ids
            && paired_count >= max
        {
            return PentanomialWalk {
                buckets,
                llr,
                available_paired_count,
                analyzed_paired_count: paired_count,
                stopping_pair_count: Some(paired_count),
                stopping_reason: "max_paired_ids_reached",
            };
        }
    }

    let llr = pentanomial_sprt::pentanomial_llr(config.elo0, config.elo1, &buckets);
    PentanomialWalk {
        buckets,
        llr,
        available_paired_count,
        analyzed_paired_count: available_paired_count,
        stopping_pair_count: None,
        stopping_reason: "insufficient_data",
    }
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
#[derive(Clone, Copy)]
struct OrderedOutcome {
    completion_line: usize,
    outcome: Outcome,
    raw_record_count: u64,
}

/// Order-preserving counterpart to `metrics::OutcomeCollector`, used only by SPRT because a
/// sequential test must retain its schedule. This deliberately keeps the existing pairing
/// contract: id-less records are single observations, a lone id is tolerated unless
/// `require_complete_pairs` is set, and an id occurring more than twice is invalid.
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

fn outcome_llr(
    variant: SprtVariant,
    config: &SprtConfig,
    candidate_wins: u64,
    baseline_wins: u64,
    draws: u64,
) -> (f64, Option<f64>) {
    match variant {
        SprtVariant::Wald => {
            let p0 = math::score_from_elo(config.elo0);
            let p1 = math::score_from_elo(config.elo1);
            (
                candidate_wins as f64 * math::llr_delta(true, p0, p1)
                    + baseline_wins as f64 * math::llr_delta(false, p0, p1),
                None,
            )
        }
        SprtVariant::Trinomial => {
            if candidate_wins + baseline_wins + draws == 0 {
                (0.0, Some(0.0))
            } else {
                let (llr, drawelo) = trinomial_sprt::llr(
                    config.elo0,
                    config.elo1,
                    candidate_wins,
                    draws,
                    baseline_wins,
                );
                (llr, Some(drawelo))
            }
        }
        SprtVariant::Pentanomial => unreachable!("pentanomial has its own sequential walk"),
    }
}

fn evaluate_outcomes_sequential(
    observations: &[OrderedOutcome],
    config: &SprtConfig,
    variant: SprtVariant,
    bounds: &math::SprtBounds,
) -> OutcomeWalk {
    let available_observation_count = observations.len() as u64;
    let available_record_count = observations
        .iter()
        .map(|observation| observation.raw_record_count)
        .sum();
    let (mut candidate_wins, mut baseline_wins, mut draws) = (0, 0, 0);
    let mut analyzed_record_count = 0;

    for (index, observation) in observations.iter().enumerate() {
        tally_outcome(
            observation.outcome,
            &mut candidate_wins,
            &mut baseline_wins,
            &mut draws,
        );
        analyzed_record_count += observation.raw_record_count;
        let analyzed_observation_count = (index + 1) as u64;
        let (llr, drawelo) = outcome_llr(variant, config, candidate_wins, baseline_wins, draws);
        let stopping_reason = if llr >= bounds.upper {
            Some("upper_bound_crossed")
        } else if llr <= bounds.lower {
            Some("lower_bound_crossed")
        } else {
            None
        };
        if let Some(stopping_reason) = stopping_reason {
            return OutcomeWalk {
                llr,
                drawelo,
                candidate_wins,
                baseline_wins,
                draws,
                available_observation_count,
                analyzed_observation_count,
                stopping_observation_count: Some(analyzed_observation_count),
                stopping_reason,
                available_record_count,
                analyzed_record_count,
            };
        }
    }

    let (llr, drawelo) = outcome_llr(variant, config, candidate_wins, baseline_wins, draws);
    OutcomeWalk {
        llr,
        drawelo,
        candidate_wins,
        baseline_wins,
        draws,
        available_observation_count,
        analyzed_observation_count: available_observation_count,
        stopping_observation_count: None,
        stopping_reason: "insufficient_data",
        available_record_count,
        analyzed_record_count: available_record_count,
    }
}

/// `paired_by_id`: see `metrics::compute` - two records sharing an `id` are
/// combined into one net observation (by total points across the pair)
/// instead of two independent trials. `records` is a streaming iterator, but sequential replay
/// retains the reduced observation schedule until the input has been validated. This makes memory
/// O(n); an incremental accumulator can restore O(1) for unpaired Wald in a future change without
/// changing the report contract.
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
    let mut records = records.into_iter().peekable();
    if records.peek().is_none() {
        return Err(VeridictError::EmptyInput);
    }
    if variant == SprtVariant::Pentanomial && !paired_by_id {
        return Err(VeridictError::InvalidThreshold(
            "--sprt-variant pentanomial requires --paired-by-id".to_string(),
        ));
    }
    if variant != SprtVariant::Pentanomial && (min_paired_ids.is_some() || max_paired_ids.is_some())
    {
        return Err(VeridictError::InvalidThreshold(
            "--min-paired-ids/--max-paired-ids require --sprt-variant pentanomial (the only \
             variant with a paired_count)"
                .to_string(),
        ));
    }
    if let Some(min) = min_paired_ids
        && min < 1
    {
        return Err(VeridictError::InvalidThreshold(
            "--min-paired-ids must be >= 1".to_string(),
        ));
    }
    if let (Some(min), Some(max)) = (min_paired_ids, max_paired_ids)
        && min > max
    {
        return Err(VeridictError::InvalidThreshold(format!(
            "--min-paired-ids ({min}) must be <= --max-paired-ids ({max})"
        )));
    }

    let mut failures = FailureBreakdown::default();
    // Both collectors are always constructed, but only the one matching `variant` is fed and
    // finished below. Keeping this branch at ingestion time also ensures every input record is
    // still parsed and its status tallied even when the statistical walk later stops early.
    let mut collector = SequentialOutcomeCollector::new(paired_by_id, require_complete_pairs);
    let mut pentanomial_collector = PentanomialCollector::new();

    for item in records {
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
            if variant == SprtVariant::Pentanomial {
                let id = record
                    .id
                    .as_deref()
                    .ok_or_else(|| VeridictError::SchemaMismatch {
                        line,
                        context: "pentanomial",
                        detail: "record has no id; --sprt-variant pentanomial requires every \
                             record to carry one"
                            .to_string(),
                    })?;
                pentanomial_collector.record(line, id, outcome);
            } else {
                collector.record(line, record.id.as_deref(), outcome);
            }
        }

        if !used {
            return Err(VeridictError::SchemaMismatch {
                line,
                context: "sprt",
                detail: "record has no result and no status fields".to_string(),
            });
        }
    }

    let timeouts = failures.baseline.timeout + failures.candidate.timeout;
    let crashes = failures.baseline.crash + failures.candidate.crash;
    let invalid = failures.baseline.invalid + failures.candidate.invalid;

    let bounds = math::bounds(config.alpha, config.beta);
    let analysis = match variant {
        SprtVariant::Wald | SprtVariant::Trinomial => {
            let observations = collector.finish()?;
            let (mut candidate_wins, mut baseline_wins, mut draws) = (0, 0, 0);
            for observation in &observations {
                tally_outcome(
                    observation.outcome,
                    &mut candidate_wins,
                    &mut baseline_wins,
                    &mut draws,
                );
            }
            let (llr, drawelo) = outcome_llr(variant, config, candidate_wins, baseline_wins, draws);
            let walk = evaluate_outcomes_sequential(&observations, config, variant, &bounds);
            RunAnalysis {
                llr,
                drawelo,
                candidate_wins,
                baseline_wins,
                draws,
                decision_llr: walk.llr,
                decision_drawelo: walk.drawelo,
                decision_candidate_wins: walk.candidate_wins,
                decision_baseline_wins: walk.baseline_wins,
                decision_draws: walk.draws,
                analysis_unit: if paired_by_id { "pair" } else { "trial" },
                available_observation_count: walk.available_observation_count,
                analyzed_observation_count: walk.analyzed_observation_count,
                stopping_observation_count: walk.stopping_observation_count,
                stopping_observation_reason: walk.stopping_reason,
                available_record_count: walk.available_record_count,
                analyzed_record_count: walk.analyzed_record_count,
                pentanomial: None,
            }
        }
        SprtVariant::Pentanomial => {
            let (completed_in_order, raw_trial_count) = pentanomial_collector.finish()?;
            let walk = evaluate_pentanomial_sequential(
                &completed_in_order,
                config,
                &bounds,
                min_paired_ids,
                max_paired_ids,
            );
            let llr = walk.llr;
            let (baseline_wins, candidate_wins, draws) = net_pentanomial_buckets(&walk.buckets);
            RunAnalysis {
                llr,
                drawelo: None,
                candidate_wins,
                baseline_wins,
                draws,
                decision_llr: llr,
                decision_drawelo: None,
                decision_candidate_wins: candidate_wins,
                decision_baseline_wins: baseline_wins,
                decision_draws: draws,
                analysis_unit: "pair",
                available_observation_count: walk.available_paired_count,
                analyzed_observation_count: walk.analyzed_paired_count,
                stopping_observation_count: walk.stopping_pair_count,
                stopping_observation_reason: walk.stopping_reason,
                available_record_count: raw_trial_count,
                analyzed_record_count: walk.analyzed_paired_count * 2,
                pentanomial: Some((walk, raw_trial_count)),
            }
        }
    };

    let unit = if variant == SprtVariant::Trinomial {
        "belo"
    } else {
        "elo"
    };
    let (verdict, mut reason) = match analysis.stopping_observation_reason {
        "upper_bound_crossed" => (
            Verdict::Pass,
            format!(
                "LLR {:.3} reached the upper bound {:.3}: reject H0 ({unit} <= {:+.1}), accept H1 ({unit} >= {:+.1})",
                analysis.decision_llr, bounds.upper, config.elo0, config.elo1
            ),
        ),
        "lower_bound_crossed" => (
            Verdict::Fail,
            format!(
                "LLR {:.3} reached the lower bound {:.3}: reject H1 ({unit} >= {:+.1}), accept H0 ({unit} <= {:+.1})",
                analysis.decision_llr, bounds.lower, config.elo1, config.elo0
            ),
        ),
        "max_paired_ids_reached" | "insufficient_data" => (
            Verdict::Inconclusive,
            format!(
                "LLR {:.3} is within ({:.3}, {:.3}): keep testing",
                analysis.decision_llr, bounds.lower, bounds.upper
            ),
        ),
        _ => unreachable!("sequential walk returned an unknown stopping reason"),
    };

    let (
        pentanomial_counts,
        raw_trial_count,
        paired_count,
        available_paired_count,
        stopping_pair_count,
        stopping_reason,
        ignored_pairs_after_stop,
    ) = match analysis.pentanomial {
        Some((walk, raw_trial_count)) => {
            if walk.stopping_reason == "insufficient_data"
                && min_paired_ids.is_some_and(|min| walk.analyzed_paired_count < min)
            {
                reason = format!(
                    "Only {} of minimum {} paired ids completed; strength not evaluated yet.",
                    walk.analyzed_paired_count,
                    min_paired_ids.unwrap()
                );
            } else if walk.stopping_reason == "max_paired_ids_reached" {
                reason = format!(
                    "{reason} (reached --max-paired-ids {} without crossing a boundary)",
                    max_paired_ids.unwrap()
                );
            }
            (
                Some(PentanomialCounts::from_buckets(walk.buckets)),
                Some(raw_trial_count),
                Some(walk.analyzed_paired_count),
                Some(walk.available_paired_count),
                walk.stopping_pair_count,
                Some(walk.stopping_reason),
                Some(walk.available_paired_count - walk.analyzed_paired_count),
            )
        }
        None => (None, None, None, None, None, None, None),
    };

    Ok(SprtReport {
        schema_version: crate::report::REPORT_SCHEMA_VERSION,
        verdict,
        validity: Validity::Valid,
        promotion: Promotion::decide(Validity::Valid, verdict),
        llr: analysis.llr,
        decision_llr: analysis.decision_llr,
        lower_bound: bounds.lower,
        upper_bound: bounds.upper,
        elo0: config.elo0,
        elo1: config.elo1,
        alpha: config.alpha,
        beta: config.beta,
        candidate_wins: analysis.candidate_wins,
        baseline_wins: analysis.baseline_wins,
        draws: analysis.draws,
        decision_candidate_wins: analysis.decision_candidate_wins,
        decision_baseline_wins: analysis.decision_baseline_wins,
        decision_draws: analysis.decision_draws,
        timeouts,
        crashes,
        invalid,
        failure_breakdown: failures,
        drawelo: analysis.drawelo,
        decision_drawelo: analysis.decision_drawelo,
        sprt_variant: variant.label(),
        pentanomial_counts,
        raw_trial_count,
        paired_count,
        available_paired_count,
        stopping_pair_count,
        stopping_reason,
        ignored_pairs_after_stop,
        analysis_unit: analysis.analysis_unit,
        available_observation_count: analysis.available_observation_count,
        analyzed_observation_count: analysis.analyzed_observation_count,
        stopping_observation_count: analysis.stopping_observation_count,
        stopping_observation_reason: analysis.stopping_observation_reason,
        ignored_observations_after_stop: analysis.available_observation_count
            - analysis.analyzed_observation_count,
        available_record_count: analysis.available_record_count,
        analyzed_record_count: analysis.analyzed_record_count,
        ignored_records_after_stop: analysis.available_record_count
            - analysis.analyzed_record_count,
        min_paired_ids,
        max_paired_ids,
        require_complete_pairs,
        reason,
    })
}

impl SprtReport {
    pub fn to_json_pretty(&self) -> String {
        serde_json::to_string_pretty(self)
            .expect("SprtReport contains only finite fields and strings; serialization cannot fail")
    }

    pub fn to_markdown(&self) -> String {
        let b = &self.failure_breakdown.baseline;
        let c = &self.failure_breakdown.candidate;
        let unit = if self.sprt_variant == "trinomial" {
            "belo"
        } else {
            "elo"
        };
        format!(
            "# Veridict SPRT Report\n\n\
             Verdict: {verdict}\n\
             Validity: {validity}\n\
             Promotion: {promotion}\n\n\
             H0: {unit} <= {elo0:+.1} / H1: {unit} >= {elo1:+.1} (alpha={alpha}, beta={beta})\n\
             Decision LLR: {decision_llr:.4} (bounds: {lower:.4} to {upper:.4})\n\
             Full-input LLR: {llr:.4}\n\
             {drawelo_line}\
             {reason}\n\n\
             Schedule: {analyzed_observations} of {available_observations} {analysis_unit}(s) analyzed; stop={stopping_reason}; ignored={ignored_observations} {analysis_unit}(s) / {ignored_records} raw record(s)\n\
             Decision outcomes: candidate_wins={decision_candidate_wins}, baseline_wins={decision_baseline_wins}, draws={decision_draws}\n\
             Full-input outcomes: candidate_wins={candidate_wins}, baseline_wins={baseline_wins}, draws={draws}\n\
             {pentanomial_line}\n\
             Status counts:\n\
             - timeout: {timeouts} (baseline={b_timeout}, candidate={c_timeout})\n\
             - crash: {crashes} (baseline={b_crash}, candidate={c_crash})\n\
             - invalid: {invalid} (baseline={b_invalid}, candidate={c_invalid})\n",
            verdict = serde_str(&self.verdict),
            validity = serde_str(&self.validity),
            promotion = serde_str(&self.promotion),
            elo0 = self.elo0,
            elo1 = self.elo1,
            alpha = self.alpha,
            beta = self.beta,
            decision_llr = self.decision_llr,
            llr = self.llr,
            lower = self.lower_bound,
            upper = self.upper_bound,
            drawelo_line = match (self.decision_drawelo, self.drawelo) {
                (Some(decision), Some(full)) => format!(
                    "Decision-prefix drawelo: {decision:+.1}\nFull-input drawelo: {full:+.1}\n"
                ),
                _ => String::new(),
            },
            reason = self.reason,
            analyzed_observations = self.analyzed_observation_count,
            available_observations = self.available_observation_count,
            analysis_unit = self.analysis_unit,
            stopping_reason = self.stopping_observation_reason,
            ignored_observations = self.ignored_observations_after_stop,
            ignored_records = self.ignored_records_after_stop,
            decision_candidate_wins = self.decision_candidate_wins,
            decision_baseline_wins = self.decision_baseline_wins,
            decision_draws = self.decision_draws,
            candidate_wins = self.candidate_wins,
            baseline_wins = self.baseline_wins,
            draws = self.draws,
            pentanomial_line = match &self.pentanomial_counts {
                Some(p) => {
                    let ignored = self.ignored_pairs_after_stop.unwrap_or(0);
                    let stopped_early =
                        format!(" ({ignored} pair(s) after the stopping point ignored)");
                    format!(
                        "\nPentanomial pairs ({} analyzed of {} available, {} raw trials): \
                         0-0={} 0.5-0={} 1-1={} 1.5-0.5={} 2-0={}{}\n",
                        self.paired_count.unwrap_or(0),
                        self.available_paired_count.unwrap_or(0),
                        self.raw_trial_count.unwrap_or(0),
                        p.score_0_0,
                        p.score_0_5,
                        p.score_1_0,
                        p.score_1_5,
                        p.score_2_0,
                        if ignored > 0 {
                            stopped_early
                        } else {
                            String::new()
                        },
                    )
                }
                None => String::new(),
            },
            timeouts = self.timeouts,
            crashes = self.crashes,
            invalid = self.invalid,
            b_timeout = b.timeout,
            c_timeout = c.timeout,
            b_crash = b.crash,
            c_crash = c.crash,
            b_invalid = b.invalid,
            c_invalid = c.invalid,
        )
    }
}

/// `verdict::apply_failure_caps`, for `SprtReport` - a separate report type
/// (see this module's doc), so it gets its own copy of the same small gate
/// rather than a shared trait for two call sites.
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
mod tests {
    use super::*;

    fn ok_iter(
        records: &[(usize, Record)],
    ) -> impl Iterator<Item = Result<(usize, Record), VeridictError>> + '_ {
        records.iter().cloned().map(Ok)
    }

    fn rec(result: Option<&str>) -> Record {
        rec_with_id(None, result)
    }

    fn rec_with_id(id: Option<&str>, result: Option<&str>) -> Record {
        Record {
            id: id.map(str::to_string),
            baseline: None,
            candidate: None,
            result: result.map(str::to_string),
            baseline_status: None,
            candidate_status: None,
        }
    }

    #[test]
    fn clear_h1_stream_passes() {
        let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();
        let records: Vec<_> = (0..2000)
            .map(|i| (i + 1, rec(Some("candidate_win"))))
            .collect();
        let report = run(
            ok_iter(&records),
            &config,
            SprtVariant::Wald,
            false,
            FailurePolicy::ReportOnly,
            false,
            None,
            None,
        )
        .unwrap();
        assert_eq!(report.verdict, Verdict::Pass);
        assert!(report.llr >= report.upper_bound);
        assert_eq!(report.drawelo, None);
    }

    #[test]
    fn trinomial_clear_h1_stream_passes_and_reports_drawelo() {
        let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();
        let records: Vec<_> = (0..2000)
            .map(|i| (i + 1, rec(Some("candidate_win"))))
            .collect();
        let report = run(
            ok_iter(&records),
            &config,
            SprtVariant::Trinomial,
            false,
            FailurePolicy::ReportOnly,
            false,
            None,
            None,
        )
        .unwrap();
        assert_eq!(report.verdict, Verdict::Pass);
        assert!(report.llr >= report.upper_bound);
        assert!(report.drawelo.is_some());
    }

    #[test]
    fn trinomial_draw_heavy_stream_still_reaches_a_verdict() {
        // A draw-heavy but clearly candidate-favored stream - the scenario
        // this variant exists for. Not asserting a specific verdict (that
        // depends on the exact mix), just that it computes a finite report
        // rather than getting stuck on the draw-rate estimation.
        let mut records = Vec::new();
        for i in 0..300 {
            records.push((i * 2 + 1, rec(Some("draw"))));
            records.push((i * 2 + 2, rec(Some("candidate_win"))));
        }
        let config = SprtConfig::new(0.0, 30.0, 0.05, 0.05).unwrap();
        let report = run(
            ok_iter(&records),
            &config,
            SprtVariant::Trinomial,
            false,
            FailurePolicy::ReportOnly,
            false,
            None,
            None,
        )
        .unwrap();
        assert!(report.llr.is_finite());
        assert!(report.drawelo.unwrap().is_finite());
    }

    #[test]
    fn trinomial_zero_draws_matches_wald_verdict() {
        // Integration-level companion to stats::trinomial_sprt's exact
        // pure-math reduction test: with no draws in the actual record
        // stream, both variants should reach the same verdict end to end.
        let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();
        let records: Vec<_> = (0..1000)
            .map(|i| {
                (
                    i + 1,
                    rec(Some(if i % 7 == 0 {
                        "baseline_win"
                    } else {
                        "candidate_win"
                    })),
                )
            })
            .collect();
        let wald = run(
            ok_iter(&records),
            &config,
            SprtVariant::Wald,
            false,
            FailurePolicy::ReportOnly,
            false,
            None,
            None,
        )
        .unwrap();
        let trinomial = run(
            ok_iter(&records),
            &config,
            SprtVariant::Trinomial,
            false,
            FailurePolicy::ReportOnly,
            false,
            None,
            None,
        )
        .unwrap();
        assert_eq!(wald.verdict, trinomial.verdict);
        assert!((wald.llr - trinomial.llr).abs() < 1e-6);
    }

    #[test]
    fn clear_h0_stream_fails() {
        let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();
        let records: Vec<_> = (0..2000)
            .map(|i| (i + 1, rec(Some("baseline_win"))))
            .collect();
        let report = run(
            ok_iter(&records),
            &config,
            SprtVariant::Wald,
            false,
            FailurePolicy::ReportOnly,
            false,
            None,
            None,
        )
        .unwrap();
        assert_eq!(report.verdict, Verdict::Fail);
        assert!(report.llr <= report.lower_bound);
    }

    #[test]
    fn small_mixed_sample_stays_inconclusive() {
        let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();
        let records = vec![
            (1, rec(Some("candidate_win"))),
            (2, rec(Some("baseline_win"))),
        ];
        let report = run(
            ok_iter(&records),
            &config,
            SprtVariant::Wald,
            false,
            FailurePolicy::ReportOnly,
            false,
            None,
            None,
        )
        .unwrap();
        assert_eq!(report.verdict, Verdict::Inconclusive);
        assert_eq!(report.stopping_observation_count, None);
        assert_eq!(report.stopping_observation_reason, "insufficient_data");
        assert_eq!(report.available_observation_count, 2);
        assert_eq!(report.analyzed_observation_count, 2);
        assert_eq!(report.ignored_observations_after_stop, 0);
    }

    #[test]
    fn wald_and_trinomial_retain_the_first_crossing_even_if_the_full_input_drifts_back() {
        let config = SprtConfig::new(0.0, 100.0, 0.4, 0.4).unwrap();
        let records = vec![
            (1, rec(Some("candidate_win"))),
            (2, rec(Some("candidate_win"))),
            (3, rec(Some("baseline_win"))),
            (4, rec(Some("baseline_win"))),
        ];

        for variant in [SprtVariant::Wald, SprtVariant::Trinomial] {
            let report = run(
                ok_iter(&records),
                &config,
                variant,
                false,
                FailurePolicy::ReportOnly,
                false,
                None,
                None,
            )
            .unwrap();

            assert_eq!(report.verdict, Verdict::Pass);
            assert!(report.decision_llr >= report.upper_bound);
            assert!(report.llr > report.lower_bound && report.llr < report.upper_bound);
            assert_eq!(report.candidate_wins, 2);
            assert_eq!(report.baseline_wins, 2);
            assert_eq!(report.decision_candidate_wins, 2);
            assert_eq!(report.decision_baseline_wins, 0);
            assert_eq!(report.available_observation_count, 4);
            assert_eq!(report.analyzed_observation_count, 2);
            assert_eq!(report.stopping_observation_count, Some(2));
            assert_eq!(report.stopping_observation_reason, "upper_bound_crossed");
            assert_eq!(report.ignored_observations_after_stop, 2);
            assert_eq!(report.available_record_count, 4);
            assert_eq!(report.analyzed_record_count, 2);
            assert_eq!(report.ignored_records_after_stop, 2);
        }
    }

    #[test]
    fn wald_and_trinomial_treat_input_order_as_the_schedule() {
        let config = SprtConfig::new(0.0, 100.0, 0.4, 0.4).unwrap();
        let candidate_first = vec![
            (1, rec(Some("candidate_win"))),
            (2, rec(Some("candidate_win"))),
            (3, rec(Some("baseline_win"))),
            (4, rec(Some("baseline_win"))),
        ];
        let baseline_first = vec![
            (1, rec(Some("baseline_win"))),
            (2, rec(Some("baseline_win"))),
            (3, rec(Some("candidate_win"))),
            (4, rec(Some("candidate_win"))),
        ];

        for variant in [SprtVariant::Wald, SprtVariant::Trinomial] {
            let first = run(
                ok_iter(&candidate_first),
                &config,
                variant,
                false,
                FailurePolicy::ReportOnly,
                false,
                None,
                None,
            )
            .unwrap();
            let reversed = run(
                ok_iter(&baseline_first),
                &config,
                variant,
                false,
                FailurePolicy::ReportOnly,
                false,
                None,
                None,
            )
            .unwrap();

            assert_eq!(first.llr, reversed.llr);
            assert_eq!(first.verdict, Verdict::Pass);
            assert_eq!(reversed.verdict, Verdict::Fail);
            assert_eq!(first.stopping_observation_count, Some(2));
            assert_eq!(reversed.stopping_observation_count, Some(2));
        }
    }

    #[test]
    fn paired_wald_checks_boundaries_only_after_a_pair_completes() {
        let config = SprtConfig::new(0.0, 100.0, 0.4, 0.4).unwrap();
        let records = vec![
            (1, rec_with_id(Some("p1"), Some("candidate_win"))),
            (2, rec_with_id(Some("p1"), Some("candidate_win"))),
            (3, rec_with_id(Some("p2"), Some("candidate_win"))),
            (4, rec_with_id(Some("p2"), Some("candidate_win"))),
            (5, rec_with_id(Some("p3"), Some("baseline_win"))),
            (6, rec_with_id(Some("p3"), Some("baseline_win"))),
            (7, rec_with_id(Some("p4"), Some("baseline_win"))),
            (8, rec_with_id(Some("p4"), Some("baseline_win"))),
        ];
        let report = run(
            ok_iter(&records),
            &config,
            SprtVariant::Wald,
            true,
            FailurePolicy::ReportOnly,
            true,
            None,
            None,
        )
        .unwrap();

        assert_eq!(report.verdict, Verdict::Pass);
        assert_eq!(report.analysis_unit, "pair");
        assert_eq!(report.available_observation_count, 4);
        assert_eq!(report.analyzed_observation_count, 2);
        assert_eq!(report.available_record_count, 8);
        assert_eq!(report.analyzed_record_count, 4);
        assert_eq!(report.ignored_records_after_stop, 4);
    }

    #[test]
    fn draws_do_not_move_the_llr() {
        let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();
        let records = vec![(1, rec(Some("draw"))), (2, rec(Some("draw")))];
        let report = run(
            ok_iter(&records),
            &config,
            SprtVariant::Wald,
            false,
            FailurePolicy::ReportOnly,
            false,
            None,
            None,
        )
        .unwrap();
        assert_eq!(report.llr, 0.0);
        assert_eq!(report.draws, 2);
    }

    #[test]
    fn rejects_elo0_not_less_than_elo1() {
        assert!(matches!(
            SprtConfig::new(10.0, 10.0, 0.05, 0.05),
            Err(VeridictError::InvalidThreshold(_))
        ));
        assert!(matches!(
            SprtConfig::new(10.0, 0.0, 0.05, 0.05),
            Err(VeridictError::InvalidThreshold(_))
        ));
    }

    #[test]
    fn rejects_alpha_beta_out_of_range() {
        assert!(matches!(
            SprtConfig::new(0.0, 10.0, 0.0, 0.05),
            Err(VeridictError::InvalidThreshold(_))
        ));
        assert!(matches!(
            SprtConfig::new(0.0, 10.0, 0.05, 1.0),
            Err(VeridictError::InvalidThreshold(_))
        ));
    }

    #[test]
    fn empty_input_is_an_error() {
        let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();
        assert!(matches!(
            run(
                ok_iter(&[]),
                &config,
                SprtVariant::Wald,
                false,
                FailurePolicy::ReportOnly,
                false,
                None,
                None,
            ),
            Err(VeridictError::EmptyInput)
        ));
    }

    #[test]
    fn unusable_record_is_schema_mismatch() {
        let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();
        let records = vec![(1, rec(None))];
        assert!(matches!(
            run(
                ok_iter(&records),
                &config,
                SprtVariant::Wald,
                false,
                FailurePolicy::ReportOnly,
                false,
                None,
                None,
            ),
            Err(VeridictError::SchemaMismatch { .. })
        ));
    }

    #[test]
    fn paired_by_id_nets_split_pairs_to_a_draw() {
        let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();
        // Every testcase splits 1-1 (net draw) when paired: LLR stays at 0,
        // even though un-paired this would be 1000 wins + 1000 losses too -
        // same total either way here, so assert the more telling case below.
        let records: Vec<_> = (0..1000)
            .flat_map(|i| {
                [
                    (
                        i * 2 + 1,
                        rec_with_id(Some(&format!("op{i}")), Some("candidate_win")),
                    ),
                    (
                        i * 2 + 2,
                        rec_with_id(Some(&format!("op{i}")), Some("baseline_win")),
                    ),
                ]
            })
            .collect();
        let report = run(
            ok_iter(&records),
            &config,
            SprtVariant::Wald,
            true,
            FailurePolicy::ReportOnly,
            false,
            None,
            None,
        )
        .unwrap();
        assert_eq!(report.llr, 0.0);
        assert_eq!(report.draws, 1000);
    }

    /// `n` pairs (`2n` records), each pair scoring `outcomes` (candidate's two per-game
    /// outcomes for that pair, summed to one pentanomial bucket).
    fn pentanomial_records(n: usize, outcomes: (&str, &str)) -> Vec<(usize, Record)> {
        (0..n)
            .flat_map(|i| {
                let id = format!("op{i}");
                [
                    (i * 2 + 1, rec_with_id(Some(&id), Some(outcomes.0))),
                    (i * 2 + 2, rec_with_id(Some(&id), Some(outcomes.1))),
                ]
            })
            .collect()
    }

    #[test]
    fn pentanomial_paired_2_0_favors_candidate() {
        let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();
        let records = pentanomial_records(200, ("candidate_win", "candidate_win"));
        let report = run(
            ok_iter(&records),
            &config,
            SprtVariant::Pentanomial,
            true,
            FailurePolicy::ReportOnly,
            false,
            None,
            None,
        )
        .unwrap();
        assert_eq!(report.sprt_variant, "pentanomial");
        // Unanimous, extreme evidence like this crosses the upper bound well before all 200
        // input pairs complete - the walk stops there, and the remainder is never analyzed. See
        // the module doc: this is the fix, not a coincidence to work around.
        assert_eq!(report.available_paired_count, Some(200));
        assert_eq!(report.raw_trial_count, Some(400));
        let analyzed = report.paired_count.unwrap();
        assert!(analyzed < 200);
        assert_eq!(report.stopping_pair_count, Some(analyzed));
        assert_eq!(report.stopping_reason, Some("upper_bound_crossed"));
        assert_eq!(report.ignored_pairs_after_stop, Some(200 - analyzed));
        let counts = report.pentanomial_counts.as_ref().unwrap();
        assert_eq!(counts.score_2_0, analyzed);
        assert_eq!(
            [
                counts.score_0_0,
                counts.score_0_5,
                counts.score_1_0,
                counts.score_1_5
            ],
            [0, 0, 0, 0]
        );
        assert_eq!(report.candidate_wins, analyzed);
        assert!(report.llr > 0.0);
        assert_eq!(report.verdict, Verdict::Pass);
        assert_eq!(report.drawelo, None);
    }

    #[test]
    fn pentanomial_paired_1_5_0_5_favors_candidate() {
        let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();
        let records = pentanomial_records(200, ("candidate_win", "draw"));
        let report = run(
            ok_iter(&records),
            &config,
            SprtVariant::Pentanomial,
            true,
            FailurePolicy::ReportOnly,
            false,
            None,
            None,
        )
        .unwrap();
        let analyzed = report.paired_count.unwrap();
        assert!(analyzed < 200);
        assert_eq!(report.stopping_reason, Some("upper_bound_crossed"));
        let counts = report.pentanomial_counts.as_ref().unwrap();
        assert_eq!(counts.score_1_5, analyzed);
        assert_eq!(report.candidate_wins, analyzed);
        assert!(report.llr > 0.0);
    }

    #[test]
    fn pentanomial_paired_1_1_eventually_favors_baseline_despite_a_tiny_elo_gap() {
        // Every pair scores exactly 1-1 (candidate_win + baseline_win), pinning the empirical
        // mean at exactly 0.5 - H0's own target (elo0=0). Zero empirical variance around that
        // point is itself evidence *against* any alternative with a different target mean,
        // however small the gap (elo1=10 here) - so this is not a perpetually-neutral stream;
        // given enough pairs, it crosses the *lower* bound (rejecting H1) purely from having no
        // variance to explain away. This is the generalized LLR test working as intended (see
        // `stats::pentanomial_sprt`'s module doc), not a quirk of this fixture.
        let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();
        let records = pentanomial_records(200, ("candidate_win", "baseline_win"));
        let report = run(
            ok_iter(&records),
            &config,
            SprtVariant::Pentanomial,
            true,
            FailurePolicy::ReportOnly,
            false,
            None,
            None,
        )
        .unwrap();
        let analyzed = report.paired_count.unwrap();
        assert!(analyzed < 200);
        assert_eq!(report.verdict, Verdict::Fail);
        assert_eq!(report.stopping_reason, Some("lower_bound_crossed"));
        let counts = report.pentanomial_counts.as_ref().unwrap();
        assert_eq!(counts.score_1_0, analyzed);
        assert_eq!(report.draws, analyzed);
        assert_eq!(report.candidate_wins, 0);
        assert_eq!(report.baseline_wins, 0);
    }

    #[test]
    fn pentanomial_paired_0_5_1_5_favors_baseline() {
        let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();
        let records = pentanomial_records(200, ("baseline_win", "draw"));
        let report = run(
            ok_iter(&records),
            &config,
            SprtVariant::Pentanomial,
            true,
            FailurePolicy::ReportOnly,
            false,
            None,
            None,
        )
        .unwrap();
        let analyzed = report.paired_count.unwrap();
        assert!(analyzed < 200);
        assert_eq!(report.stopping_reason, Some("lower_bound_crossed"));
        let counts = report.pentanomial_counts.as_ref().unwrap();
        assert_eq!(counts.score_0_5, analyzed);
        assert_eq!(report.baseline_wins, analyzed);
        assert!(report.llr < 0.0);
    }

    #[test]
    fn pentanomial_paired_0_2_favors_baseline() {
        let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();
        let records = pentanomial_records(200, ("baseline_win", "baseline_win"));
        let report = run(
            ok_iter(&records),
            &config,
            SprtVariant::Pentanomial,
            true,
            FailurePolicy::ReportOnly,
            false,
            None,
            None,
        )
        .unwrap();
        let analyzed = report.paired_count.unwrap();
        assert!(analyzed < 200);
        assert_eq!(report.stopping_reason, Some("lower_bound_crossed"));
        let counts = report.pentanomial_counts.as_ref().unwrap();
        assert_eq!(counts.score_0_0, analyzed);
        assert_eq!(report.baseline_wins, analyzed);
        assert!(report.llr < 0.0);
        assert_eq!(report.verdict, Verdict::Fail);
    }

    #[test]
    fn pentanomial_incomplete_pair_is_an_error() {
        let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();
        let mut records = pentanomial_records(50, ("candidate_win", "baseline_win"));
        records.push((999, rec_with_id(Some("lonely"), Some("candidate_win"))));
        assert!(matches!(
            run(
                ok_iter(&records),
                &config,
                SprtVariant::Pentanomial,
                true,
                FailurePolicy::ReportOnly,
                false,
                None,
                None,
            ),
            Err(VeridictError::SchemaMismatch {
                context: "pentanomial",
                ..
            })
        ));
    }

    #[test]
    fn pentanomial_triple_id_is_an_error() {
        let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();
        let mut records = pentanomial_records(50, ("candidate_win", "baseline_win"));
        records.push((997, rec_with_id(Some("op0"), Some("candidate_win"))));
        assert!(matches!(
            run(
                ok_iter(&records),
                &config,
                SprtVariant::Pentanomial,
                true,
                FailurePolicy::ReportOnly,
                false,
                None,
                None,
            ),
            Err(VeridictError::SchemaMismatch {
                context: "pentanomial",
                ..
            })
        ));
    }

    #[test]
    fn pentanomial_record_without_id_is_an_error() {
        let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();
        let records = vec![(1, rec(Some("candidate_win")))];
        assert!(matches!(
            run(
                ok_iter(&records),
                &config,
                SprtVariant::Pentanomial,
                true,
                FailurePolicy::ReportOnly,
                false,
                None,
                None,
            ),
            Err(VeridictError::SchemaMismatch {
                context: "pentanomial",
                ..
            })
        ));
    }

    #[test]
    fn pentanomial_without_paired_by_id_is_rejected() {
        let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();
        let records = pentanomial_records(50, ("candidate_win", "baseline_win"));
        assert!(matches!(
            run(
                ok_iter(&records),
                &config,
                SprtVariant::Pentanomial,
                false,
                FailurePolicy::ReportOnly,
                false,
                None,
                None,
            ),
            Err(VeridictError::InvalidThreshold(_))
        ));
    }

    // --- apply_failure_caps ---

    #[test]
    fn sprt_report_defaults_to_valid_with_no_caps_applied() {
        let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();
        let records: Vec<_> = (0..40).map(|_| (1, rec(Some("candidate_win")))).collect();
        let report = run(
            ok_iter(&records),
            &config,
            SprtVariant::Wald,
            false,
            FailurePolicy::ReportOnly,
            false,
            None,
            None,
        )
        .unwrap();
        assert_eq!(report.validity, Validity::Valid);
        assert_eq!(
            report.promotion,
            Promotion::decide(Validity::Valid, report.verdict)
        );
    }

    #[test]
    fn sprt_report_invalidated_by_a_breached_crash_cap() {
        let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();
        let mut records: Vec<_> = (0..40).map(|_| (1, rec(Some("candidate_win")))).collect();
        for i in 0..3 {
            records.push((
                41 + i,
                Record {
                    id: None,
                    baseline: None,
                    candidate: None,
                    result: None,
                    baseline_status: None,
                    candidate_status: Some("crash".to_string()),
                },
            ));
        }
        let mut report = run(
            ok_iter(&records),
            &config,
            SprtVariant::Wald,
            false,
            FailurePolicy::ReportOnly,
            false,
            None,
            None,
        )
        .unwrap();
        assert_eq!(report.crashes, 3);
        let caps = FailureCaps {
            max_crashes: Some(0),
            ..Default::default()
        };
        apply_failure_caps(&mut report, &caps);
        assert_eq!(report.validity, Validity::Invalid);
        assert_eq!(report.verdict, Verdict::Inconclusive);
        assert_eq!(report.promotion, Promotion::NotPromoted);
        assert!(report.reason.contains("INVALID"));
    }

    // --- min_paired_ids/max_paired_ids validation ---

    #[test]
    fn min_paired_ids_zero_is_rejected() {
        let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();
        let records = pentanomial_records(5, ("candidate_win", "candidate_win"));
        assert!(matches!(
            run(
                ok_iter(&records),
                &config,
                SprtVariant::Pentanomial,
                true,
                FailurePolicy::ReportOnly,
                false,
                Some(0),
                None,
            ),
            Err(VeridictError::InvalidThreshold(_))
        ));
    }

    #[test]
    fn max_paired_ids_below_min_paired_ids_is_rejected() {
        let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();
        let records = pentanomial_records(5, ("candidate_win", "candidate_win"));
        assert!(matches!(
            run(
                ok_iter(&records),
                &config,
                SprtVariant::Pentanomial,
                true,
                FailurePolicy::ReportOnly,
                false,
                Some(10),
                Some(5),
            ),
            Err(VeridictError::InvalidThreshold(_))
        ));
    }

    #[test]
    fn min_paired_ids_may_equal_max_paired_ids() {
        let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();
        let records = pentanomial_records(50, ("candidate_win", "baseline_win")); // neutral
        let report = run(
            ok_iter(&records),
            &config,
            SprtVariant::Pentanomial,
            true,
            FailurePolicy::ReportOnly,
            false,
            Some(50),
            Some(50),
        )
        .unwrap();
        assert_eq!(report.paired_count, Some(50));
        assert_eq!(report.stopping_reason, Some("max_paired_ids_reached"));
    }

    #[test]
    fn min_max_paired_ids_require_pentanomial_variant() {
        let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();
        let records: Vec<_> = (0..10)
            .map(|i| (i + 1, rec(Some("candidate_win"))))
            .collect();
        assert!(matches!(
            run(
                ok_iter(&records),
                &config,
                SprtVariant::Wald,
                false,
                FailurePolicy::ReportOnly,
                false,
                Some(5),
                None,
            ),
            Err(VeridictError::InvalidThreshold(_))
        ));
    }

    // --- sequential min/max-paired-ids walk ---

    #[test]
    fn below_minimum_stays_inconclusive_even_though_the_llr_would_already_decide() {
        let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();
        let records = pentanomial_records(200, ("candidate_win", "candidate_win"));
        let unconstrained = run(
            ok_iter(&records),
            &config,
            SprtVariant::Pentanomial,
            true,
            FailurePolicy::ReportOnly,
            false,
            None,
            None,
        )
        .unwrap();
        // Sanity: this unanimous stream decides well before all 200 pairs complete - otherwise
        // the gated assertion below would hold vacuously (nothing to withhold).
        assert_eq!(unconstrained.verdict, Verdict::Pass);
        assert!(unconstrained.paired_count.unwrap() < 200);

        let gated = run(
            ok_iter(&records),
            &config,
            SprtVariant::Pentanomial,
            true,
            FailurePolicy::ReportOnly,
            false,
            Some(250), // above available_paired_count (200): the minimum is never reached
            None,
        )
        .unwrap();
        // The crossing happens well before the minimum, but the walk never even evaluates a
        // boundary check until the minimum is reached - and here it never is, so this is exactly
        // as inconclusive as a stream that never crossed anything, not "decided, but overridden."
        assert_eq!(gated.verdict, Verdict::Inconclusive);
        assert_eq!(gated.validity, Validity::Valid); // insufficient pairs, not a corrupt run
        assert_eq!(gated.paired_count, Some(200));
        assert_eq!(gated.stopping_pair_count, None);
        assert_eq!(gated.stopping_reason, Some("insufficient_data"));
        assert!(gated.reason.contains("200"));
        assert!(gated.reason.contains("250"));
        // Pin the surprising-looking field state deliberately: llr sits past upper_bound even
        // though verdict is Inconclusive, because stopping_pair_count is None (never evaluated
        // against the bounds). A caller that re-derives a decision from llr vs the bounds instead
        // of reading verdict would get this backwards - see the doc comment on SprtReport::llr.
        assert!(gated.llr > gated.upper_bound);
    }

    #[test]
    fn reaching_minimum_lets_a_real_decision_through() {
        let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();
        let records = pentanomial_records(200, ("candidate_win", "candidate_win"));
        let report = run(
            ok_iter(&records),
            &config,
            SprtVariant::Pentanomial,
            true,
            FailurePolicy::ReportOnly,
            false,
            Some(1),
            None,
        )
        .unwrap();
        assert_eq!(report.verdict, Verdict::Pass);
        assert_eq!(report.stopping_reason, Some("upper_bound_crossed"));
        assert_eq!(report.available_paired_count, Some(200));
        // The walk stopped well before the input's end - the remainder was never analyzed.
        assert!(report.paired_count.unwrap() < 200);
        assert_eq!(report.stopping_pair_count, report.paired_count);
        assert_eq!(
            report.ignored_pairs_after_stop,
            Some(200 - report.paired_count.unwrap())
        );
    }

    #[test]
    fn max_paired_ids_reached_without_crossing_stays_inconclusive() {
        let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();
        let records = pentanomial_records(50, ("candidate_win", "baseline_win")); // neutral
        let unconstrained = run(
            ok_iter(&records),
            &config,
            SprtVariant::Pentanomial,
            true,
            FailurePolicy::ReportOnly,
            false,
            None,
            None,
        )
        .unwrap();
        assert_eq!(unconstrained.verdict, Verdict::Inconclusive);

        let report = run(
            ok_iter(&records),
            &config,
            SprtVariant::Pentanomial,
            true,
            FailurePolicy::ReportOnly,
            false,
            None,
            Some(50),
        )
        .unwrap();
        assert_eq!(report.verdict, Verdict::Inconclusive); // no truncated-SPRT decision rule
        assert_eq!(report.stopping_reason, Some("max_paired_ids_reached"));
        assert_eq!(report.stopping_pair_count, Some(50));
        assert_eq!(report.ignored_pairs_after_stop, Some(0));
        assert!(report.reason.contains("--max-paired-ids"));
    }

    // --- require_complete_pairs ---

    #[test]
    fn require_complete_pairs_turns_a_lone_wald_id_into_a_hard_error() {
        let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();
        let mut records: Vec<_> = (0..40)
            .map(|i| {
                (
                    i + 1,
                    rec_with_id(Some(&format!("op{i}")), Some("candidate_win")),
                )
            })
            .collect();
        records.push((999, rec_with_id(Some("lonely"), Some("candidate_win"))));
        assert!(matches!(
            run(
                ok_iter(&records),
                &config,
                SprtVariant::Wald,
                true,
                FailurePolicy::ReportOnly,
                true,
                None,
                None,
            ),
            Err(VeridictError::SchemaMismatch {
                context: "paired-by-id",
                ..
            })
        ));
    }

    #[test]
    fn without_require_complete_pairs_a_lone_wald_id_is_tolerated() {
        let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();
        let mut records: Vec<_> = (0..40)
            .map(|i| {
                (
                    i + 1,
                    rec_with_id(Some(&format!("op{i}")), Some("candidate_win")),
                )
            })
            .collect();
        records.push((999, rec_with_id(Some("lonely"), Some("candidate_win"))));
        let report = run(
            ok_iter(&records),
            &config,
            SprtVariant::Wald,
            true,
            FailurePolicy::ReportOnly,
            false,
            None,
            None,
        )
        .unwrap();
        assert!(!report.require_complete_pairs);
    }

    #[test]
    fn require_complete_pairs_is_a_no_op_for_pentanomial() {
        // Pentanomial already unconditionally errors on a lone id via its own
        // `PentanomialCollector`, regardless of this flag - same error either way.
        let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();
        let mut records = pentanomial_records(50, ("candidate_win", "baseline_win"));
        records.push((999, rec_with_id(Some("lonely"), Some("candidate_win"))));
        for require_complete_pairs in [false, true] {
            assert!(matches!(
                run(
                    ok_iter(&records),
                    &config,
                    SprtVariant::Pentanomial,
                    true,
                    FailurePolicy::ReportOnly,
                    require_complete_pairs,
                    None,
                    None,
                ),
                Err(VeridictError::SchemaMismatch {
                    context: "pentanomial",
                    ..
                })
            ));
        }
    }

    // --- the discriminating test: no look-ahead on a fully-formed input file ---

    /// One pair per bucket index (0..=4), giving full control over the sequential trajectory a
    /// test wants to construct - unlike `pentanomial_records`, which only ever repeats one
    /// bucket. Config here is always `SprtConfig::new(0.0, 10.0, 0.05, 0.05)` - the same one
    /// every other test in this module uses - so the bucket-to-pair mapping below only has to
    /// be verified once (see the probing that produced these exact pair counts).
    fn pentanomial_records_from_buckets(buckets: &[usize]) -> Vec<(usize, Record)> {
        let outcomes = |b: usize| -> (&'static str, &'static str) {
            match b {
                0 => ("baseline_win", "baseline_win"),
                1 => ("baseline_win", "draw"),
                2 => ("candidate_win", "baseline_win"),
                3 => ("candidate_win", "draw"),
                4 => ("candidate_win", "candidate_win"),
                _ => panic!("bucket index must be 0..=4, got {b}"),
            }
        };
        buckets
            .iter()
            .enumerate()
            .flat_map(|(i, &b)| {
                let id = format!("op{i}");
                let (a, c) = outcomes(b);
                [
                    (i * 2 + 1, rec_with_id(Some(&id), Some(a))),
                    (i * 2 + 2, rec_with_id(Some(&id), Some(c))),
                ]
            })
            .collect()
    }

    /// 150 pairs of bucket 4 (2-0, unanimous candidate), then 150 of bucket 0 (0-2, unanimous
    /// baseline), then 300 neutral (bucket 2) - elo0=0/elo1=10/alpha=beta=0.05. Verified (by
    /// direct trajectory inspection while writing this test) to cross the upper bound at pair
    /// 104, fall back within bounds by pair 195, and settle at a final (all-600-pairs) LLR of
    /// about -0.50 - comfortably *inside* the bounds. A reader (or an implementation) that only
    /// looks at the finished file's aggregate would call this run inconclusive; the formal
    /// sequential answer is PASS, decided at pair 104.
    fn crosses_upper_then_settles_inconclusive() -> Vec<usize> {
        let mut buckets = vec![4; 150];
        buckets.extend(std::iter::repeat_n(0, 150));
        buckets.extend(std::iter::repeat_n(2, 300));
        buckets
    }

    /// Mirror of the above: bucket 0 first (crosses lower around pair 101), then bucket 4
    /// (back within bounds by pair 201), then neutral padding to 600.
    fn crosses_lower_then_settles_inconclusive() -> Vec<usize> {
        let mut buckets = vec![0; 150];
        buckets.extend(std::iter::repeat_n(4, 150));
        buckets.extend(std::iter::repeat_n(2, 300));
        buckets
    }

    #[test]
    fn sequential_walk_decides_at_the_true_crossing_pair_not_the_files_final_state() {
        let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();
        let records = pentanomial_records_from_buckets(&crosses_upper_then_settles_inconclusive());
        let report = run(
            ok_iter(&records),
            &config,
            SprtVariant::Pentanomial,
            true,
            FailurePolicy::ReportOnly,
            false,
            None,
            None,
        )
        .unwrap();
        // The whole point: a naive final-aggregate recompute over all 600 pairs would land
        // within bounds (verified while constructing this fixture) and call this Inconclusive.
        // The correct sequential answer is PASS, decided the moment the walk first crossed.
        assert_eq!(report.verdict, Verdict::Pass);
        assert_eq!(report.stopping_pair_count, Some(104));
        assert_eq!(report.paired_count, Some(104));
        assert_eq!(report.available_paired_count, Some(600));
        assert_eq!(report.ignored_pairs_after_stop, Some(600 - 104));
        assert_eq!(report.stopping_reason, Some("upper_bound_crossed"));
    }

    #[test]
    fn appending_pairs_after_the_stopping_point_changes_nothing() {
        let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();
        let full_buckets = crosses_upper_then_settles_inconclusive();
        let truncated_records = pentanomial_records_from_buckets(&full_buckets[..300]); // phase 1 + phase 2 only
        let full_records = pentanomial_records_from_buckets(&full_buckets); // + 300 more pairs

        let run_on = |records: &[(usize, Record)]| {
            run(
                ok_iter(records),
                &config,
                SprtVariant::Pentanomial,
                true,
                FailurePolicy::ReportOnly,
                false,
                None,
                None,
            )
            .unwrap()
        };
        let truncated = run_on(&truncated_records);
        let full = run_on(&full_records);
        assert_eq!(truncated.verdict, full.verdict);
        assert_eq!(truncated.stopping_pair_count, full.stopping_pair_count);
        assert_eq!(truncated.llr, full.llr);
        assert_eq!(truncated.paired_count, full.paired_count);
    }

    #[test]
    fn sequential_walk_decides_fail_at_the_true_lower_crossing_pair_not_the_files_final_state() {
        let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();
        let records = pentanomial_records_from_buckets(&crosses_lower_then_settles_inconclusive());
        let report = run(
            ok_iter(&records),
            &config,
            SprtVariant::Pentanomial,
            true,
            FailurePolicy::ReportOnly,
            false,
            None,
            None,
        )
        .unwrap();
        // Mirror of the upper-crossing test: a naive final-aggregate recompute over all 600
        // pairs lands within bounds and would call this Inconclusive. The correct sequential
        // answer is FAIL, decided the moment the walk first crossed the lower bound.
        assert_eq!(report.verdict, Verdict::Fail);
        assert_eq!(report.stopping_pair_count, Some(101));
        assert_eq!(report.paired_count, Some(101));
        assert_eq!(report.available_paired_count, Some(600));
        assert_eq!(report.ignored_pairs_after_stop, Some(600 - 101));
        assert_eq!(report.stopping_reason, Some("lower_bound_crossed"));
    }

    #[test]
    fn crossing_upper_before_minimum_that_settles_back_stays_inconclusive() {
        let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();
        let records = pentanomial_records_from_buckets(&crosses_upper_then_settles_inconclusive());
        // At pair 200 (past the 195-pair reversion point, verified while constructing this
        // fixture), the accumulated LLR is already back within bounds - so gating on
        // min_paired_ids=200 must not resurrect the earlier, now-irrelevant pair-104 crossing.
        let report = run(
            ok_iter(&records),
            &config,
            SprtVariant::Pentanomial,
            true,
            FailurePolicy::ReportOnly,
            false,
            Some(200),
            None,
        )
        .unwrap();
        assert_eq!(report.verdict, Verdict::Inconclusive);
        assert_eq!(report.stopping_pair_count, None);
        assert_eq!(report.stopping_reason, Some("insufficient_data"));
        assert_eq!(report.paired_count, Some(600));
    }

    #[test]
    fn crossing_lower_before_minimum_that_settles_back_stays_inconclusive() {
        let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();
        let records = pentanomial_records_from_buckets(&crosses_lower_then_settles_inconclusive());
        // Mirror of the upper case: back within bounds by pair 201 (verified while constructing
        // this fixture), so min_paired_ids=250 must not resurrect the pair-101 lower crossing.
        let report = run(
            ok_iter(&records),
            &config,
            SprtVariant::Pentanomial,
            true,
            FailurePolicy::ReportOnly,
            false,
            Some(250),
            None,
        )
        .unwrap();
        assert_eq!(report.verdict, Verdict::Inconclusive);
        assert_eq!(report.stopping_pair_count, None);
        assert_eq!(report.stopping_reason, Some("insufficient_data"));
        assert_eq!(report.paired_count, Some(600));
    }

    #[test]
    fn max_paired_ids_ignores_a_crossing_that_happens_after_the_cutoff() {
        let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();
        let records = pentanomial_records_from_buckets(&crosses_upper_then_settles_inconclusive());
        // The same file crosses upper at pair 104 if let run - but max_paired_ids=90 stops the
        // walk before that ever happens, so this must land Inconclusive at pair 90, not Pass.
        let report = run(
            ok_iter(&records),
            &config,
            SprtVariant::Pentanomial,
            true,
            FailurePolicy::ReportOnly,
            false,
            None,
            Some(90),
        )
        .unwrap();
        assert_eq!(report.verdict, Verdict::Inconclusive);
        assert_eq!(report.stopping_pair_count, Some(90));
        assert_eq!(report.stopping_reason, Some("max_paired_ids_reached"));
        assert_eq!(report.ignored_pairs_after_stop, Some(600 - 90));
    }

    #[test]
    fn odd_length_prefix_is_an_error_even_length_prefix_is_not() {
        let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();
        let records = pentanomial_records(10, ("candidate_win", "baseline_win"));
        // A prefix ending mid-pair (odd number of records) - the shape a poller would see if it
        // ever checked between a pair's two records instead of at a completed-pair boundary.
        let mut odd_prefix = records.clone();
        odd_prefix.truncate(7);
        assert!(matches!(
            run(
                ok_iter(&odd_prefix),
                &config,
                SprtVariant::Pentanomial,
                true,
                FailurePolicy::ReportOnly,
                false,
                None,
                None,
            ),
            Err(VeridictError::SchemaMismatch {
                context: "pentanomial",
                ..
            })
        ));
        // A prefix at a completed-pair boundary (even number of records) - an ordinary report.
        let mut even_prefix = records.clone();
        even_prefix.truncate(8);
        assert!(
            run(
                ok_iter(&even_prefix),
                &config,
                SprtVariant::Pentanomial,
                true,
                FailurePolicy::ReportOnly,
                false,
                None,
                None,
            )
            .is_ok()
        );
    }
}
