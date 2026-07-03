//! The machine-readable verdict report, plus a Markdown rendering for
//! humans. JSON field set matches AGENTS.md's "Minimum report fields", plus
//! `reason`, `pass_above`/`fail_below`, and `failure_breakdown` (all
//! justified by AGENTS.md's own "every verdict must include enough
//! information to understand why" and "failure classification summary"
//! requirements).

use serde::Serialize;

use crate::metrics::FailureBreakdown;
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
    pub pass_above: f64,
    pub fail_below: f64,
    pub timeouts: u64,
    pub crashes: u64,
    pub invalid: u64,
    pub failure_breakdown: FailureBreakdown,
    pub reason: String,
}

/// A metric's effect/CI/thresholds are proportions (winrate, sign-test) or
/// raw units (mean-diff); proportions read better as percentage points.
fn is_proportion(metric: MetricKind) -> bool {
    matches!(metric, MetricKind::WinRate | MetricKind::SignTest)
}

fn fmt_effect(metric: MetricKind, value: f64) -> String {
    if is_proportion(metric) {
        format!("{:+.1} pp", value * 100.0)
    } else {
        format!("{value:+.4}")
    }
}

/// Renders any `Serialize`-derived enum via its own serde representation,
/// so Markdown output can never drift from the JSON field it mirrors.
fn serde_str<T: Serialize>(value: &T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

impl Report {
    pub fn to_json_pretty(&self) -> String {
        serde_json::to_string_pretty(self)
            .expect("Report contains only finite fields and strings; serialization cannot fail")
    }

    /// Full standalone report: AGENTS.md's `# Veridict Report` header plus
    /// this metric's body. What `--report-md` writes for a single-metric run.
    pub fn to_markdown(&self) -> String {
        format!("# Veridict Report\n\n{}", self.to_markdown_body())
    }

    /// Body only (no top-level header), for embedding under a
    /// `MultiReport`'s single shared header.
    fn to_markdown_body(&self) -> String {
        let b = &self.failure_breakdown.baseline;
        let c = &self.failure_breakdown.candidate;
        format!(
            "Verdict: {verdict}\n\n\
             Metric: {metric}\n\
             Effect: {effect}\n\
             {confidence_pct}% CI: {ci_low} to {ci_high}\n\
             Threshold for pass: {pass_above} / Threshold for fail: {fail_below}\n\n\
             {reason}\n\n\
             Samples: baseline={baseline_count}, candidate={candidate_count}, paired={paired_count}\n\n\
             Status counts:\n\
             - timeout: {timeouts} (baseline={b_timeout}, candidate={c_timeout})\n\
             - crash: {crashes} (baseline={b_crash}, candidate={c_crash})\n\
             - invalid: {invalid} (baseline={b_invalid}, candidate={c_invalid})\n",
            verdict = serde_str(&self.verdict),
            metric = serde_str(&self.metric),
            effect = fmt_effect(self.metric, self.effect),
            confidence_pct = self.confidence * 100.0,
            ci_low = fmt_effect(self.metric, self.ci_low),
            ci_high = fmt_effect(self.metric, self.ci_high),
            pass_above = fmt_effect(self.metric, self.pass_above),
            fail_below = fmt_effect(self.metric, self.fail_below),
            reason = self.reason,
            baseline_count = self.baseline_count,
            candidate_count = self.candidate_count,
            paired_count = self.paired_count,
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

/// Output of `compare_many`: one overall verdict plus each metric's own
/// report. Only produced for multi-metric runs; a single `--metric` run
/// keeps printing a plain `Report` so its JSON shape never changes.
#[derive(Debug, Serialize)]
pub struct MultiReport {
    pub verdict: Verdict,
    pub reports: Vec<Report>,
}

impl MultiReport {
    pub fn to_json_pretty(&self) -> String {
        serde_json::to_string_pretty(self).expect(
            "MultiReport contains only finite fields and strings; serialization cannot fail",
        )
    }

    pub fn to_markdown(&self) -> String {
        let mut out = format!(
            "# Veridict Report\n\nOverall verdict: {}\n",
            serde_str(&self.verdict)
        );
        for report in &self.reports {
            out.push_str("\n---\n\n");
            out.push_str(&report.to_markdown_body());
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_report() -> Report {
        Report {
            verdict: Verdict::Pass,
            metric: MetricKind::WinRate,
            baseline_count: 20,
            candidate_count: 80,
            paired_count: 100,
            effect: 0.06,
            confidence: 0.95,
            ci_low: 0.02,
            ci_high: 0.10,
            pass_above: 0.02,
            fail_below: -0.02,
            timeouts: 1,
            crashes: 0,
            invalid: 0,
            failure_breakdown: FailureBreakdown::default(),
            reason: "ok".to_string(),
        }
    }

    #[test]
    fn serializes_metric_and_verdict_as_lowercase() {
        let json = sample_report().to_json_pretty();
        assert!(json.contains("\"verdict\": \"pass\""));
        assert!(json.contains("\"metric\": \"winrate\""));
    }

    #[test]
    fn markdown_reports_effect_in_percentage_points_for_winrate() {
        let md = sample_report().to_markdown();
        assert!(md.contains("Verdict: pass"));
        assert!(md.contains("+6.0 pp"));
    }

    #[test]
    fn markdown_reports_effect_in_raw_units_for_mean_diff() {
        let mut report = sample_report();
        report.metric = MetricKind::MeanDiff;
        report.effect = 0.032;
        let md = report.to_markdown();
        assert!(md.contains("+0.0320"));
    }

    #[test]
    fn multi_report_wraps_each_metric_section() {
        let multi = MultiReport {
            verdict: Verdict::Fail,
            reports: vec![sample_report()],
        };
        let md = multi.to_markdown();
        assert!(md.contains("Overall verdict: fail"));
        assert!(md.contains("Verdict: pass"));
        let json = multi.to_json_pretty();
        assert!(json.contains("\"reports\""));
    }
}
