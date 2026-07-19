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
use crate::metrics::{
    FailureBreakdown, MetricAggregator, MetricOutput, effective_outcome, metric_label,
};
use crate::stats::bootstrap::cluster_bootstrap_ci;
use crate::stats::{elo, wilson};
use crate::{FailurePolicy, TrialStatus};

pub(crate) struct EloAggregator {
    collector: OutcomeCollector,
    confidence: f64,
    failure_policy: FailurePolicy,
    cluster_by_id: bool,
    resamples: usize,
    seed: u64,
}

impl EloAggregator {
    pub(crate) fn new(
        confidence: f64,
        paired_by_id: bool,
        cluster_by_id: bool,
        failure_policy: FailurePolicy,
        resamples: usize,
        seed: u64,
    ) -> Self {
        Self {
            collector: OutcomeCollector::new(paired_by_id, cluster_by_id),
            confidence,
            failure_policy,
            cluster_by_id,
            resamples,
            seed,
        }
    }
}

/// `elo_from_score((candidate_wins + 0.5*draws) / n)` - always well-defined for any nonempty
/// resample (every cluster is nonempty by construction, so `n >= 1` whenever at least one
/// cluster was drawn), unlike `winrate_statistic` this never needs a retry.
fn elo_statistic(baseline_wins: u64, candidate_wins: u64, draws: u64) -> f64 {
    let n = baseline_wins + candidate_wins + draws;
    let score = (candidate_wins as f64 + 0.5 * draws as f64) / n as f64;
    elo::elo_from_score(score)
}

impl MetricAggregator for EloAggregator {
    fn ingest(
        &mut self,
        line: usize,
        record: &Record,
        baseline_status: Option<TrialStatus>,
        candidate_status: Option<TrialStatus>,
    ) -> Result<(), VeridictError> {
        let used =
            baseline_status.is_some() || candidate_status.is_some() || record.result.is_some();
        if let Some(outcome) = effective_outcome(
            self.failure_policy,
            baseline_status,
            candidate_status,
            record.result.as_deref(),
            line,
        )? {
            self.collector.record(line, record.id.as_deref(), outcome);
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
        let confidence = self.confidence;
        let resamples = self.resamples;
        let seed = self.seed;
        let (baseline_wins, candidate_wins, draws, clusters) = if self.cluster_by_id {
            let clusters = self.collector.finish_clusters();
            let (b, c, d) =
                clusters
                    .iter()
                    .flatten()
                    .fold((0u64, 0u64, 0u64), |(b, c, d), outcome| match outcome {
                        crate::Outcome::BaselineWin => (b + 1, c, d),
                        crate::Outcome::CandidateWin => (b, c + 1, d),
                        crate::Outcome::Draw => (b, c, d + 1),
                    });
            (b, c, d, Some(clusters))
        } else {
            let (b, c, d) = self.collector.finish()?;
            (b, c, d, None)
        };
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
                records_with_id: 0,
                max_id_count: 0,
                quantile: None,
                cluster_count: None,
                max_cluster_size: None,
                effective_sample_size: None,
                design_effect: None,
            });
        }
        let score = (candidate_wins as f64 + 0.5 * draws as f64) / n as f64;

        let (lo, hi, cluster_count, max_cluster_size, effective_sample_size, design_effect) =
            match &clusters {
                Some(clusters) => {
                    let result = cluster_bootstrap_ci(
                        clusters,
                        n,
                        confidence,
                        resamples,
                        seed,
                        elo_statistic,
                    );
                    (
                        result.ci_low,
                        result.ci_high,
                        Some(clusters.len() as u64),
                        clusters.iter().map(|c| c.len() as u64).max(),
                        Some(result.effective_sample_size),
                        Some(result.design_effect),
                    )
                }
                None => {
                    let (lo, hi) = wilson::wilson_ci_from_proportion(score, n as f64, confidence)?;
                    (
                        elo::elo_from_score(lo),
                        elo::elo_from_score(hi),
                        None,
                        None,
                        None,
                        None,
                    )
                }
            };
        Ok(MetricOutput {
            effect: elo::elo_from_score(score),
            ci_low: lo,
            ci_high: hi,
            baseline_count: baseline_wins,
            candidate_count: candidate_wins,
            paired_count: n,
            cluster_count,
            max_cluster_size,
            effective_sample_size,
            design_effect,
            timeouts,
            crashes,
            invalid,
            failures: *failures,
            warning: None,
            records_with_id: 0,
            max_id_count: 0,
            quantile: None,
        })
    }
}
