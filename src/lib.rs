//! Core statistical decision library. Domain-agnostic: consumes baseline vs
//! candidate trial data and returns a pass/fail/inconclusive verdict with
//! enough detail to explain why. No stdout/stderr side effects; that is the
//! CLI's job (see `main.rs`).

pub mod error;
pub mod input;
pub mod metrics;
pub mod report;
pub mod sprt;
pub mod stats;
pub mod verdict;

pub use error::VeridictError;
pub use report::{MultiReport, Report};

use serde::Serialize;

/// Final decision returned for a comparison run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Pass,
    Fail,
    Inconclusive,
}

/// Health of a single trial's execution, independent of any score it produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrialStatus {
    Ok,
    Timeout,
    Crash,
    Invalid,
}

impl TrialStatus {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "ok" => Some(Self::Ok),
            "timeout" => Some(Self::Timeout),
            "crash" => Some(Self::Crash),
            "invalid" => Some(Self::Invalid),
            _ => None,
        }
    }
}

/// Result of a single win/loss/draw comparison.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    BaselineWin,
    CandidateWin,
    Draw,
}

impl Outcome {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "baseline_win" => Some(Self::BaselineWin),
            "candidate_win" => Some(Self::CandidateWin),
            "draw" => Some(Self::Draw),
            _ => None,
        }
    }
}

/// Which statistical method computed the effect size and confidence interval.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum MetricKind {
    #[serde(rename = "winrate")]
    WinRate,
    #[serde(rename = "mean-diff")]
    MeanDiff,
    #[serde(rename = "sign-test")]
    SignTest,
    #[serde(rename = "elo")]
    Elo,
}

/// Runs one metric end to end: classify records, compute its effect and
/// confidence interval, and apply the pass/fail thresholds.
pub fn compare_one(
    records: &[(usize, input::Record)],
    metric: MetricKind,
    confidence: f64,
    thresholds: &verdict::Thresholds,
    resamples: usize,
    seed: u64,
) -> Result<Report, VeridictError> {
    let out = metrics::compute(records, metric, confidence, resamples, seed)?;

    // Zero usable trials means "no signal", not "the CLI ran a threshold
    // check on a fabricated zero": force Inconclusive rather than letting
    // (0.0, 0.0) accidentally satisfy a threshold that includes zero.
    let (verdict, reason) = match &out.warning {
        Some(warning) => (Verdict::Inconclusive, warning.clone()),
        None => verdict::decide(out.ci_low, out.ci_high, thresholds),
    };

    Ok(Report {
        verdict,
        metric,
        baseline_count: out.baseline_count,
        candidate_count: out.candidate_count,
        paired_count: out.paired_count,
        effect: out.effect,
        confidence,
        ci_low: out.ci_low,
        ci_high: out.ci_high,
        pass_above: thresholds.pass_above,
        fail_below: thresholds.fail_below,
        timeouts: out.timeouts,
        crashes: out.crashes,
        invalid: out.invalid,
        failure_breakdown: out.failures,
        reason,
    })
}

/// Runs several metrics against the same records and thresholds, and
/// combines them into one overall verdict: `Fail` if any metric fails,
/// else `Inconclusive` if any metric is inconclusive, else `Pass`. Matches
/// the "a false pass is worse than an inconclusive result" rule: one
/// metric failing sinks the whole run.
pub fn compare_many(
    records: &[(usize, input::Record)],
    metrics: &[MetricKind],
    confidence: f64,
    thresholds: &verdict::Thresholds,
    resamples: usize,
    seed: u64,
) -> Result<MultiReport, VeridictError> {
    let reports = metrics
        .iter()
        .map(|&metric| compare_one(records, metric, confidence, thresholds, resamples, seed))
        .collect::<Result<Vec<_>, _>>()?;
    let verdict = verdict::aggregate(reports.iter().map(|r| r.verdict));
    Ok(MultiReport { verdict, reports })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::Record;
    use crate::stats::bootstrap::DEFAULT_SEED;
    use crate::verdict::Thresholds;

    fn rec(
        id: &str,
        baseline: Option<f64>,
        candidate: Option<f64>,
        result: Option<&str>,
        baseline_status: Option<&str>,
        candidate_status: Option<&str>,
    ) -> Record {
        Record {
            id: Some(id.to_string()),
            baseline,
            candidate,
            result: result.map(str::to_string),
            baseline_status: baseline_status.map(str::to_string),
            candidate_status: candidate_status.map(str::to_string),
        }
    }

    #[test]
    fn end_to_end_winrate_pass() {
        let mut records = Vec::new();
        for i in 0..80 {
            records.push((
                i + 1,
                rec(
                    &format!("c{i}"),
                    None,
                    None,
                    Some("candidate_win"),
                    None,
                    None,
                ),
            ));
        }
        for i in 0..20 {
            records.push((
                80 + i + 1,
                rec(
                    &format!("b{i}"),
                    None,
                    None,
                    Some("baseline_win"),
                    None,
                    None,
                ),
            ));
        }
        let thresholds = Thresholds::symmetric(0.02).unwrap();
        let report = compare_one(
            &records,
            MetricKind::WinRate,
            0.95,
            &thresholds,
            2000,
            DEFAULT_SEED,
        )
        .unwrap();
        assert_eq!(report.verdict, Verdict::Pass);
        assert_eq!(report.candidate_count, 80);
        assert_eq!(report.baseline_count, 20);
    }

    #[test]
    fn end_to_end_mean_diff_inconclusive_on_tiny_sample() {
        let records = vec![
            (1, rec("a", Some(1.0), Some(1.1), None, None, None)),
            (2, rec("b", Some(2.0), Some(1.9), None, None, None)),
        ];
        let thresholds = Thresholds::symmetric(0.02).unwrap();
        let report = compare_one(
            &records,
            MetricKind::MeanDiff,
            0.95,
            &thresholds,
            2000,
            DEFAULT_SEED,
        )
        .unwrap();
        assert_eq!(report.verdict, Verdict::Inconclusive);
    }

    #[test]
    fn zero_usable_trials_is_inconclusive_not_error() {
        let records = vec![(1, rec("a", None, None, None, Some("timeout"), None))];
        let thresholds = Thresholds::symmetric(0.02).unwrap();
        let report = compare_one(
            &records,
            MetricKind::WinRate,
            0.95,
            &thresholds,
            2000,
            DEFAULT_SEED,
        )
        .unwrap();
        assert_eq!(report.verdict, Verdict::Inconclusive);
        assert_eq!(report.timeouts, 1);
    }

    #[test]
    fn empty_input_is_an_error() {
        let records: Vec<(usize, Record)> = Vec::new();
        let thresholds = Thresholds::symmetric(0.02).unwrap();
        let result = compare_one(
            &records,
            MetricKind::WinRate,
            0.95,
            &thresholds,
            2000,
            DEFAULT_SEED,
        );
        assert!(matches!(result, Err(VeridictError::EmptyInput)));
    }

    #[test]
    fn compare_many_passes_overall_when_every_metric_passes() {
        let records: Vec<_> = (0..20)
            .map(|i| {
                (
                    i + 1,
                    rec(
                        &format!("r{i}"),
                        Some(1.0),
                        Some(2.0),
                        Some("candidate_win"),
                        None,
                        None,
                    ),
                )
            })
            .collect();
        let thresholds = Thresholds::symmetric(0.1).unwrap();
        let report = compare_many(
            &records,
            &[MetricKind::WinRate, MetricKind::MeanDiff],
            0.95,
            &thresholds,
            2000,
            DEFAULT_SEED,
        )
        .unwrap();
        assert_eq!(report.verdict, Verdict::Pass);
        assert_eq!(report.reports.len(), 2);
    }

    #[test]
    fn compare_many_fails_overall_if_any_metric_fails() {
        // Each record carries both fields: result says the candidate always
        // loses (winrate -> Fail), but the numeric score always favors the
        // candidate (mean-diff -> Pass). Fail must dominate the aggregate.
        let records: Vec<_> = (0..20)
            .map(|i| {
                (
                    i + 1,
                    rec(
                        &format!("r{i}"),
                        Some(1.0),
                        Some(2.0),
                        Some("baseline_win"),
                        None,
                        None,
                    ),
                )
            })
            .collect();
        let thresholds = Thresholds::symmetric(0.1).unwrap();
        let report = compare_many(
            &records,
            &[MetricKind::WinRate, MetricKind::MeanDiff],
            0.95,
            &thresholds,
            2000,
            DEFAULT_SEED,
        )
        .unwrap();
        assert_eq!(report.reports[0].verdict, Verdict::Fail);
        assert_eq!(report.reports[1].verdict, Verdict::Pass);
        assert_eq!(report.verdict, Verdict::Fail);
    }
}
