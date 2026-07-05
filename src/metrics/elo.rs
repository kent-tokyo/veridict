//! Elo difference from win/loss/draw counts: `score = (candidate_wins + 0.5
//! * draws) / n`, converted via the standard logistic Elo model. The score
//! rate's Wilson CI treats each trial as a plain Bernoulli draw, which
//! overstates variance versus the true trinomial distribution (a draw's
//! half-point carries less variance than a coin flip) - a deliberately
//! conservative (too-wide, never too-narrow) approximation, same tradeoff
//! already accepted for `sign-test`.

use crate::MetricKind;
use crate::error::VeridictError;
use crate::input::Record;
use crate::metrics::common::OutcomeCollector;
use crate::metrics::{FailureBreakdown, MetricAggregator, MetricOutput, metric_label};
use crate::stats::{elo, wilson};

pub(crate) struct EloAggregator {
    collector: OutcomeCollector,
    confidence: f64,
}

impl EloAggregator {
    pub(crate) fn new(confidence: f64, paired_by_id: bool) -> Self {
        Self {
            collector: OutcomeCollector::new(paired_by_id),
            confidence,
        }
    }
}

impl MetricAggregator for EloAggregator {
    fn ingest(
        &mut self,
        line: usize,
        record: &Record,
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
                        expected: "baseline_win|candidate_win|draw",
                    });
                }
            }
        }
        if !used {
            return Err(VeridictError::SchemaMismatch {
                line,
                context: metric_label(MetricKind::Elo),
                detail: "record has no fields usable by this metric and no status fields"
                    .to_string(),
            });
        }
        Ok(())
    }

    fn finish(self: Box<Self>, failures: &FailureBreakdown) -> Result<MetricOutput, VeridictError> {
        let (baseline_wins, candidate_wins, draws) = self.collector.finish()?;
        let timeouts = failures.baseline.timeout + failures.candidate.timeout;
        let crashes = failures.baseline.crash + failures.candidate.crash;
        let invalid = failures.baseline.invalid + failures.candidate.invalid;

        let n = baseline_wins + candidate_wins + draws;
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
                warning: Some("no trials to compute an Elo difference".to_string()),
            });
        }
        let score = (candidate_wins as f64 + 0.5 * draws as f64) / n as f64;
        let (lo, hi) = wilson::wilson_ci_from_proportion(score, n as f64, self.confidence)?;
        Ok(MetricOutput {
            effect: elo::elo_from_score(score),
            ci_low: elo::elo_from_score(lo),
            ci_high: elo::elo_from_score(hi),
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
