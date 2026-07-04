//! Nonparametric alternative to `mean-diff`: only the *direction* of each
//! pair's difference matters, not its magnitude. Ties (`candidate ==
//! baseline`) are excluded from `n`, same treatment as draws in `winrate`.
//! The proportion of positive signs is run through the same Wilson CI as
//! `winrate` and centered the same way, since "is the sign test's underlying
//! proportion above 50%, with what confidence" is exactly a binomial
//! proportion question.

use crate::error::VeridictError;
use crate::input::Record;
use crate::metrics::common::DiffCollector;
use crate::metrics::{FailureBreakdown, MetricAggregator, MetricOutput, metric_label};
use crate::stats::{exact, wilson};
use crate::{CiMethod, MetricKind};

pub(crate) struct SignTestAggregator<'a> {
    collector: DiffCollector<'a>,
    confidence: f64,
    ci_method: CiMethod,
}

impl<'a> SignTestAggregator<'a> {
    pub(crate) fn new(confidence: f64, paired_by_id: bool, ci_method: CiMethod) -> Self {
        Self {
            collector: DiffCollector::new(paired_by_id),
            confidence,
            ci_method,
        }
    }
}

impl<'a> MetricAggregator<'a> for SignTestAggregator<'a> {
    fn ingest(
        &mut self,
        line: usize,
        record: &'a Record,
        has_status: bool,
    ) -> Result<(), VeridictError> {
        let mut used = has_status;
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
                context: metric_label(MetricKind::SignTest),
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
                failures: *failures,
                warning: Some("no non-tied paired trials to compute the sign test".to_string()),
            });
        }
        let (lo, hi) = match self.ci_method {
            CiMethod::Wilson => wilson::wilson_ci(positive, n, self.confidence)?,
            CiMethod::Exact => exact::clopper_pearson_ci(positive, n, self.confidence)?,
        };
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
            failures: *failures,
            warning: None,
        })
    }
}
