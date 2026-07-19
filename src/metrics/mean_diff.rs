//! Percentile bootstrap confidence interval on `candidate - baseline` for
//! paired numeric records.

use crate::error::VeridictError;
use crate::input::Record;
use crate::metrics::common::DiffCollector;
use crate::metrics::{FailureBreakdown, MetricAggregator, MetricOutput, metric_label};
use crate::stats::bootstrap;
use crate::{BootstrapMethod, MetricKind, TrialStatus};

pub(crate) struct MeanDiffAggregator {
    collector: DiffCollector,
    confidence: f64,
    resamples: usize,
    seed: u64,
    bootstrap_method: BootstrapMethod,
}

impl MeanDiffAggregator {
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
        }
    }
}

impl MetricAggregator for MeanDiffAggregator {
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
            if !c.is_finite() {
                return Err(VeridictError::InvalidValue {
                    line,
                    field: "candidate",
                    value: c,
                });
            }
            used = true;
            self.collector.record(line, record.id.as_deref(), c - b)?;
        }
        if !used {
            return Err(VeridictError::SchemaMismatch {
                line,
                context: metric_label(MetricKind::MeanDiff),
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
                warning: Some("no paired numeric trials to compute mean difference".to_string()),
                records_with_id: 0,
                max_id_count: 0,
                quantile: None,
                cluster_count: None,
                max_cluster_size: None,
                effective_sample_size: None,
                design_effect: None,
            });
        }
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
        })
    }
}
