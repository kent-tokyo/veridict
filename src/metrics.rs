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
mod sign_test;
mod winrate;

pub(crate) use common::OutcomeCollector;
use serde::Serialize;

use crate::error::VeridictError;
use crate::input::Record;
use crate::{CiMethod, IntoRecordResult, MetricConfig, MetricKind, TrialStatus};

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
}

/// One metric's independent, incremental computation. `ingest` is called
/// once per record (`has_status` is computed once per record by the shared
/// driver below, since status-tally logic is identical regardless of which
/// metric is running - today that work would otherwise be redone per
/// metric). If this aggregator finds none of its own required fields on a
/// record AND `has_status` is false, it must reject with `SchemaMismatch` -
/// this preserves strict per-metric validation even when several metrics
/// share one pass: a record usable by metric A but not metric B still
/// errors when B is requested, exactly as if B had run alone.
pub(crate) trait MetricAggregator {
    fn ingest(
        &mut self,
        line: usize,
        record: &Record,
        has_status: bool,
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
        MetricConfig::WinRate { ci_method } => Box::new(winrate::WinRateAggregator::new(
            confidence,
            paired_by_id,
            ci_method,
        )),
        MetricConfig::Elo => Box::new(elo::EloAggregator::new(confidence, paired_by_id)),
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

    for item in records {
        let (line, record) = item?;
        let mut has_status = false;
        if let Some(status) = record.baseline_status.as_deref() {
            has_status = true;
            tally_status(status, line, "baseline_status", &mut failures.baseline)?;
        }
        if let Some(status) = record.candidate_status.as_deref() {
            has_status = true;
            tally_status(status, line, "candidate_status", &mut failures.candidate)?;
        }
        for agg in &mut aggregators {
            agg.ingest(line, &record, has_status)?;
        }
    }

    aggregators
        .into_iter()
        .map(|agg| agg.finish(&failures))
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

pub(crate) fn tally_status(
    raw: &str,
    line: usize,
    field: &'static str,
    counts: &mut FailureCounts,
) -> Result<(), VeridictError> {
    match TrialStatus::parse(raw) {
        Some(TrialStatus::Ok) => Ok(()),
        Some(TrialStatus::Timeout) => {
            counts.timeout += 1;
            Ok(())
        }
        Some(TrialStatus::Crash) => {
            counts.crash += 1;
            Ok(())
        }
        Some(TrialStatus::Invalid) => {
            counts.invalid += 1;
            Ok(())
        }
        None => Err(VeridictError::UnrecognizedStatus {
            line,
            field,
            value: raw.to_string(),
        }),
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
            MetricConfig::Elo,
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
            MetricConfig::Elo,
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
            MetricConfig::Elo,
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
            MetricConfig::Elo,
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
