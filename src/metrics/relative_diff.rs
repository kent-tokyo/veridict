//! Percentile/basic/BCa bootstrap confidence interval on `(candidate - baseline) / baseline` for
//! paired numeric records - proportional change relative to the baseline, rather than `mean-diff`'s
//! absolute `candidate - baseline`. See `docs/metrics.md`'s `relative-diff` section for why this is
//! an independent metric (different units, different baseline constraint, non-symmetric under
//! swapping the arms) rather than a display mode of `mean-diff`.

use crate::error::VeridictError;
use crate::input::Record;
use crate::metrics::common::{DiffCollector, compute_scale_diagnostics};
use crate::metrics::{FailureBreakdown, MetricAggregator, MetricOutput, metric_label};
use crate::stats::bootstrap;
use crate::{BootstrapMethod, MetricKind, TrialStatus};

pub(crate) struct RelativeDiffAggregator {
    collector: DiffCollector,
    confidence: f64,
    resamples: usize,
    seed: u64,
    bootstrap_method: BootstrapMethod,
    /// Raw baseline values, in ingestion order, for `scale_diagnostics` - see `MeanDiffAggregator`'s
    /// identically-purposed field. Every value pushed here is already validated `> 0` and finite (a
    /// non-positive or non-finite baseline is rejected before reaching this point), so
    /// `non_positive_baseline_count` in this metric's own `ScaleDiagnostics` is always `0`.
    baselines: Vec<f64>,
}

impl RelativeDiffAggregator {
    pub(crate) fn new(
        confidence: f64,
        resamples: usize,
        seed: u64,
        paired_by_id: bool,
        bootstrap_method: BootstrapMethod,
    ) -> Self {
        Self {
            collector: DiffCollector::new(paired_by_id),
            confidence,
            resamples,
            seed,
            bootstrap_method,
            baselines: Vec::new(),
        }
    }
}

impl MetricAggregator for RelativeDiffAggregator {
    fn ingest(
        &mut self,
        line: usize,
        record: &Record,
        baseline_status: Option<TrialStatus>,
        candidate_status: Option<TrialStatus>,
    ) -> Result<(), VeridictError> {
        let mut used = baseline_status.is_some() || candidate_status.is_some();
        if let (Some(b), Some(c)) = (record.baseline, record.candidate) {
            if !b.is_finite() {
                return Err(VeridictError::InvalidValue {
                    line,
                    field: "baseline",
                    value: b,
                });
            }
            // Zero and negative baselines are both rejected here, not just zero - see
            // `VeridictError::RelativeDiffRequiresPositiveBaseline`'s doc for why a negative
            // baseline is rejected too, rather than silently dividing by `abs(baseline)` (a
            // different effect size that would quietly change what the number means) or adding an
            // epsilon to the denominator (which would fabricate a number rather than reporting
            // that this record's percentage change isn't well-defined).
            if b <= 0.0 {
                return Err(VeridictError::RelativeDiffRequiresPositiveBaseline {
                    line,
                    baseline: b,
                });
            }
            if !c.is_finite() {
                return Err(VeridictError::InvalidValue {
                    line,
                    field: "candidate",
                    value: c,
                });
            }
            used = true;
            self.baselines.push(b);
            let relative = (c - b) / b;
            // `b > 0` and `c` finite doesn't guarantee `(c - b) / b` is finite - a large finite `c`
            // over a tiny finite `b` (e.g. candidate=1e300, baseline=1e-300) can overflow to
            // infinity even though every raw input value was itself a legal, finite number.
            if !relative.is_finite() {
                return Err(VeridictError::InvalidValue {
                    line,
                    field: "relative_diff",
                    value: relative,
                });
            }
            self.collector
                .record(line, record.id.as_deref(), relative)?;
        }
        if !used {
            return Err(VeridictError::SchemaMismatch {
                line,
                context: metric_label(MetricKind::RelativeDiff),
                detail: "record has no fields usable by this metric and no status fields"
                    .to_string(),
            });
        }
        Ok(())
    }

    fn finish(self: Box<Self>, failures: &FailureBreakdown) -> Result<MetricOutput, VeridictError> {
        let diffs = self.collector.finish()?;
        let timeouts = failures.baseline.timeout + failures.candidate.timeout;
        let crashes = failures.baseline.crash + failures.candidate.crash;
        let invalid = failures.baseline.invalid + failures.candidate.invalid;

        if diffs.is_empty() {
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
                failures: *failures,
                warning: Some(
                    "no paired numeric trials to compute relative difference".to_string(),
                ),
                records_with_id: 0,
                max_id_count: 0,
                quantile: None,
                cluster_count: None,
                max_cluster_size: None,
                effective_sample_size: None,
                design_effect: None,
                scale_diagnostics: None,
            });
        }
        // The effect is `mean(relative_diff_i)` - the average of each pair's own ratio, not
        // `sum(candidate_i) / sum(baseline_i) - 1` (a total-volume ratio). Averaging per-pair ratios
        // treats every benchmark case as one equally-weighted observation, matching how `mean-diff`
        // already treats every pair - a large-baseline case doesn't get more influence over the
        // effect just because its absolute numbers are bigger. The two quantities are generally
        // different whenever baselines vary in scale, which is precisely the situation this metric
        // exists for.
        let effect = bootstrap::mean(&diffs);
        let (ci_low, ci_high) = match self.bootstrap_method {
            BootstrapMethod::Percentile => bootstrap::bootstrap_mean_diff_ci(
                &diffs,
                self.confidence,
                self.resamples,
                self.seed,
            ),
            BootstrapMethod::Bca => bootstrap::bootstrap_mean_diff_ci_bca(
                &diffs,
                self.confidence,
                self.resamples,
                self.seed,
            ),
            BootstrapMethod::Basic => bootstrap::bootstrap_mean_diff_ci_basic(
                &diffs,
                self.confidence,
                self.resamples,
                self.seed,
            ),
        };
        Ok(MetricOutput {
            effect,
            ci_low,
            ci_high,
            baseline_count: diffs.len() as u64,
            candidate_count: diffs.len() as u64,
            paired_count: diffs.len() as u64,
            timeouts,
            crashes,
            invalid,
            failures: *failures,
            warning: None,
            records_with_id: 0,
            max_id_count: 0,
            quantile: None,
            cluster_count: None,
            max_cluster_size: None,
            effective_sample_size: None,
            design_effect: None,
            scale_diagnostics: Some(compute_scale_diagnostics(&self.baselines)),
        })
    }
}
