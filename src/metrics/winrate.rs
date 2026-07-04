//! Win rate: Wilson score interval over decisive (non-draw) trials only.
//! `p_hat = candidate_wins / (candidate_wins + baseline_wins)`. Draws are
//! counted among "used" records but excluded from the proportion, matching
//! standard paired-match testing practice (a draw is neither a win nor a
//! loss). `effect`/`ci_low`/`ci_high` are centered on 0 (deviation from a
//! 50/50 split) so they compose with `--min-effect`/`--pass-above`/
//! `--fail-below`, which are expressed as signed deltas.

use crate::error::VeridictError;
use crate::input::Record;
use crate::metrics::common::OutcomeCollector;
use crate::metrics::{FailureBreakdown, MetricAggregator, MetricOutput, metric_label};
use crate::stats::{exact, wilson};
use crate::{CiMethod, MetricKind};

pub(crate) struct WinRateAggregator<'a> {
    collector: OutcomeCollector<'a>,
    confidence: f64,
    ci_method: CiMethod,
}

impl<'a> WinRateAggregator<'a> {
    pub(crate) fn new(confidence: f64, paired_by_id: bool, ci_method: CiMethod) -> Self {
        Self {
            collector: OutcomeCollector::new(paired_by_id),
            confidence,
            ci_method,
        }
    }
}

impl<'a> MetricAggregator<'a> for WinRateAggregator<'a> {
    fn ingest(
        &mut self,
        line: usize,
        record: &'a Record,
        has_status: bool,
    ) -> Result<(), VeridictError> {
        let mut used = has_status;
        if let Some(result) = record.result.as_deref() {
            used = true;
            match crate::Outcome::parse(result) {
                Some(outcome) => self.collector.record(line, record.id.as_deref(), outcome),
                None => {
                    return Err(VeridictError::UnrecognizedOutcome {
                        line,
                        value: result.to_string(),
                    });
                }
            }
        }
        if !used {
            return Err(VeridictError::SchemaMismatch {
                line,
                context: metric_label(MetricKind::WinRate),
                detail: "record has no fields usable by this metric and no status fields"
                    .to_string(),
            });
        }
        Ok(())
    }

    fn finish(self: Box<Self>, failures: &FailureBreakdown) -> Result<MetricOutput, VeridictError> {
        let (baseline_wins, candidate_wins, _draws) = self.collector.finish()?;
        let timeouts = failures.baseline.timeout + failures.candidate.timeout;
        let crashes = failures.baseline.crash + failures.candidate.crash;
        let invalid = failures.baseline.invalid + failures.candidate.invalid;

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
                failures: *failures,
                warning: Some("no decisive (non-draw) trials to compute win rate".to_string()),
            });
        }
        let (lo, hi) = match self.ci_method {
            CiMethod::Wilson => wilson::wilson_ci(candidate_wins, n, self.confidence)?,
            CiMethod::Exact => exact::clopper_pearson_ci(candidate_wins, n, self.confidence)?,
        };
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
            failures: *failures,
            warning: None,
        })
    }
}
