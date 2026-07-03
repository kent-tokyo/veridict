//! Turns raw records into a metric's effect size and confidence interval.
//!
//! A record is "used" if it contributes to either the status tally
//! (timeout/crash/invalid) or the chosen metric's calculation. A record that
//! matches neither is rejected as `SchemaMismatch` rather than silently
//! dropped, per AGENTS.md's "never silently ignore invalid data" rule.

use std::collections::{HashMap, HashSet};

use serde::Serialize;

use crate::error::VeridictError;
use crate::input::Record;
use crate::stats::{bootstrap, elo, wilson};
use crate::{MetricKind, Outcome, TrialStatus};

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

/// `paired_by_id`: when true, two records sharing the same `id` are treated
/// as one testcase played twice (e.g. roles swapped to cancel the
/// testcase's own bias) and combined into a single net observation instead
/// of two independent ones - see [`reduce_outcome_pairs`]/
/// [`reduce_diff_pairs`] for exactly how. An `id` seen only once is an
/// ordinary unpaired sample; three or more is a data error, not a pair.
/// When false (the default), every record is its own independent sample,
/// same as before this flag existed.
pub fn compute(
    records: &[(usize, Record)],
    metric: MetricKind,
    confidence: f64,
    resamples: usize,
    seed: u64,
    paired_by_id: bool,
) -> Result<MetricOutput, VeridictError> {
    if records.is_empty() {
        return Err(VeridictError::EmptyInput);
    }
    if !confidence.is_finite() || confidence <= 0.0 || confidence >= 1.0 {
        return Err(VeridictError::InvalidConfidence(confidence));
    }

    let mut failures = FailureBreakdown::default();

    // Shared by WinRate and Elo: both need a win/loss/draw observation per
    // record; they only differ in how the collected observations are
    // turned into an effect size below.
    let mut outcomes: Vec<(usize, Option<&str>, Outcome)> = Vec::new();
    // Shared by MeanDiff and SignTest: both need a paired (baseline,
    // candidate) numeric observation per record.
    let mut numeric: Vec<(usize, Option<&str>, f64)> = Vec::new();
    let mut seen_ids: HashSet<&str> = HashSet::new();

    for (line, record) in records {
        let mut used = false;

        if let Some(status) = record.baseline_status.as_deref() {
            used = true;
            tally_status(status, *line, "baseline_status", &mut failures.baseline)?;
        }
        if let Some(status) = record.candidate_status.as_deref() {
            used = true;
            tally_status(status, *line, "candidate_status", &mut failures.candidate)?;
        }

        match metric {
            MetricKind::WinRate | MetricKind::Elo => {
                if let Some(result) = record.result.as_deref() {
                    used = true;
                    match Outcome::parse(result) {
                        Some(outcome) => outcomes.push((*line, record.id.as_deref(), outcome)),
                        None => {
                            return Err(VeridictError::UnrecognizedOutcome {
                                line: *line,
                                value: result.to_string(),
                            });
                        }
                    }
                }
            }
            MetricKind::MeanDiff | MetricKind::SignTest => {
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
                    // Without pairing, a repeated id is almost always a data
                    // mistake, so it's rejected up front. With pairing, a
                    // repeated id is the whole point - reduce_diff_pairs
                    // validates it (exactly 2, not more) instead.
                    if !paired_by_id
                        && let Some(id) = record.id.as_deref()
                        && !seen_ids.insert(id)
                    {
                        return Err(VeridictError::DuplicateId {
                            id: id.to_string(),
                            line: *line,
                        });
                    }
                    used = true;
                    numeric.push((*line, record.id.as_deref(), c - b));
                }
            }
        }

        if !used {
            return Err(VeridictError::SchemaMismatch {
                line: *line,
                context: metric_label(metric),
                detail: "record has no fields usable by this metric and no status fields"
                    .to_string(),
            });
        }
    }

    let timeouts = failures.baseline.timeout + failures.candidate.timeout;
    let crashes = failures.baseline.crash + failures.candidate.crash;
    let invalid = failures.baseline.invalid + failures.candidate.invalid;

    let (baseline_wins, candidate_wins, draws) = if paired_by_id {
        reduce_outcome_pairs(&outcomes)?
    } else {
        tally_outcomes(&outcomes)
    };
    let diffs = if paired_by_id {
        reduce_diff_pairs(&numeric)?
    } else {
        numeric.iter().map(|(_, _, d)| *d).collect()
    };

    match metric {
        MetricKind::WinRate => compute_winrate(
            baseline_wins,
            candidate_wins,
            confidence,
            timeouts,
            crashes,
            invalid,
            failures,
        ),
        MetricKind::Elo => compute_elo(
            baseline_wins,
            candidate_wins,
            draws,
            confidence,
            timeouts,
            crashes,
            invalid,
            failures,
        ),
        MetricKind::MeanDiff => Ok(compute_mean_diff(
            &diffs, confidence, resamples, seed, timeouts, crashes, invalid, failures,
        )),
        MetricKind::SignTest => {
            compute_sign_test(&diffs, confidence, timeouts, crashes, invalid, failures)
        }
    }
}

fn tally_outcomes(outcomes: &[(usize, Option<&str>, Outcome)]) -> (u64, u64, u64) {
    let (mut baseline_wins, mut candidate_wins, mut draws) = (0u64, 0u64, 0u64);
    for (_, _, outcome) in outcomes {
        match outcome {
            Outcome::BaselineWin => baseline_wins += 1,
            Outcome::CandidateWin => candidate_wins += 1,
            Outcome::Draw => draws += 1,
        }
    }
    (baseline_wins, candidate_wins, draws)
}

/// Combines each pair of same-id outcomes by total points scored (win=1,
/// draw=0.5, loss=0, from the candidate's perspective): >1 point across the
/// pair is a net candidate win, <1 a net baseline win, exactly 1 a net draw,
/// the standard "paired game" scoring convention. An id-less outcome is
/// always its own singleton (there's nothing to match it to).
pub(crate) fn reduce_outcome_pairs(
    outcomes: &[(usize, Option<&str>, Outcome)],
) -> Result<(u64, u64, u64), VeridictError> {
    let mut groups: HashMap<&str, Vec<(usize, Outcome)>> = HashMap::new();
    let (mut baseline_wins, mut candidate_wins, mut draws) = (0u64, 0u64, 0u64);

    let mut tally = |outcome: Outcome| match outcome {
        Outcome::BaselineWin => baseline_wins += 1,
        Outcome::CandidateWin => candidate_wins += 1,
        Outcome::Draw => draws += 1,
    };

    for (line, id, outcome) in outcomes {
        match id {
            Some(id) => groups.entry(id).or_default().push((*line, *outcome)),
            None => tally(*outcome),
        }
    }

    for (id, group) in groups {
        match group.as_slice() {
            [(_, outcome)] => tally(*outcome),
            [(_, a), (_, b)] => {
                let points = |o: &Outcome| match o {
                    Outcome::CandidateWin => 1.0,
                    Outcome::Draw => 0.5,
                    Outcome::BaselineWin => 0.0,
                };
                let total = points(a) + points(b);
                #[allow(clippy::float_cmp)]
                tally(if total > 1.0 {
                    Outcome::CandidateWin
                } else if total < 1.0 {
                    Outcome::BaselineWin
                } else {
                    Outcome::Draw
                });
            }
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

    Ok((baseline_wins, candidate_wins, draws))
}

/// Combines each pair of same-id diffs by averaging them, so a testcase
/// played twice contributes one net-of-bias sample instead of two raw
/// ones. An id-less diff is always its own singleton.
pub(crate) fn reduce_diff_pairs(
    numeric: &[(usize, Option<&str>, f64)],
) -> Result<Vec<f64>, VeridictError> {
    let mut groups: HashMap<&str, Vec<(usize, f64)>> = HashMap::new();
    let mut diffs: Vec<f64> = Vec::new();

    for (line, id, diff) in numeric {
        match id {
            Some(id) => groups.entry(id).or_default().push((*line, *diff)),
            None => diffs.push(*diff),
        }
    }

    for (id, group) in groups {
        match group.as_slice() {
            [(_, d)] => diffs.push(*d),
            [(_, a), (_, b)] => diffs.push((a + b) / 2.0),
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

    Ok(diffs)
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

/// Win rate is computed over decisive (non-draw) trials only: `p_hat =
/// candidate_wins / (candidate_wins + baseline_wins)`. Draws are counted
/// among "used" records but excluded from the proportion, matching standard
/// paired-match testing practice (a draw is neither a win nor a loss).
/// `effect`/`ci_low`/`ci_high` are centered on 0 (deviation from a 50/50
/// split) so they compose with `--min-effect`/`--pass-above`/`--fail-below`,
/// which are expressed as signed deltas.
#[allow(clippy::too_many_arguments)]
fn compute_winrate(
    baseline_wins: u64,
    candidate_wins: u64,
    confidence: f64,
    timeouts: u64,
    crashes: u64,
    invalid: u64,
    failures: FailureBreakdown,
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
            failures,
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
        failures,
        warning: None,
    })
}

/// Elo difference from win/loss/draw counts: `score = (candidate_wins + 0.5
/// * draws) / n`, converted via the standard logistic Elo model. The score
/// rate's Wilson CI treats each trial as a plain Bernoulli draw, which
/// overstates variance versus the true trinomial distribution (a draw's
/// half-point carries less variance than a coin flip) - a deliberately
/// conservative (too-wide, never too-narrow) approximation, same tradeoff
/// already accepted for `sign-test`.
#[allow(clippy::too_many_arguments)]
fn compute_elo(
    baseline_wins: u64,
    candidate_wins: u64,
    draws: u64,
    confidence: f64,
    timeouts: u64,
    crashes: u64,
    invalid: u64,
    failures: FailureBreakdown,
) -> Result<MetricOutput, VeridictError> {
    let n = baseline_wins + candidate_wins + draws;
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
            failures,
            warning: Some("no trials to compute an Elo difference".to_string()),
        });
    }
    let score = (candidate_wins as f64 + 0.5 * draws as f64) / n as f64;
    let (lo, hi) = wilson::wilson_ci_from_proportion(score, n as f64, confidence)?;
    Ok(MetricOutput {
        effect: elo::elo_from_score(score),
        ci_low: elo::elo_from_score(lo),
        ci_high: elo::elo_from_score(hi),
        baseline_count: baseline_wins,
        candidate_count: candidate_wins,
        paired_count: n,
        timeouts,
        crashes,
        invalid,
        failures,
        warning: None,
    })
}

#[allow(clippy::too_many_arguments)]
fn compute_mean_diff(
    diffs: &[f64],
    confidence: f64,
    resamples: usize,
    seed: u64,
    timeouts: u64,
    crashes: u64,
    invalid: u64,
    failures: FailureBreakdown,
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
            failures,
            warning: Some("no paired numeric trials to compute mean difference".to_string()),
        };
    }
    let effect = bootstrap::mean(diffs);
    let (ci_low, ci_high) = bootstrap::bootstrap_mean_diff_ci(diffs, confidence, resamples, seed);
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
        failures,
        warning: None,
    }
}

/// Nonparametric alternative to `mean-diff`: only the *direction* of each
/// pair's difference matters, not its magnitude. Ties (`candidate ==
/// baseline`) are excluded from `n`, same treatment as draws in `winrate`.
/// The proportion of positive signs is run through the same Wilson CI as
/// `winrate` and centered the same way, since "is the sign test's underlying
/// proportion above 50%, with what confidence" is exactly a binomial
/// proportion question.
fn compute_sign_test(
    diffs: &[f64],
    confidence: f64,
    timeouts: u64,
    crashes: u64,
    invalid: u64,
    failures: FailureBreakdown,
) -> Result<MetricOutput, VeridictError> {
    let positive = diffs.iter().filter(|d| **d > 0.0).count() as u64;
    let negative = diffs.iter().filter(|d| **d < 0.0).count() as u64;
    let n = positive + negative;
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
            failures,
            warning: Some("no non-tied paired trials to compute the sign test".to_string()),
        });
    }
    let (lo, hi) = wilson::wilson_ci(positive, n, confidence)?;
    let p_hat = positive as f64 / n as f64;
    Ok(MetricOutput {
        effect: p_hat - 0.5,
        ci_low: lo - 0.5,
        ci_high: hi - 0.5,
        baseline_count: negative,
        candidate_count: positive,
        paired_count: n,
        timeouts,
        crashes,
        invalid,
        failures,
        warning: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SEED: u64 = bootstrap::DEFAULT_SEED;

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
        let out = compute(&records, MetricKind::WinRate, 0.95, 1000, SEED, false).unwrap();
        assert_eq!(out.paired_count, 2);
        assert_eq!(out.candidate_count, 1);
    }

    #[test]
    fn winrate_all_draws_is_zero_n_warning() {
        let records = vec![(1, rec("a", None, None, Some("draw"), None, None))];
        let out = compute(&records, MetricKind::WinRate, 0.95, 1000, SEED, false).unwrap();
        assert!(out.warning.is_some());
        assert_eq!(out.paired_count, 0);
    }

    #[test]
    fn mean_diff_paired_count_and_effect() {
        let records = vec![
            (1, rec("a", Some(1.0), Some(1.5), None, None, None)),
            (2, rec("b", Some(2.0), Some(2.5), None, None, None)),
        ];
        let out = compute(&records, MetricKind::MeanDiff, 0.95, 1000, SEED, false).unwrap();
        assert_eq!(out.paired_count, 2);
        assert!((out.effect - 0.5).abs() < 1e-9);
    }

    #[test]
    fn mean_diff_rejects_nan() {
        let records = vec![(1, rec("a", Some(f64::NAN), Some(1.0), None, None, None))];
        let result = compute(&records, MetricKind::MeanDiff, 0.95, 1000, SEED, false);
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
        let result = compute(&records, MetricKind::MeanDiff, 0.95, 1000, SEED, false);
        assert!(matches!(result, Err(VeridictError::DuplicateId { .. })));
    }

    #[test]
    fn status_only_records_count_but_are_not_schema_mismatch() {
        let records = vec![(1, rec("a", None, None, None, Some("ok"), Some("timeout")))];
        let out = compute(&records, MetricKind::WinRate, 0.95, 1000, SEED, false).unwrap();
        assert_eq!(out.timeouts, 1);
        assert_eq!(out.failures.candidate.timeout, 1);
        assert_eq!(out.failures.baseline.timeout, 0);
    }

    #[test]
    fn unusable_record_is_schema_mismatch() {
        let records = vec![(1, rec("a", None, None, None, None, None))];
        let result = compute(&records, MetricKind::WinRate, 0.95, 1000, SEED, false);
        assert!(matches!(result, Err(VeridictError::SchemaMismatch { .. })));
    }

    #[test]
    fn unrecognized_status_is_an_error() {
        let records = vec![(1, rec("a", None, None, None, Some("bogus"), None))];
        let result = compute(&records, MetricKind::WinRate, 0.95, 1000, SEED, false);
        assert!(matches!(
            result,
            Err(VeridictError::UnrecognizedStatus { .. })
        ));
    }

    #[test]
    fn unrecognized_outcome_is_an_error() {
        let records = vec![(1, rec("a", None, None, Some("bogus"), None, None))];
        let result = compute(&records, MetricKind::WinRate, 0.95, 1000, SEED, false);
        assert!(matches!(
            result,
            Err(VeridictError::UnrecognizedOutcome { .. })
        ));
    }

    #[test]
    fn empty_records_is_empty_input() {
        let result = compute(&[], MetricKind::WinRate, 0.95, 1000, SEED, false);
        assert!(matches!(result, Err(VeridictError::EmptyInput)));
    }

    #[test]
    fn sign_test_excludes_ties() {
        let records = vec![
            (1, rec("a", Some(1.0), Some(1.5), None, None, None)), // positive
            (2, rec("b", Some(2.0), Some(1.0), None, None, None)), // negative
            (3, rec("c", Some(3.0), Some(3.0), None, None, None)), // tie, excluded
        ];
        let out = compute(&records, MetricKind::SignTest, 0.95, 1000, SEED, false).unwrap();
        assert_eq!(out.paired_count, 2);
        assert_eq!(out.candidate_count, 1);
        assert_eq!(out.baseline_count, 1);
    }

    #[test]
    fn sign_test_all_ties_is_zero_n_warning() {
        let records = vec![(1, rec("a", Some(1.0), Some(1.0), None, None, None))];
        let out = compute(&records, MetricKind::SignTest, 0.95, 1000, SEED, false).unwrap();
        assert!(out.warning.is_some());
    }

    #[test]
    fn elo_counts_draws_as_half_a_point() {
        let records = vec![
            (1, rec("a", None, None, Some("candidate_win"), None, None)),
            (2, rec("b", None, None, Some("draw"), None, None)),
        ];
        let out = compute(&records, MetricKind::Elo, 0.95, 1000, SEED, false).unwrap();
        // score = (1 + 0.5) / 2 = 0.75 -> a positive Elo effect.
        assert!(out.effect > 0.0);
        assert_eq!(out.paired_count, 2);
    }

    #[test]
    fn elo_even_record_is_zero_effect() {
        let records = vec![
            (1, rec("a", None, None, Some("candidate_win"), None, None)),
            (2, rec("b", None, None, Some("baseline_win"), None, None)),
        ];
        let out = compute(&records, MetricKind::Elo, 0.95, 1000, SEED, false).unwrap();
        assert!(out.effect.abs() < 1e-9);
    }

    #[test]
    fn elo_zero_trials_is_a_warning_not_an_error() {
        let records = vec![(1, rec("a", None, None, None, Some("timeout"), None))];
        let out = compute(&records, MetricKind::Elo, 0.95, 1000, SEED, false).unwrap();
        assert!(out.warning.is_some());
    }

    // --- paired_by_id ---

    #[test]
    fn paired_winrate_nets_two_games_per_id_by_points() {
        let records = vec![
            // id "op1": candidate wins one, loses the other -> net draw (1.0 pt).
            (1, rec("op1", None, None, Some("candidate_win"), None, None)),
            (2, rec("op1", None, None, Some("baseline_win"), None, None)),
            // id "op2": candidate wins both -> net candidate win (2.0 pts).
            (3, rec("op2", None, None, Some("candidate_win"), None, None)),
            (4, rec("op2", None, None, Some("candidate_win"), None, None)),
        ];
        let out = compute(&records, MetricKind::WinRate, 0.95, 1000, SEED, true).unwrap();
        // 4 raw games -> 2 paired samples: 1 draw (excluded from n), 1 candidate win.
        assert_eq!(out.paired_count, 1);
        assert_eq!(out.candidate_count, 1);
        assert_eq!(out.baseline_count, 0);
    }

    #[test]
    fn paired_winrate_unpaired_singleton_still_counts() {
        let records = vec![(
            1,
            rec("solo", None, None, Some("candidate_win"), None, None),
        )];
        let out = compute(&records, MetricKind::WinRate, 0.95, 1000, SEED, true).unwrap();
        assert_eq!(out.paired_count, 1);
        assert_eq!(out.candidate_count, 1);
    }

    #[test]
    fn paired_winrate_rejects_triple_id() {
        let records = vec![
            (1, rec("op1", None, None, Some("candidate_win"), None, None)),
            (2, rec("op1", None, None, Some("candidate_win"), None, None)),
            (3, rec("op1", None, None, Some("candidate_win"), None, None)),
        ];
        let result = compute(&records, MetricKind::WinRate, 0.95, 1000, SEED, true);
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
        let records = vec![
            (1, rec("op1", Some(1.0), Some(1.2), None, None, None)), // diff +0.2
            (2, rec("op1", Some(1.0), Some(0.8), None, None, None)), // diff -0.2
        ];
        let out = compute(&records, MetricKind::MeanDiff, 0.95, 1000, SEED, true).unwrap();
        assert_eq!(out.paired_count, 1);
        assert!(out.effect.abs() < 1e-9); // net-of-bias effect is ~0, not the two raw +-0.2 diffs
    }

    #[test]
    fn paired_mean_diff_allows_duplicate_id_that_unpaired_mode_rejects() {
        let records = vec![
            (1, rec("dup", Some(1.0), Some(1.1), None, None, None)),
            (2, rec("dup", Some(2.0), Some(2.1), None, None, None)),
        ];
        let result = compute(&records, MetricKind::MeanDiff, 0.95, 1000, SEED, true);
        assert!(result.is_ok());
    }

    #[test]
    fn paired_sign_test_rejects_triple_id() {
        let records = vec![
            (1, rec("op1", Some(1.0), Some(1.1), None, None, None)),
            (2, rec("op1", Some(1.0), Some(1.1), None, None, None)),
            (3, rec("op1", Some(1.0), Some(1.1), None, None, None)),
        ];
        let result = compute(&records, MetricKind::SignTest, 0.95, 1000, SEED, true);
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
        let records = vec![
            (1, rec("op1", None, None, Some("candidate_win"), None, None)),
            (2, rec("op1", None, None, Some("draw"), None, None)),
        ];
        // total = 1.0 + 0.5 = 1.5 pts across the pair -> net candidate win.
        let out = compute(&records, MetricKind::Elo, 0.95, 1000, SEED, true).unwrap();
        assert_eq!(out.paired_count, 1);
        assert!(out.effect > 0.0);
    }
}
