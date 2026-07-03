//! Core statistical decision library. Domain-agnostic: consumes baseline vs
//! candidate trial data and returns a pass/fail/inconclusive verdict with
//! enough detail to explain why. No stdout/stderr side effects; that is the
//! CLI's job (see `main.rs`).

pub mod error;
pub mod input;
pub mod metrics;
pub mod report;
pub mod stats;
pub mod verdict;

pub use error::VeridictError;
pub use report::Report;

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
}

/// Runs the full pipeline: classify records, compute the metric's effect and
/// confidence interval, and apply the pass/fail thresholds. This is the only
/// function the CLI needs to call.
pub fn compare(
    records: &[(usize, input::Record)],
    metric: MetricKind,
    confidence: f64,
    thresholds: &verdict::Thresholds,
    resamples: usize,
) -> Result<Report, VeridictError> {
    let out = metrics::compute(records, metric, confidence, resamples)?;

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
        timeouts: out.timeouts,
        crashes: out.crashes,
        invalid: out.invalid,
        reason,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::Record;
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
        let report = compare(&records, MetricKind::WinRate, 0.95, &thresholds, 2000).unwrap();
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
        let report = compare(&records, MetricKind::MeanDiff, 0.95, &thresholds, 2000).unwrap();
        assert_eq!(report.verdict, Verdict::Inconclusive);
    }

    #[test]
    fn zero_usable_trials_is_inconclusive_not_error() {
        let records = vec![(1, rec("a", None, None, None, Some("timeout"), None))];
        let thresholds = Thresholds::symmetric(0.02).unwrap();
        let report = compare(&records, MetricKind::WinRate, 0.95, &thresholds, 2000).unwrap();
        assert_eq!(report.verdict, Verdict::Inconclusive);
        assert_eq!(report.timeouts, 1);
    }

    #[test]
    fn empty_input_is_an_error() {
        let records: Vec<(usize, Record)> = Vec::new();
        let thresholds = Thresholds::symmetric(0.02).unwrap();
        let result = compare(&records, MetricKind::WinRate, 0.95, &thresholds, 2000);
        assert!(matches!(result, Err(VeridictError::EmptyInput)));
    }
}
