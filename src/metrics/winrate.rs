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
use crate::metrics::{
    FailureBreakdown, MetricAggregator, MetricOutput, effective_outcome, metric_label,
};
use crate::stats::bootstrap::cluster_bootstrap_ci;
use crate::stats::{exact, jeffreys, wilson};
use crate::{CiMethod, FailurePolicy, MetricKind, TrialStatus};

pub(crate) struct WinRateAggregator {
    collector: OutcomeCollector,
    confidence: f64,
    ci_method: CiMethod,
    failure_policy: FailurePolicy,
    cluster_by_id: bool,
    resamples: usize,
    seed: u64,
}

impl WinRateAggregator {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        confidence: f64,
        paired_by_id: bool,
        cluster_by_id: bool,
        ci_method: CiMethod,
        failure_policy: FailurePolicy,
        resamples: usize,
        seed: u64,
    ) -> Self {
        Self {
            collector: OutcomeCollector::new(paired_by_id, cluster_by_id),
            confidence,
            ci_method,
            failure_policy,
            cluster_by_id,
            resamples,
            seed,
        }
    }
}

/// `candidate_wins / (candidate_wins + baseline_wins)` - the raw proportion, same scale
/// `wilson_ci`/`exact`/`jeffreys` already return (not yet recentered to the `-0.5` "effect"
/// scale `finish` converts to at the very end, for both branches uniformly). `NaN` when there
/// are no decisive outcomes to divide by - `cluster_bootstrap_outcome_draws` retries a resample
/// that lands here rather than ever letting a `NaN` reach a report field (see its own doc).
fn winrate_statistic(baseline_wins: u64, candidate_wins: u64, _draws: u64) -> f64 {
    let n = baseline_wins + candidate_wins;
    if n == 0 {
        f64::NAN
    } else {
        candidate_wins as f64 / n as f64
    }
}

impl MetricAggregator for WinRateAggregator {
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
                context: metric_label(MetricKind::WinRate),
                detail: "record has no fields usable by this metric and no status fields"
                    .to_string(),
            });
        }
        Ok(())
    }

    fn finish(self: Box<Self>, failures: &FailureBreakdown) -> Result<MetricOutput, VeridictError> {
        let confidence = self.confidence;
        let ci_method = self.ci_method;
        let resamples = self.resamples;
        let seed = self.seed;
        let (baseline_wins, candidate_wins, _draws, clusters) = if self.cluster_by_id {
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
                records_with_id: 0,
                max_id_count: 0,
                quantile: None,
                cluster_count: None,
                max_cluster_size: None,
                effective_sample_size: None,
                design_effect: None,
            });
        }
        let p_hat = candidate_wins as f64 / n as f64;

        let (lo, hi, cluster_count, max_cluster_size, effective_sample_size, design_effect) =
            match &clusters {
                Some(clusters) => {
                    let result = cluster_bootstrap_ci(
                        clusters,
                        n,
                        confidence,
                        resamples,
                        seed,
                        winrate_statistic,
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
                    let (lo, hi) = match ci_method {
                        CiMethod::Wilson => wilson::wilson_ci(candidate_wins, n, confidence)?,
                        CiMethod::Exact => {
                            exact::clopper_pearson_ci(candidate_wins, n, confidence)?
                        }
                        CiMethod::Jeffreys => jeffreys::jeffreys_ci(candidate_wins, n, confidence)?,
                    };
                    (lo, hi, None, None, None, None)
                }
            };
        Ok(MetricOutput {
            effect: p_hat - 0.5,
            ci_low: lo - 0.5,
            ci_high: hi - 0.5,
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
