//! Bootstrap confidence interval on the `quantile`-th quantile of `candidate - baseline` for
//! paired numeric records - the same input shape as `mean_diff`, generalized from the mean to an
//! arbitrary quantile (median, p90, p95, ...).

use crate::error::VeridictError;
use crate::input::Record;
use crate::metrics::common::DiffCollector;
use crate::metrics::{FailureBreakdown, MetricAggregator, MetricOutput, metric_label};
use crate::stats::bootstrap;
use crate::{BootstrapMethod, MetricKind, TrialStatus};

pub(crate) struct QuantileDiffAggregator {
    collector: DiffCollector,
    confidence: f64,
    resamples: usize,
    seed: u64,
    quantile: f64,
    bootstrap_method: BootstrapMethod,
}

impl QuantileDiffAggregator {
    pub(crate) fn new(
        confidence: f64,
        resamples: usize,
        seed: u64,
        paired_by_id: bool,
        quantile: f64,
        bootstrap_method: BootstrapMethod,
    ) -> Self {
        Self {
            collector: DiffCollector::new(paired_by_id),
            confidence,
            resamples,
            seed,
            quantile,
            bootstrap_method,
        }
    }
}

impl MetricAggregator for QuantileDiffAggregator {
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
                context: metric_label(MetricKind::QuantileDiff),
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
                    "no paired numeric trials to compute quantile difference".to_string(),
                ),
                records_with_id: 0,
                max_id_count: 0,
                quantile: Some(self.quantile),
            });
        }
        let effect = bootstrap::quantile(&diffs, self.quantile);
        let (ci_low, ci_high) = match self.bootstrap_method {
            BootstrapMethod::Percentile => bootstrap::bootstrap_quantile_diff_ci(
                &diffs,
                self.quantile,
                self.confidence,
                self.resamples,
                self.seed,
            ),
            // Unreachable via the CLI - `MetricConfig::new` rejects `Bca` for `quantile-diff`
            // before a `QuantileDiffAggregator` is ever built (see
            // `VeridictError::IncompatibleBootstrapMethod`). Kept total rather than
            // `unreachable!()` since a caller could still construct `MetricConfig::QuantileDiff`
            // directly, bypassing that guard.
            BootstrapMethod::Bca => bootstrap::bootstrap_quantile_diff_ci_bca(
                &diffs,
                self.quantile,
                self.confidence,
                self.resamples,
                self.seed,
            ),
            BootstrapMethod::Basic => bootstrap::bootstrap_quantile_diff_ci_basic(
                &diffs,
                self.quantile,
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
            quantile: Some(self.quantile),
        })
    }
}
