//! Turns raw records into a metric's effect size and confidence interval.
//!
//! A record is "used" if it contributes to either the status tally
//! (timeout/crash/invalid) or the chosen metric's calculation. A record that
//! matches neither is rejected as `SchemaMismatch` rather than silently
//! dropped, per AGENTS.md's "never silently ignore invalid data" rule.
//!
//! Each metric is an independent `MetricAggregator` (see the per-metric
//! submodules), so running several `--metric` flags together scans the
//! input once (`compute_many`), feeding every record to every requested
//! metric's aggregator, rather than one full pass per metric.

mod common;
mod elo;
mod mean_diff;
mod quantile_diff;
mod sign_test;
mod winrate;

pub(crate) use common::{DiffCollector, OutcomeCollector};
use serde::Serialize;
use std::collections::HashMap;

use crate::error::VeridictError;
use crate::input::Record;
use crate::{
    BootstrapMethod, CiMethod, FailurePolicy, IntoRecordResult, MetricConfig, MetricKind, Outcome,
    TrialStatus,
};

/// Per-side failure tally, so a report can distinguish "the baseline kept
/// crashing" from "the candidate kept timing out" instead of one opaque
/// combined number.
#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct FailureCounts {
    pub timeout: u64,
    pub crash: u64,
    pub invalid: u64,
}

#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct FailureBreakdown {
    pub baseline: FailureCounts,
    pub candidate: FailureCounts,
}

/// Everything the report needs from a metric computation, before thresholds
/// are applied.
pub struct MetricOutput {
    pub effect: f64,
    pub ci_low: f64,
    pub ci_high: f64,
    pub baseline_count: u64,
    pub candidate_count: u64,
    pub paired_count: u64,
    pub timeouts: u64,
    pub crashes: u64,
    pub invalid: u64,
    pub failures: FailureBreakdown,
    /// Set when there were zero trials usable by the metric; the caller must
    /// treat this as Inconclusive rather than running it through thresholds.
    pub warning: Option<String>,
    /// Id-repetition stats, tracked centrally in `compute_many` (not by any
    /// one aggregator - it applies identically no matter which metric is
    /// requested). Every aggregator sets these to `0` in its own `finish()`;
    /// `compute_many` overwrites them with the real values on every output
    /// before returning. `0` for both when `paired_by_id` is set - paired
    /// mode already has its own meaning for repeated ids (`SchemaMismatch`
    /// on >2-per-id), so this tracking is skipped there entirely.
    pub records_with_id: u64,
    /// The largest number of records sharing any single id.
    pub max_id_count: u64,
    /// The quantile `quantile-diff` measured (e.g. `0.95` for p95), so the report can say which
    /// one. `None` for every other metric.
    pub quantile: Option<f64>,
}

/// One metric's independent, incremental computation. `ingest` is called once per record
/// (`baseline_status`/`candidate_status` are parsed once per record by the shared driver below,
/// since status-tally logic is identical regardless of which metric is running - today that
/// work would otherwise be redone per metric). Passed as parsed `TrialStatus`, not a resolved
/// `Outcome`: `mean-diff`/`sign-test` have no use for an outcome (they only ever check
/// presence, via `.is_some()`), while `winrate`/`elo` resolve it themselves via
/// `effective_outcome` using their own stored `FailurePolicy` - a shared "resolved outcome"
/// wouldn't fit the numeric metrics at all. If this aggregator finds none of its own required
/// fields on a record AND both statuses are `None`, it must reject with `SchemaMismatch` - this
/// preserves strict per-metric validation even when several metrics share one pass: a record
/// usable by metric A but not metric B still errors when B is requested, exactly as if B had
/// run alone.
pub(crate) trait MetricAggregator {
    fn ingest(
        &mut self,
        line: usize,
        record: &Record,
        baseline_status: Option<TrialStatus>,
        candidate_status: Option<TrialStatus>,
    ) -> Result<(), VeridictError>;
    fn finish(self: Box<Self>, failures: &FailureBreakdown) -> Result<MetricOutput, VeridictError>;
}

fn build_aggregator(
    config: MetricConfig,
    confidence: f64,
    resamples: usize,
    seed: u64,
    paired_by_id: bool,
) -> Box<dyn MetricAggregator> {
    match config {
        MetricConfig::WinRate {
            ci_method,
            failure_policy,
        } => Box::new(winrate::WinRateAggregator::new(
            confidence,
            paired_by_id,
            ci_method,
            failure_policy,
        )),
        MetricConfig::Elo { failure_policy } => Box::new(elo::EloAggregator::new(
            confidence,
            paired_by_id,
            failure_policy,
        )),
        MetricConfig::MeanDiff { bootstrap_method } => {
            Box::new(mean_diff::MeanDiffAggregator::new(
                confidence,
                resamples,
                seed,
                paired_by_id,
                bootstrap_method,
            ))
        }
        MetricConfig::SignTest { ci_method } => Box::new(sign_test::SignTestAggregator::new(
            confidence,
            paired_by_id,
            ci_method,
        )),
        MetricConfig::QuantileDiff {
            quantile,
            bootstrap_method,
        } => Box::new(quantile_diff::QuantileDiffAggregator::new(
            confidence,
            resamples,
            seed,
            paired_by_id,
            quantile,
            bootstrap_method,
        )),
    }
}

/// Runs several metrics against the same records in a single pass: every
/// record is fed to every requested metric's aggregator, instead of
/// re-scanning `records` once per metric. `records` is consumed as a
/// streaming iterator, not a slice - `main.rs::read_records` hands this a
/// lazy iterator straight from the input file/stdin, so callers that only
/// need `winrate`/`elo`/`sign-test` (never `mean-diff`) see genuinely
/// bounded memory regardless of input size (`--paired-by-id` buffers
/// unresolved ids, which scales with distinct in-flight ids, not total
/// record count - see each aggregator's module doc). `mean-diff`'s
/// bootstrap always needs the full sample materialized internally
/// (`DiffCollector`'s `Vec<f64>`); that floor is inherent to random-access
/// resampling and out of scope to remove. `paired_by_id`: see each
/// aggregator's module doc - two records sharing the same `id` are combined
/// into one net observation instead of two independent ones. Each
/// `MetricConfig` is valid by construction (see `MetricConfig::new`), so
/// there's no ci_method/bootstrap_method compatibility check here anymore -
/// it moved to construction time, once, instead of running on every call.
/// `records_with_id`/`max_id_count` (see `MetricOutput`'s doc) are tracked
/// here, centrally, rather than per-aggregator - it's the same fact
/// regardless of which metrics are requested, and per-aggregator tracking is
/// exactly how `OutcomeCollector` (no duplicate-id check) and
/// `DiffCollector`/`SignCounts` (hard error on any duplicate) ended up with
/// different behavior in the first place.
pub fn compute_many<I>(
    records: I,
    metrics: &[MetricConfig],
    confidence: f64,
    resamples: usize,
    seed: u64,
    paired_by_id: bool,
) -> Result<Vec<MetricOutput>, VeridictError>
where
    I: IntoIterator,
    I::Item: IntoRecordResult,
{
    let mut records = records
        .into_iter()
        .map(IntoRecordResult::into_record_result)
        .peekable();
    if records.peek().is_none() {
        return Err(VeridictError::EmptyInput);
    }
    if !confidence.is_finite() || confidence <= 0.0 || confidence >= 1.0 {
        return Err(VeridictError::InvalidConfidence(confidence));
    }

    let mut failures = FailureBreakdown::default();
    let mut aggregators: Vec<Box<dyn MetricAggregator>> = metrics
        .iter()
        .map(|&config| build_aggregator(config, confidence, resamples, seed, paired_by_id))
        .collect();

    // Paired mode already has its own meaning for a repeated id (nets to one
    // observation, or a hard `SchemaMismatch` past 2) - this tracking is only
    // about the unpaired gap, so it's skipped entirely when paired_by_id.
    let mut id_counts: HashMap<String, u64> = HashMap::new();
    let mut records_with_id: u64 = 0;

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
        if !paired_by_id && let Some(id) = record.id.as_deref() {
            records_with_id += 1;
            *id_counts.entry(id.to_string()).or_insert(0) += 1;
        }
        for agg in &mut aggregators {
            agg.ingest(line, &record, baseline_status, candidate_status)?;
        }
    }
    let max_id_count = id_counts.values().copied().max().unwrap_or(0);

    aggregators
        .into_iter()
        .map(|agg| {
            agg.finish(&failures).map(|mut out| {
                out.records_with_id = records_with_id;
                out.max_id_count = max_id_count;
                out
            })
        })
        .collect()
}

/// Single-metric convenience wrapper around [`compute_many`].
pub fn compute<I>(
    records: I,
    metric: MetricConfig,
    confidence: f64,
    resamples: usize,
    seed: u64,
    paired_by_id: bool,
) -> Result<MetricOutput, VeridictError>
where
    I: IntoIterator,
    I::Item: IntoRecordResult,
{
    compute_many(
        records,
        std::slice::from_ref(&metric),
        confidence,
        resamples,
        seed,
        paired_by_id,
    )
    .map(|mut outs| outs.remove(0))
}

/// A record-level `SchemaMismatch` needs a short label for what it failed
/// to match; matches the CLI's `--metric` spelling.
pub(crate) fn metric_label(metric: MetricKind) -> &'static str {
    match metric {
        MetricKind::WinRate => "metric winrate",
        MetricKind::MeanDiff => "metric mean-diff",
        MetricKind::SignTest => "metric sign-test",
        MetricKind::Elo => "metric elo",
        MetricKind::QuantileDiff => "metric quantile-diff",
    }
}

/// A label for `IncompatibleCiMethod`'s error message; matches the CLI's
/// `--ci-method` spelling. Only ever reached for a non-`Wilson` method (the
/// upfront guard above never fires for `Wilson`), but kept total rather than
/// `unreachable!()` for that arm, per AGENTS.md's "boring, explicit" rule.
pub(crate) fn ci_method_label(ci_method: CiMethod) -> &'static str {
    match ci_method {
        CiMethod::Wilson => "wilson",
        CiMethod::Exact => "exact",
        CiMethod::Jeffreys => "jeffreys",
    }
}

/// A label for `IncompatibleFailurePolicy`'s error message; matches the CLI's `--failure-policy`
/// spelling. Only ever reached for a non-`ReportOnly` policy (the upfront guard in
/// `MetricConfig::new` never fires for `ReportOnly`), but kept total rather than `unreachable!()`
/// for that arm, same precedent as `ci_method_label`.
pub(crate) fn failure_policy_label(policy: FailurePolicy) -> &'static str {
    match policy {
        FailurePolicy::ReportOnly => "report-only",
        FailurePolicy::Exclude => "exclude",
        FailurePolicy::Loss => "loss",
    }
}

/// A label for `IncompatibleBootstrapMethod`'s error message; matches the CLI's
/// `--bootstrap-method` spelling. Only ever reached for `Bca` (the upfront guard in
/// `MetricConfig::new` never fires for `Percentile`/`Basic`), but kept total rather than
/// `unreachable!()` for those arms, same precedent as `ci_method_label`.
pub(crate) fn bootstrap_method_label(method: BootstrapMethod) -> &'static str {
    match method {
        BootstrapMethod::Percentile => "percentile",
        BootstrapMethod::Bca => "bca",
        BootstrapMethod::Basic => "basic",
    }
}

/// Parses `raw` and tallies it into `counts` if it's a failure - returns the parsed
/// `TrialStatus` either way (not just `Ok(())`) so callers can also feed it to
/// `effective_outcome` without re-parsing the same string a second time.
pub(crate) fn tally_status(
    raw: &str,
    line: usize,
    field: &'static str,
    counts: &mut FailureCounts,
) -> Result<TrialStatus, VeridictError> {
    match TrialStatus::parse(raw) {
        Some(TrialStatus::Ok) => Ok(TrialStatus::Ok),
        Some(status @ TrialStatus::Timeout) => {
            counts.timeout += 1;
            Ok(status)
        }
        Some(status @ TrialStatus::Crash) => {
            counts.crash += 1;
            Ok(status)
        }
        Some(status @ TrialStatus::Invalid) => {
            counts.invalid += 1;
            Ok(status)
        }
        None => Err(VeridictError::UnrecognizedStatus {
            line,
            field,
            value: raw.to_string(),
        }),
    }
}

/// The outcome to actually record for win/loss/draw-shaped metrics (`winrate`/`elo`) and `sprt`,
/// given the record's parsed per-side status and `--failure-policy`. `ReportOnly` is exactly
/// today's pre-`FailurePolicy` behavior, unchanged: a failure is tallied by the caller (via
/// `tally_status`, before this runs) but never itself contributes an outcome - only a literal
/// `result` field does. `Exclude`/`Loss` only diverge from `ReportOnly` when a failure status is
/// present (a status-only record, the common case, already contributes nothing under any
/// policy).
///
/// Neither `Exclude` nor `Loss` validates `result`'s contents when a failure status is present -
/// once a policy has decided the trial's outcome is excluded or overridden, an unparseable
/// `result` string alongside it is moot, not silently-ignored invalid data (the ordinary
/// `UnrecognizedOutcome` check still applies whenever `result` is actually consulted, i.e.
/// whenever neither side failed, or under `ReportOnly`).
pub(crate) fn effective_outcome(
    policy: FailurePolicy,
    baseline_status: Option<TrialStatus>,
    candidate_status: Option<TrialStatus>,
    result: Option<&str>,
    line: usize,
) -> Result<Option<Outcome>, VeridictError> {
    let baseline_failed = matches!(baseline_status, Some(s) if s != TrialStatus::Ok);
    let candidate_failed = matches!(candidate_status, Some(s) if s != TrialStatus::Ok);

    match policy {
        FailurePolicy::ReportOnly => parse_result_outcome(result, line),
        FailurePolicy::Exclude if baseline_failed || candidate_failed => Ok(None),
        FailurePolicy::Exclude => parse_result_outcome(result, line),
        // A failure status overrides any literal `result` on the same record - trusting the
        // execution-level failure signal is the conservative choice (AGENTS.md: "a false pass
        // is worse than an inconclusive result"), so a `candidate_win` result next to a
        // `candidate_status: crash` can never silently override the crash.
        FailurePolicy::Loss => match (baseline_failed, candidate_failed) {
            (true, true) => Ok(Some(Outcome::Draw)),
            (true, false) => Ok(Some(Outcome::CandidateWin)),
            (false, true) => Ok(Some(Outcome::BaselineWin)),
            (false, false) => parse_result_outcome(result, line),
        },
    }
}

fn parse_result_outcome(
    result: Option<&str>,
    line: usize,
) -> Result<Option<Outcome>, VeridictError> {
    match result {
        None => Ok(None),
        Some(r) => match Outcome::parse(r) {
            Some(o) => Ok(Some(o)),
            None => Err(VeridictError::UnrecognizedOutcome {
                line,
                value: r.to_string(),
                expected: "baseline_win|candidate_win|draw",
            }),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BootstrapMethod;

    const SEED: u64 = crate::stats::bootstrap::DEFAULT_SEED;

    fn rec(
        id: &str,
        baseline: Option<f64>,
        candidate: Option<f64>,
        result: Option<&str>,
        bs: Option<&str>,
        cs: Option<&str>,
    ) -> Record {
        Record {
            id: Some(id.to_string()),
            baseline,
            candidate,
            result: result.map(str::to_string),
            baseline_status: bs.map(str::to_string),
            candidate_status: cs.map(str::to_string),
        }
    }

    #[test]
    fn effective_outcome_report_only_ignores_a_status_only_failure() {
        let out = effective_outcome(
            FailurePolicy::ReportOnly,
            Some(TrialStatus::Crash),
            None,
            None,
            1,
        )
        .unwrap();
        assert_eq!(out, None);
    }

    #[test]
    fn effective_outcome_report_only_still_honors_a_result_next_to_a_failure() {
        // Mixed status+result: `ReportOnly` keeps today's pre-`FailurePolicy` behavior - the
        // literal `result` still counts, the failure status is only tallied, not consulted here.
        let out = effective_outcome(
            FailurePolicy::ReportOnly,
            None,
            Some(TrialStatus::Crash),
            Some("candidate_win"),
            1,
        )
        .unwrap();
        assert_eq!(out, Some(Outcome::CandidateWin));
    }

    #[test]
    fn effective_outcome_exclude_drops_a_result_next_to_a_failure() {
        let out = effective_outcome(
            FailurePolicy::Exclude,
            None,
            Some(TrialStatus::Crash),
            Some("candidate_win"),
            1,
        )
        .unwrap();
        assert_eq!(out, None);
    }

    #[test]
    fn effective_outcome_exclude_matches_report_only_without_a_failure() {
        let out = effective_outcome(FailurePolicy::Exclude, None, None, Some("candidate_win"), 1)
            .unwrap();
        assert_eq!(out, Some(Outcome::CandidateWin));
    }

    #[test]
    fn effective_outcome_loss_synthesizes_baseline_win_on_a_candidate_failure() {
        let out = effective_outcome(
            FailurePolicy::Loss,
            None,
            Some(TrialStatus::Timeout),
            None,
            1,
        )
        .unwrap();
        assert_eq!(out, Some(Outcome::BaselineWin));
    }

    #[test]
    fn effective_outcome_loss_synthesizes_candidate_win_on_a_baseline_failure() {
        let out = effective_outcome(FailurePolicy::Loss, Some(TrialStatus::Crash), None, None, 1)
            .unwrap();
        assert_eq!(out, Some(Outcome::CandidateWin));
    }

    #[test]
    fn effective_outcome_loss_both_sides_failing_nets_to_a_draw() {
        let out = effective_outcome(
            FailurePolicy::Loss,
            Some(TrialStatus::Invalid),
            Some(TrialStatus::Crash),
            None,
            1,
        )
        .unwrap();
        assert_eq!(out, Some(Outcome::Draw));
    }

    #[test]
    fn effective_outcome_loss_overrides_a_literal_result() {
        // The crash wins even though `result` says `candidate_win` - AGENTS.md's "a false pass
        // is worse than an inconclusive result" means the execution-level failure signal is
        // trusted over a same-record `result` field, not the other way around.
        let out = effective_outcome(
            FailurePolicy::Loss,
            None,
            Some(TrialStatus::Crash),
            Some("candidate_win"),
            1,
        )
        .unwrap();
        assert_eq!(out, Some(Outcome::BaselineWin));
    }

    #[test]
    fn effective_outcome_loss_falls_through_to_result_without_a_failure() {
        let out =
            effective_outcome(FailurePolicy::Loss, None, None, Some("candidate_win"), 1).unwrap();
        assert_eq!(out, Some(Outcome::CandidateWin));
    }

    #[test]
    fn effective_outcome_malformed_result_next_to_a_failure_is_not_an_error_under_exclude() {
        // Once a policy has decided the trial's outcome is excluded, an unparseable `result`
        // alongside it is moot, not silently-ignored invalid data - see `effective_outcome`'s doc.
        let out = effective_outcome(
            FailurePolicy::Exclude,
            None,
            Some(TrialStatus::Crash),
            Some("not-a-real-outcome"),
            1,
        )
        .unwrap();
        assert_eq!(out, None);
    }

    #[test]
    fn effective_outcome_malformed_result_is_still_an_error_when_actually_consulted() {
        assert!(matches!(
            effective_outcome(
                FailurePolicy::ReportOnly,
                None,
                None,
                Some("not-a-real-outcome"),
                1
            ),
            Err(VeridictError::UnrecognizedOutcome { .. })
        ));
    }

    #[test]
    fn winrate_excludes_draws_from_n() {
        let records = [
            (1, rec("a", None, None, Some("candidate_win"), None, None)),
            (2, rec("b", None, None, Some("baseline_win"), None, None)),
            (3, rec("c", None, None, Some("draw"), None, None)),
        ];
        let out = compute(
            records.iter().cloned(),
            MetricConfig::WinRate {
                ci_method: CiMethod::Wilson,
                failure_policy: FailurePolicy::ReportOnly,
            },
            0.95,
            1000,
            SEED,
            false,
        )
        .unwrap();
        assert_eq!(out.paired_count, 2);
        assert_eq!(out.candidate_count, 1);
    }

    #[test]
    fn winrate_all_draws_is_zero_n_warning() {
        let records = [(1, rec("a", None, None, Some("draw"), None, None))];
        let out = compute(
            records.iter().cloned(),
            MetricConfig::WinRate {
                ci_method: CiMethod::Wilson,
                failure_policy: FailurePolicy::ReportOnly,
            },
            0.95,
            1000,
            SEED,
            false,
        )
        .unwrap();
        assert!(out.warning.is_some());
        assert_eq!(out.paired_count, 0);
    }

    #[test]
    fn mean_diff_paired_count_and_effect() {
        let records = [
            (1, rec("a", Some(1.0), Some(1.5), None, None, None)),
            (2, rec("b", Some(2.0), Some(2.5), None, None, None)),
        ];
        let out = compute(
            records.iter().cloned(),
            MetricConfig::MeanDiff {
                bootstrap_method: BootstrapMethod::Percentile,
            },
            0.95,
            1000,
            SEED,
            false,
        )
        .unwrap();
        assert_eq!(out.paired_count, 2);
        assert!((out.effect - 0.5).abs() < 1e-9);
    }

    #[test]
    fn mean_diff_rejects_nan() {
        let records = [(1, rec("a", Some(f64::NAN), Some(1.0), None, None, None))];
        let result = compute(
            records.iter().cloned(),
            MetricConfig::MeanDiff {
                bootstrap_method: BootstrapMethod::Percentile,
            },
            0.95,
            1000,
            SEED,
            false,
        );
        assert!(matches!(
            result,
            Err(VeridictError::InvalidValue {
                field: "baseline",
                ..
            })
        ));
    }

    #[test]
    fn mean_diff_rejects_duplicate_id() {
        let records = [
            (1, rec("dup", Some(1.0), Some(1.1), None, None, None)),
            (2, rec("dup", Some(2.0), Some(2.1), None, None, None)),
        ];
        let result = compute(
            records.iter().cloned(),
            MetricConfig::MeanDiff {
                bootstrap_method: BootstrapMethod::Percentile,
            },
            0.95,
            1000,
            SEED,
            false,
        );
        assert!(matches!(result, Err(VeridictError::DuplicateId { .. })));
    }

    #[test]
    fn status_only_records_count_but_are_not_schema_mismatch() {
        let records = [(1, rec("a", None, None, None, Some("ok"), Some("timeout")))];
        let out = compute(
            records.iter().cloned(),
            MetricConfig::WinRate {
                ci_method: CiMethod::Wilson,
                failure_policy: FailurePolicy::ReportOnly,
            },
            0.95,
            1000,
            SEED,
            false,
        )
        .unwrap();
        assert_eq!(out.timeouts, 1);
        assert_eq!(out.failures.candidate.timeout, 1);
        assert_eq!(out.failures.baseline.timeout, 0);
    }

    #[test]
    fn unusable_record_is_schema_mismatch() {
        let records = [(1, rec("a", None, None, None, None, None))];
        let result = compute(
            records.iter().cloned(),
            MetricConfig::WinRate {
                ci_method: CiMethod::Wilson,
                failure_policy: FailurePolicy::ReportOnly,
            },
            0.95,
            1000,
            SEED,
            false,
        );
        assert!(matches!(result, Err(VeridictError::SchemaMismatch { .. })));
    }

    #[test]
    fn unrecognized_status_is_an_error() {
        let records = [(1, rec("a", None, None, None, Some("bogus"), None))];
        let result = compute(
            records.iter().cloned(),
            MetricConfig::WinRate {
                ci_method: CiMethod::Wilson,
                failure_policy: FailurePolicy::ReportOnly,
            },
            0.95,
            1000,
            SEED,
            false,
        );
        assert!(matches!(
            result,
            Err(VeridictError::UnrecognizedStatus { .. })
        ));
    }

    #[test]
    fn unrecognized_outcome_is_an_error() {
        let records = [(1, rec("a", None, None, Some("bogus"), None, None))];
        let result = compute(
            records.iter().cloned(),
            MetricConfig::WinRate {
                ci_method: CiMethod::Wilson,
                failure_policy: FailurePolicy::ReportOnly,
            },
            0.95,
            1000,
            SEED,
            false,
        );
        assert!(matches!(
            result,
            Err(VeridictError::UnrecognizedOutcome { .. })
        ));
    }

    #[test]
    fn empty_records_is_empty_input() {
        let result = compute(
            std::iter::empty::<(usize, Record)>(),
            MetricConfig::WinRate {
                ci_method: CiMethod::Wilson,
                failure_policy: FailurePolicy::ReportOnly,
            },
            0.95,
            1000,
            SEED,
            false,
        );
        assert!(matches!(result, Err(VeridictError::EmptyInput)));
    }

    #[test]
    fn sign_test_excludes_ties() {
        let records = [
            (1, rec("a", Some(1.0), Some(1.5), None, None, None)), // positive
            (2, rec("b", Some(2.0), Some(1.0), None, None, None)), // negative
            (3, rec("c", Some(3.0), Some(3.0), None, None, None)), // tie, excluded
        ];
        let out = compute(
            records.iter().cloned(),
            MetricConfig::SignTest {
                ci_method: CiMethod::Wilson,
            },
            0.95,
            1000,
            SEED,
            false,
        )
        .unwrap();
        assert_eq!(out.paired_count, 2);
        assert_eq!(out.candidate_count, 1);
        assert_eq!(out.baseline_count, 1);
    }

    #[test]
    fn sign_test_all_ties_is_zero_n_warning() {
        let records = [(1, rec("a", Some(1.0), Some(1.0), None, None, None))];
        let out = compute(
            records.iter().cloned(),
            MetricConfig::SignTest {
                ci_method: CiMethod::Wilson,
            },
            0.95,
            1000,
            SEED,
            false,
        )
        .unwrap();
        assert!(out.warning.is_some());
    }

    #[test]
    fn elo_counts_draws_as_half_a_point() {
        let records = [
            (1, rec("a", None, None, Some("candidate_win"), None, None)),
            (2, rec("b", None, None, Some("draw"), None, None)),
        ];
        let out = compute(
            records.iter().cloned(),
            MetricConfig::Elo {
                failure_policy: FailurePolicy::ReportOnly,
            },
            0.95,
            1000,
            SEED,
            false,
        )
        .unwrap();
        // score = (1 + 0.5) / 2 = 0.75 -> a positive Elo effect.
        assert!(out.effect > 0.0);
        assert_eq!(out.paired_count, 2);
    }

    #[test]
    fn elo_even_record_is_zero_effect() {
        let records = [
            (1, rec("a", None, None, Some("candidate_win"), None, None)),
            (2, rec("b", None, None, Some("baseline_win"), None, None)),
        ];
        let out = compute(
            records.iter().cloned(),
            MetricConfig::Elo {
                failure_policy: FailurePolicy::ReportOnly,
            },
            0.95,
            1000,
            SEED,
            false,
        )
        .unwrap();
        assert!(out.effect.abs() < 1e-9);
    }

    #[test]
    fn elo_zero_trials_is_a_warning_not_an_error() {
        let records = [(1, rec("a", None, None, None, Some("timeout"), None))];
        let out = compute(
            records.iter().cloned(),
            MetricConfig::Elo {
                failure_policy: FailurePolicy::ReportOnly,
            },
            0.95,
            1000,
            SEED,
            false,
        )
        .unwrap();
        assert!(out.warning.is_some());
    }

    // --- paired_by_id ---

    #[test]
    fn paired_winrate_nets_two_games_per_id_by_points() {
        let records = [
            // id "op1": candidate wins one, loses the other -> net draw (1.0 pt).
            (1, rec("op1", None, None, Some("candidate_win"), None, None)),
            (2, rec("op1", None, None, Some("baseline_win"), None, None)),
            // id "op2": candidate wins both -> net candidate win (2.0 pts).
            (3, rec("op2", None, None, Some("candidate_win"), None, None)),
            (4, rec("op2", None, None, Some("candidate_win"), None, None)),
        ];
        let out = compute(
            records.iter().cloned(),
            MetricConfig::WinRate {
                ci_method: CiMethod::Wilson,
                failure_policy: FailurePolicy::ReportOnly,
            },
            0.95,
            1000,
            SEED,
            true,
        )
        .unwrap();
        // 4 raw games -> 2 paired samples: 1 draw (excluded from n), 1 candidate win.
        assert_eq!(out.paired_count, 1);
        assert_eq!(out.candidate_count, 1);
        assert_eq!(out.baseline_count, 0);
    }

    #[test]
    fn paired_winrate_unpaired_singleton_still_counts() {
        let records = [(
            1,
            rec("solo", None, None, Some("candidate_win"), None, None),
        )];
        let out = compute(
            records.iter().cloned(),
            MetricConfig::WinRate {
                ci_method: CiMethod::Wilson,
                failure_policy: FailurePolicy::ReportOnly,
            },
            0.95,
            1000,
            SEED,
            true,
        )
        .unwrap();
        assert_eq!(out.paired_count, 1);
        assert_eq!(out.candidate_count, 1);
    }

    #[test]
    fn paired_winrate_rejects_triple_id() {
        let records = [
            (1, rec("op1", None, None, Some("candidate_win"), None, None)),
            (2, rec("op1", None, None, Some("candidate_win"), None, None)),
            (3, rec("op1", None, None, Some("candidate_win"), None, None)),
        ];
        let result = compute(
            records.iter().cloned(),
            MetricConfig::WinRate {
                ci_method: CiMethod::Wilson,
                failure_policy: FailurePolicy::ReportOnly,
            },
            0.95,
            1000,
            SEED,
            true,
        );
        assert!(matches!(
            result,
            Err(VeridictError::SchemaMismatch {
                context: "paired-by-id",
                ..
            })
        ));
    }

    #[test]
    fn paired_mean_diff_averages_the_pair() {
        let records = [
            (1, rec("op1", Some(1.0), Some(1.2), None, None, None)), // diff +0.2
            (2, rec("op1", Some(1.0), Some(0.8), None, None, None)), // diff -0.2
        ];
        let out = compute(
            records.iter().cloned(),
            MetricConfig::MeanDiff {
                bootstrap_method: BootstrapMethod::Percentile,
            },
            0.95,
            1000,
            SEED,
            true,
        )
        .unwrap();
        assert_eq!(out.paired_count, 1);
        assert!(out.effect.abs() < 1e-9); // net-of-bias effect is ~0, not the two raw +-0.2 diffs
    }

    #[test]
    fn paired_mean_diff_allows_duplicate_id_that_unpaired_mode_rejects() {
        let records = [
            (1, rec("dup", Some(1.0), Some(1.1), None, None, None)),
            (2, rec("dup", Some(2.0), Some(2.1), None, None, None)),
        ];
        let result = compute(
            records.iter().cloned(),
            MetricConfig::MeanDiff {
                bootstrap_method: BootstrapMethod::Percentile,
            },
            0.95,
            1000,
            SEED,
            true,
        );
        assert!(result.is_ok());
    }

    #[test]
    fn paired_sign_test_rejects_triple_id() {
        let records = [
            (1, rec("op1", Some(1.0), Some(1.1), None, None, None)),
            (2, rec("op1", Some(1.0), Some(1.1), None, None, None)),
            (3, rec("op1", Some(1.0), Some(1.1), None, None, None)),
        ];
        let result = compute(
            records.iter().cloned(),
            MetricConfig::SignTest {
                ci_method: CiMethod::Wilson,
            },
            0.95,
            1000,
            SEED,
            true,
        );
        assert!(matches!(
            result,
            Err(VeridictError::SchemaMismatch {
                context: "paired-by-id",
                ..
            })
        ));
    }

    #[test]
    fn paired_elo_nets_by_points_too() {
        let records = [
            (1, rec("op1", None, None, Some("candidate_win"), None, None)),
            (2, rec("op1", None, None, Some("draw"), None, None)),
        ];
        // total = 1.0 + 0.5 = 1.5 pts across the pair -> net candidate win.
        let out = compute(
            records.iter().cloned(),
            MetricConfig::Elo {
                failure_policy: FailurePolicy::ReportOnly,
            },
            0.95,
            1000,
            SEED,
            true,
        )
        .unwrap();
        assert_eq!(out.paired_count, 1);
        assert!(out.effect > 0.0);
    }

    // --- compute_many: single-scan multi-metric behavior ---

    #[test]
    fn compute_many_rejects_record_unusable_by_one_of_several_metrics() {
        // Usable by MeanDiff (has baseline/candidate) but not by WinRate (no result, no status).
        let records = [(1, rec("a", Some(1.0), Some(1.1), None, None, None))];
        let result = compute_many(
            records.iter().cloned(),
            &[
                MetricConfig::MeanDiff {
                    bootstrap_method: BootstrapMethod::Percentile,
                },
                MetricConfig::WinRate {
                    ci_method: CiMethod::Wilson,
                    failure_policy: FailurePolicy::ReportOnly,
                },
            ],
            0.95,
            1000,
            SEED,
            false,
        );
        assert!(matches!(result, Err(VeridictError::SchemaMismatch { .. })));
    }

    #[test]
    fn compute_many_matches_independent_compute_calls() {
        let records = [
            (
                1,
                rec("a", Some(1.0), Some(1.5), Some("candidate_win"), None, None),
            ),
            (
                2,
                rec("b", Some(2.0), Some(1.5), Some("baseline_win"), None, None),
            ),
            (3, rec("c", Some(1.0), Some(1.0), Some("draw"), None, None)),
        ];
        let combined = compute_many(
            records.iter().cloned(),
            &[
                MetricConfig::WinRate {
                    ci_method: CiMethod::Wilson,
                    failure_policy: FailurePolicy::ReportOnly,
                },
                MetricConfig::MeanDiff {
                    bootstrap_method: BootstrapMethod::Percentile,
                },
                MetricConfig::SignTest {
                    ci_method: CiMethod::Wilson,
                },
            ],
            0.95,
            1000,
            SEED,
            false,
        )
        .unwrap();
        let winrate = compute(
            records.iter().cloned(),
            MetricConfig::WinRate {
                ci_method: CiMethod::Wilson,
                failure_policy: FailurePolicy::ReportOnly,
            },
            0.95,
            1000,
            SEED,
            false,
        )
        .unwrap();
        let mean_diff = compute(
            records.iter().cloned(),
            MetricConfig::MeanDiff {
                bootstrap_method: BootstrapMethod::Percentile,
            },
            0.95,
            1000,
            SEED,
            false,
        )
        .unwrap();
        let sign_test = compute(
            records.iter().cloned(),
            MetricConfig::SignTest {
                ci_method: CiMethod::Wilson,
            },
            0.95,
            1000,
            SEED,
            false,
        )
        .unwrap();
        assert_eq!(combined[0].paired_count, winrate.paired_count);
        assert!((combined[0].effect - winrate.effect).abs() < 1e-12);
        assert_eq!(combined[1].paired_count, mean_diff.paired_count);
        assert!((combined[1].effect - mean_diff.effect).abs() < 1e-12);
        assert_eq!(combined[2].paired_count, sign_test.paired_count);
        assert!((combined[2].effect - sign_test.effect).abs() < 1e-12);
    }
}
