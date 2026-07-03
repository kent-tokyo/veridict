//! Turns raw records into a metric's effect size and confidence interval.
//!
//! A record is "used" if it contributes to either the status tally
//! (timeout/crash/invalid) or the chosen metric's calculation. A record that
//! matches neither is rejected as `SchemaMismatch` rather than silently
//! dropped, per AGENTS.md's "never silently ignore invalid data" rule.

use std::collections::HashSet;

use crate::error::VeridictError;
use crate::input::Record;
use crate::stats::{bootstrap, wilson};
use crate::{MetricKind, TrialStatus};

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
    /// Set when there were zero trials usable by the metric; the caller must
    /// treat this as Inconclusive rather than running it through thresholds.
    pub warning: Option<String>,
}

pub fn compute(
    records: &[(usize, Record)],
    metric: MetricKind,
    confidence: f64,
    resamples: usize,
) -> Result<MetricOutput, VeridictError> {
    if records.is_empty() {
        return Err(VeridictError::EmptyInput);
    }
    if !confidence.is_finite() || confidence <= 0.0 || confidence >= 1.0 {
        return Err(VeridictError::InvalidConfidence(confidence));
    }

    let mut timeouts = 0u64;
    let mut crashes = 0u64;
    let mut invalid = 0u64;

    let mut baseline_wins = 0u64;
    let mut candidate_wins = 0u64;

    let mut diffs: Vec<f64> = Vec::new();
    let mut seen_ids: HashSet<&str> = HashSet::new();

    for (line, record) in records {
        let mut used = false;

        if let Some(status) = record.baseline_status.as_deref() {
            used = true;
            tally_status(
                status,
                *line,
                "baseline_status",
                &mut timeouts,
                &mut crashes,
                &mut invalid,
            )?;
        }
        if let Some(status) = record.candidate_status.as_deref() {
            used = true;
            tally_status(
                status,
                *line,
                "candidate_status",
                &mut timeouts,
                &mut crashes,
                &mut invalid,
            )?;
        }

        match metric {
            MetricKind::WinRate => {
                if let Some(result) = record.result.as_deref() {
                    used = true;
                    match crate::Outcome::parse(result) {
                        Some(crate::Outcome::BaselineWin) => baseline_wins += 1,
                        Some(crate::Outcome::CandidateWin) => candidate_wins += 1,
                        Some(crate::Outcome::Draw) => {}
                        None => {
                            return Err(VeridictError::UnrecognizedOutcome {
                                line: *line,
                                value: result.to_string(),
                            });
                        }
                    }
                }
            }
            MetricKind::MeanDiff => {
                if let (Some(b), Some(c)) = (record.baseline, record.candidate) {
                    if !b.is_finite() {
                        return Err(VeridictError::InvalidValue {
                            line: *line,
                            field: "baseline",
                            value: b,
                        });
                    }
                    if !c.is_finite() {
                        return Err(VeridictError::InvalidValue {
                            line: *line,
                            field: "candidate",
                            value: c,
                        });
                    }
                    if let Some(id) = record.id.as_deref()
                        && !seen_ids.insert(id)
                    {
                        return Err(VeridictError::DuplicateId {
                            id: id.to_string(),
                            line: *line,
                        });
                    }
                    used = true;
                    diffs.push(c - b);
                }
            }
        }

        if !used {
            return Err(VeridictError::SchemaMismatch {
                line: *line,
                metric,
                detail: "record has no fields usable by this metric and no status fields"
                    .to_string(),
            });
        }
    }

    match metric {
        MetricKind::WinRate => Ok(compute_winrate(
            baseline_wins,
            candidate_wins,
            confidence,
            timeouts,
            crashes,
            invalid,
        )?),
        MetricKind::MeanDiff => Ok(compute_mean_diff(
            &diffs, confidence, resamples, timeouts, crashes, invalid,
        )),
    }
}

fn tally_status(
    raw: &str,
    line: usize,
    field: &'static str,
    timeouts: &mut u64,
    crashes: &mut u64,
    invalid: &mut u64,
) -> Result<(), VeridictError> {
    match TrialStatus::parse(raw) {
        Some(TrialStatus::Ok) => Ok(()),
        Some(TrialStatus::Timeout) => {
            *timeouts += 1;
            Ok(())
        }
        Some(TrialStatus::Crash) => {
            *crashes += 1;
            Ok(())
        }
        Some(TrialStatus::Invalid) => {
            *invalid += 1;
            Ok(())
        }
        None => Err(VeridictError::UnrecognizedStatus {
            line,
            field,
            value: raw.to_string(),
        }),
    }
}

/// Win rate is computed over decisive (non-draw) trials only: `p_hat =
/// candidate_wins / (candidate_wins + baseline_wins)`. Draws are counted
/// among "used" records but excluded from the proportion, matching standard
/// paired-match testing practice (a draw is neither a win nor a loss).
/// `effect`/`ci_low`/`ci_high` are centered on 0 (deviation from a 50/50
/// split) so they compose with `--min-effect`/`--pass-above`/`--fail-below`,
/// which are expressed as signed deltas.
fn compute_winrate(
    baseline_wins: u64,
    candidate_wins: u64,
    confidence: f64,
    timeouts: u64,
    crashes: u64,
    invalid: u64,
) -> Result<MetricOutput, VeridictError> {
    let n = baseline_wins + candidate_wins;
    if n == 0 {
        return Ok(MetricOutput {
            effect: 0.0,
            ci_low: 0.0,
            ci_high: 0.0,
            baseline_count: 0,
            candidate_count: 0,
            paired_count: 0,
            timeouts,
            crashes,
            invalid,
            warning: Some("no decisive (non-draw) trials to compute win rate".to_string()),
        });
    }
    let (lo, hi) = wilson::wilson_ci(candidate_wins, n, confidence)?;
    let p_hat = candidate_wins as f64 / n as f64;
    Ok(MetricOutput {
        effect: p_hat - 0.5,
        ci_low: lo - 0.5,
        ci_high: hi - 0.5,
        baseline_count: baseline_wins,
        candidate_count: candidate_wins,
        paired_count: n,
        timeouts,
        crashes,
        invalid,
        warning: None,
    })
}

fn compute_mean_diff(
    diffs: &[f64],
    confidence: f64,
    resamples: usize,
    timeouts: u64,
    crashes: u64,
    invalid: u64,
) -> MetricOutput {
    if diffs.is_empty() {
        return MetricOutput {
            effect: 0.0,
            ci_low: 0.0,
            ci_high: 0.0,
            baseline_count: 0,
            candidate_count: 0,
            paired_count: 0,
            timeouts,
            crashes,
            invalid,
            warning: Some("no paired numeric trials to compute mean difference".to_string()),
        };
    }
    let effect = bootstrap::mean(diffs);
    let (ci_low, ci_high) = bootstrap::bootstrap_mean_diff_ci(diffs, confidence, resamples);
    MetricOutput {
        effect,
        ci_low,
        ci_high,
        baseline_count: diffs.len() as u64,
        candidate_count: diffs.len() as u64,
        paired_count: diffs.len() as u64,
        timeouts,
        crashes,
        invalid,
        warning: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let records = vec![
            (1, rec("a", None, None, Some("candidate_win"), None, None)),
            (2, rec("b", None, None, Some("baseline_win"), None, None)),
            (3, rec("c", None, None, Some("draw"), None, None)),
        ];
        let out = compute(&records, MetricKind::WinRate, 0.95, 1000).unwrap();
        assert_eq!(out.paired_count, 2);
        assert_eq!(out.candidate_count, 1);
    }

    #[test]
    fn winrate_all_draws_is_zero_n_warning() {
        let records = vec![(1, rec("a", None, None, Some("draw"), None, None))];
        let out = compute(&records, MetricKind::WinRate, 0.95, 1000).unwrap();
        assert!(out.warning.is_some());
        assert_eq!(out.paired_count, 0);
    }

    #[test]
    fn mean_diff_paired_count_and_effect() {
        let records = vec![
            (1, rec("a", Some(1.0), Some(1.5), None, None, None)),
            (2, rec("b", Some(2.0), Some(2.5), None, None, None)),
        ];
        let out = compute(&records, MetricKind::MeanDiff, 0.95, 1000).unwrap();
        assert_eq!(out.paired_count, 2);
        assert!((out.effect - 0.5).abs() < 1e-9);
    }

    #[test]
    fn mean_diff_rejects_nan() {
        let records = vec![(1, rec("a", Some(f64::NAN), Some(1.0), None, None, None))];
        let result = compute(&records, MetricKind::MeanDiff, 0.95, 1000);
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
        let records = vec![
            (1, rec("dup", Some(1.0), Some(1.1), None, None, None)),
            (2, rec("dup", Some(2.0), Some(2.1), None, None, None)),
        ];
        let result = compute(&records, MetricKind::MeanDiff, 0.95, 1000);
        assert!(matches!(result, Err(VeridictError::DuplicateId { .. })));
    }

    #[test]
    fn status_only_records_count_but_are_not_schema_mismatch() {
        let records = vec![(1, rec("a", None, None, None, Some("ok"), Some("timeout")))];
        let out = compute(&records, MetricKind::WinRate, 0.95, 1000).unwrap();
        assert_eq!(out.timeouts, 1);
    }

    #[test]
    fn unusable_record_is_schema_mismatch() {
        let records = vec![(1, rec("a", None, None, None, None, None))];
        let result = compute(&records, MetricKind::WinRate, 0.95, 1000);
        assert!(matches!(result, Err(VeridictError::SchemaMismatch { .. })));
    }

    #[test]
    fn unrecognized_status_is_an_error() {
        let records = vec![(1, rec("a", None, None, None, Some("bogus"), None))];
        let result = compute(&records, MetricKind::WinRate, 0.95, 1000);
        assert!(matches!(
            result,
            Err(VeridictError::UnrecognizedStatus { .. })
        ));
    }

    #[test]
    fn unrecognized_outcome_is_an_error() {
        let records = vec![(1, rec("a", None, None, Some("bogus"), None, None))];
        let result = compute(&records, MetricKind::WinRate, 0.95, 1000);
        assert!(matches!(
            result,
            Err(VeridictError::UnrecognizedOutcome { .. })
        ));
    }

    #[test]
    fn empty_records_is_empty_input() {
        let result = compute(&[], MetricKind::WinRate, 0.95, 1000);
        assert!(matches!(result, Err(VeridictError::EmptyInput)));
    }
}
