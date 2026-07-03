//! The machine-readable verdict report. Field set matches AGENTS.md's
//! "Minimum report fields", plus `reason` (justified by the "every verdict
//! must include enough information to understand why it happened" rule).

use serde::Serialize;

use crate::{MetricKind, Verdict};

#[derive(Debug, Serialize)]
pub struct Report {
    pub verdict: Verdict,
    pub metric: MetricKind,
    pub baseline_count: u64,
    pub candidate_count: u64,
    pub paired_count: u64,
    pub effect: f64,
    pub confidence: f64,
    pub ci_low: f64,
    pub ci_high: f64,
    pub timeouts: u64,
    pub crashes: u64,
    pub invalid: u64,
    pub reason: String,
}

impl Report {
    pub fn to_json_pretty(&self) -> String {
        serde_json::to_string_pretty(self)
            .expect("Report contains only finite fields and strings; serialization cannot fail")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_metric_and_verdict_as_lowercase() {
        let report = Report {
            verdict: Verdict::Pass,
            metric: MetricKind::WinRate,
            baseline_count: 20,
            candidate_count: 80,
            paired_count: 100,
            effect: 0.06,
            confidence: 0.95,
            ci_low: 0.02,
            ci_high: 0.10,
            timeouts: 1,
            crashes: 0,
            invalid: 0,
            reason: "ok".to_string(),
        };
        let json = report.to_json_pretty();
        assert!(json.contains("\"verdict\": \"pass\""));
        assert!(json.contains("\"metric\": \"winrate\""));
    }
}
