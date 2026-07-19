//! The machine-readable verdict report, plus a Markdown rendering for
//! humans. JSON field set matches AGENTS.md's "Minimum report fields", plus
//! `reason`, `pass_above`/`fail_below`, and `failure_breakdown` (all
//! justified by AGENTS.md's own "every verdict must include enough
//! information to understand why" and "failure classification summary"
//! requirements).

use serde::Serialize;

use crate::metrics::FailureBreakdown;
use crate::{MetricKind, Promotion, Validity, Verdict};

/// Current JSON report schema version, for `Report`/`MultiReport`/
/// `SprtReport`/`ComparisonMatrix` alike. Every change so far (including
/// this sprint's) has been additive (new fields, new enum variants) - this
/// stays `1` until a future change actually removes or renames a field,
/// which is the point of having it: a place for that change to signal
/// itself, per AGENTS.md's "breaking schema changes must be intentional and
/// documented."
pub const REPORT_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Serialize)]
pub struct Report {
    pub schema_version: u32,
    pub verdict: Verdict,
    /// Whether this report's data cleared every configured `FailureCaps`
    /// threshold - `Valid` unless `--max-timeouts`/`--max-crashes`/
    /// `--max-invalid` was breached, in which case `verdict` above is also
    /// forced to `Inconclusive` (see `verdict::apply_failure_caps`). Always
    /// `Valid` when no cap flag was passed - existing behavior, unchanged.
    pub validity: Validity,
    /// The single go/no-go field: `Promoted` only when `validity` is `Valid`
    /// and `verdict` is `Pass`. Derived, not independently settable.
    pub promotion: Promotion,
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
    /// Rough estimate of additional trials needed to reach a decisive
    /// verdict; see `verdict::estimate_additional_trials` for exactly when
    /// this is `None` (already decided, dead-zone effect, etc.) and the
    /// formula's documented bias. Always serialized, including as `null` -
    /// a fixed JSON key set matters more to machine consumers than omitting
    /// it when absent.
    pub estimated_additional_trials: Option<u64>,
    /// Purely advisory data-quality flags (tiny sample, high failure rate,
    /// draw-heavy Elo run) - unlike `reason`, these never change `verdict`.
    /// Always present, empty when there's nothing to flag. Human-readable
    /// strings derived from the same computation as `data_quality`'s flags
    /// (see `collect_data_quality`) - kept for backward compatibility.
    pub warnings: Vec<String>,
    /// Structured counterpart to `warnings`, additive alongside it (not a
    /// replacement - `REPORT_SCHEMA_VERSION`'s policy is to stay at `1`
    /// until a field is removed/renamed, and this is a pure addition).
    pub data_quality: DataQuality,
    /// The quantile `quantile-diff` measured (e.g. `0.95` for p95) - `None` for every other
    /// metric. Additive alongside every other field, same `REPORT_SCHEMA_VERSION` policy.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quantile: Option<f64>,
    /// `--cluster-by-id` only (winrate/elo) - number of distinct resampling clusters. `None`
    /// when `--cluster-by-id` wasn't requested.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cluster_count: Option<u64>,
    /// `--cluster-by-id` only - the largest single cluster's record count.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_cluster_size: Option<u64>,
    /// `--cluster-by-id` only - `paired_count` deflated by `design_effect` (Kish 1965): how many
    /// truly independent trials this clustered data is actually worth.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effective_sample_size: Option<f64>,
    /// `--cluster-by-id` only - `Var(cluster bootstrap) / Var(i.i.d. bootstrap)` on the same
    /// data; 1.0 means no measurable clustering effect.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub design_effect: Option<f64>,
    /// Multiple-comparison correction fields (see `correction` module) - all
    /// `None`/omitted unless `compare --correction bonferroni|holm` was
    /// requested, so a default run's JSON is byte-identical to before this
    /// existed. `verdict` above already reflects the *adjusted* value once
    /// correction is active; `unadjusted_verdict` keeps the pre-correction
    /// value visible alongside it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub correction_method: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub family_size: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub achieved_alpha: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub adjusted_alpha_threshold: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unadjusted_verdict: Option<crate::Verdict>,
}

/// Machine-checkable data-quality flags, computed together with `warnings`'
/// strings from the same underlying rates/counts (one computation, two
/// representations - not two independently-maintained condition sets that
/// could drift out of sync with each other).
#[derive(Debug, Serialize, Default, Clone, Copy, PartialEq, Eq)]
pub struct DataQuality {
    pub tiny_sample: bool,
    pub high_failure_rate: bool,
    /// `Elo` only - `winrate`/`sign-test` discard their tie/draw count
    /// before it reaches `MetricOutput`, so extending this would need a new
    /// tracked field (deferred, not silently dropped). Always `false` for
    /// every other metric.
    pub draw_heavy: bool,
    /// The effect's own magnitude is smaller than the CI's half-width - the
    /// measured effect could plausibly be noise around zero. A different
    /// condition from `tiny_sample` (which is about `paired_count` alone):
    /// deliberately guarded by `!tiny_sample` so a wide CI from a tiny
    /// sample doesn't also trip this, which would make it a redundant
    /// restatement of "the sample is small" rather than its own signal
    /// (a large-enough sample whose effect is *still* swamped by its own
    /// uncertainty).
    pub effect_within_noise_floor: bool,
    /// One `id` dominates the (unpaired) trial stream - a sign the "N
    /// independent trials" assumption behind the CI doesn't hold, since the
    /// same underlying test case was likely logged multiple times rather
    /// than run N genuinely separate times. Always `false` when
    /// `--paired-by-id` is set (repeated ids mean something different
    /// there) or when too few records carry an `id` for the signal to be
    /// meaningful - see `collect_data_quality`.
    pub low_id_diversity: bool,
    /// `quantile-diff` only - too few observations expected in the thinner tail to estimate this
    /// particular quantile reliably (e.g. p95 on a small sample). Always `false` for every other
    /// metric. See `collect_data_quality` for the threshold.
    pub thin_quantile_tail: bool,
}

/// A metric's effect/CI/thresholds are proportions (winrate, sign-test),
/// Elo points, or raw input units (mean-diff); each reads better in its own
/// unit than as a bare float.
fn fmt_effect(metric: MetricKind, value: f64) -> String {
    match metric {
        MetricKind::WinRate | MetricKind::SignTest => format!("{:+.1} pp", value * 100.0),
        MetricKind::Elo => format!("{value:+.1} elo"),
        MetricKind::MeanDiff | MetricKind::QuantileDiff => format!("{value:+.4}"),
    }
}

/// Renders any `Serialize`-derived enum via its own serde representation,
/// so Markdown output can never drift from the JSON field it mirrors. Also
/// used by `sprt`'s report, which shares the same `Verdict` type.
pub(crate) fn serde_str<T: Serialize>(value: &T) -> String {
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
        let mut body = format!(
            "Verdict: {verdict}\n\
             Validity: {validity}\n\
             Promotion: {promotion}\n\n\
             Metric: {metric}\n\
             Effect: {effect}\n\
             {confidence_pct}% CI: {ci_low} to {ci_high}\n\
             Threshold for pass: {pass_above} / Threshold for fail: {fail_below}\n\n\
             {reason}\n\
             {estimate_line}\n\
             Samples: baseline={baseline_count}, candidate={candidate_count}, paired={paired_count}\n\n\
             Status counts:\n\
             - timeout: {timeouts} (baseline={b_timeout}, candidate={c_timeout})\n\
             - crash: {crashes} (baseline={b_crash}, candidate={c_crash})\n\
             - invalid: {invalid} (baseline={b_invalid}, candidate={c_invalid})\n",
            verdict = serde_str(&self.verdict),
            validity = serde_str(&self.validity),
            promotion = serde_str(&self.promotion),
            metric = match self.quantile {
                Some(q) => format!("{} (q={q:.2})", serde_str(&self.metric)),
                None => serde_str(&self.metric),
            },
            effect = fmt_effect(self.metric, self.effect),
            confidence_pct = self.confidence * 100.0,
            ci_low = fmt_effect(self.metric, self.ci_low),
            ci_high = fmt_effect(self.metric, self.ci_high),
            pass_above = fmt_effect(self.metric, self.pass_above),
            fail_below = fmt_effect(self.metric, self.fail_below),
            reason = self.reason,
            estimate_line = match self.estimated_additional_trials {
                Some(n) =>
                    format!("\nEstimated additional trials to reach a decisive verdict: ~{n}\n"),
                None => String::new(),
            },
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
        );
        if let Some(cluster_count) = self.cluster_count {
            body.push_str(&format!(
                "\nClusters: {cluster_count} (largest: {max_cluster_size}), effective_sample_size={effective_sample_size:.1}, design_effect={design_effect:.2}\n",
                max_cluster_size = self.max_cluster_size.unwrap_or(0),
                effective_sample_size = self.effective_sample_size.unwrap_or(0.0),
                design_effect = self.design_effect.unwrap_or(1.0),
            ));
        }
        if !self.warnings.is_empty() {
            body.push_str("\nWarnings:\n");
            for warning in &self.warnings {
                body.push_str(&format!("- {warning}\n"));
            }
        }
        body
    }
}

/// Output of `compare_many`: one overall verdict plus each metric's own
/// report. Only produced for multi-metric runs; a single `--metric` run
/// keeps printing a plain `Report` so its JSON shape never changes.
#[derive(Debug, Serialize)]
pub struct MultiReport {
    pub schema_version: u32,
    pub verdict: Verdict,
    /// `Invalid` if any per-metric report's `validity` is `Invalid` - see
    /// `verdict::apply_failure_caps_to_multi`.
    pub validity: Validity,
    /// `Promotion::decide(validity, verdict)` above - the one field a
    /// promotion pipeline should read for a multi-metric run.
    pub promotion: Promotion,
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
            "# Veridict Report\n\nOverall verdict: {}\nOverall validity: {}\nOverall promotion: {}\n",
            serde_str(&self.verdict),
            serde_str(&self.validity),
            serde_str(&self.promotion),
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
            schema_version: REPORT_SCHEMA_VERSION,
            verdict: Verdict::Pass,
            validity: Validity::Valid,
            promotion: Promotion::Promoted,
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
            estimated_additional_trials: None,
            warnings: Vec::new(),
            data_quality: DataQuality::default(),
            quantile: None,
            cluster_count: None,
            max_cluster_size: None,
            effective_sample_size: None,
            design_effect: None,
            correction_method: None,
            family_size: None,
            achieved_alpha: None,
            adjusted_alpha_threshold: None,
            unadjusted_verdict: None,
        }
    }

    #[test]
    fn serializes_metric_and_verdict_as_lowercase() {
        let json = sample_report().to_json_pretty();
        assert!(json.contains("\"verdict\": \"pass\""));
        assert!(json.contains("\"metric\": \"winrate\""));
    }

    #[test]
    fn report_and_multi_report_both_carry_schema_version() {
        let report_json = sample_report().to_json_pretty();
        assert!(report_json.contains("\"schema_version\": 1"));

        let multi = MultiReport {
            schema_version: REPORT_SCHEMA_VERSION,
            verdict: Verdict::Pass,
            validity: Validity::Valid,
            promotion: Promotion::Promoted,
            reports: vec![sample_report()],
        };
        assert!(multi.to_json_pretty().contains("\"schema_version\": 1"));
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
    fn markdown_reports_effect_in_elo_units() {
        let mut report = sample_report();
        report.metric = MetricKind::Elo;
        report.effect = 12.3;
        let md = report.to_markdown();
        assert!(md.contains("+12.3 elo"));
    }

    #[test]
    fn estimated_additional_trials_null_is_omitted_from_markdown_but_present_in_json() {
        let report = sample_report();
        assert!(!report.to_markdown().contains("Estimated additional trials"));
        assert!(
            report
                .to_json_pretty()
                .contains("\"estimated_additional_trials\": null")
        );
    }

    #[test]
    fn estimated_additional_trials_some_appears_in_markdown_and_json() {
        let mut report = sample_report();
        report.estimated_additional_trials = Some(750);
        assert!(
            report
                .to_markdown()
                .contains("Estimated additional trials to reach a decisive verdict: ~750")
        );
        assert!(
            report
                .to_json_pretty()
                .contains("\"estimated_additional_trials\": 750")
        );
    }

    #[test]
    fn empty_warnings_produce_no_markdown_section_but_serialize_as_empty_array() {
        let report = sample_report();
        assert!(!report.to_markdown().contains("Warnings:"));
        assert!(report.to_json_pretty().contains("\"warnings\": []"));
    }

    #[test]
    fn non_empty_warnings_are_bulleted_in_markdown_and_present_in_json() {
        let mut report = sample_report();
        report.warnings = vec![
            "small sample size".to_string(),
            "high failure rate".to_string(),
        ];
        let md = report.to_markdown();
        assert!(md.contains("Warnings:\n- small sample size\n- high failure rate\n"));
        assert!(report.to_json_pretty().contains("\"small sample size\""));
    }

    #[test]
    fn multi_report_wraps_each_metric_section() {
        let multi = MultiReport {
            schema_version: REPORT_SCHEMA_VERSION,
            verdict: Verdict::Fail,
            validity: Validity::Valid,
            promotion: Promotion::NotPromoted,
            reports: vec![sample_report()],
        };
        let md = multi.to_markdown();
        assert!(md.contains("Overall verdict: fail"));
        assert!(md.contains("Verdict: pass"));
        let json = multi.to_json_pretty();
        assert!(json.contains("\"reports\""));
    }
}
