//! Fixed-sample logistic Elo for complete two-trial pairs.
//!
//! Each pair contributes its normalized combined candidate score in
//! `{0, 0.25, 0.5, 0.75, 1}`. The point estimate is the logistic-Elo transform of the mean pair
//! score. The confidence interval is a deterministic percentile bootstrap that resamples whole
//! pairs, preserving within-pair correlation and the five score categories. It is a fixed-sample
//! effect/uncertainty report, not a sequential test and not a replacement for pentanomial SPRT.

use std::collections::HashMap;

use crate::error::VeridictError;
use crate::input::Record;
use crate::metrics::{
    FailureBreakdown, MetricAggregator, MetricOutput, effective_outcome, metric_label,
};
use crate::report::PairScoreCounts;
use crate::stats::bootstrap::{bootstrap_mean_diff_ci, mean};
use crate::stats::elo;
use crate::{FailurePolicy, MetricKind, Outcome, TrialStatus};

pub(crate) struct PairedEloAggregator {
    confidence: f64,
    failure_policy: FailurePolicy,
    resamples: usize,
    seed: u64,
    groups: HashMap<String, Vec<PairRecord>>,
}

#[derive(Clone, Copy)]
struct PairRecord {
    line: usize,
    outcome: Option<Outcome>,
}

impl PairedEloAggregator {
    pub(crate) fn new(
        confidence: f64,
        failure_policy: FailurePolicy,
        resamples: usize,
        seed: u64,
    ) -> Self {
        Self {
            confidence,
            failure_policy,
            resamples,
            seed,
            groups: HashMap::new(),
        }
    }

    fn record(
        &mut self,
        line: usize,
        id: Option<&str>,
        outcome: Option<Outcome>,
    ) -> Result<(), VeridictError> {
        let id = id
            .filter(|id| !id.is_empty())
            .ok_or_else(|| VeridictError::SchemaMismatch {
                line,
                context: metric_label(MetricKind::PairedElo),
                detail: "every record requires a non-empty id; pair membership cannot be inferred"
                    .to_string(),
            })?;
        let group = self.groups.entry(id.to_string()).or_default();
        group.push(PairRecord { line, outcome });
        if group.len() > 2 {
            return Err(VeridictError::SchemaMismatch {
                line,
                context: metric_label(MetricKind::PairedElo),
                detail: format!(
                    "id '{id}' appears more than twice; paired-elo requires exactly 2 usable \
                     records per id"
                ),
            });
        }
        Ok(())
    }
}

fn half_points(outcome: Outcome) -> usize {
    match outcome {
        Outcome::BaselineWin => 0,
        Outcome::Draw => 1,
        Outcome::CandidateWin => 2,
    }
}

impl MetricAggregator for PairedEloAggregator {
    fn ingest(
        &mut self,
        line: usize,
        record: &Record,
        baseline_status: Option<TrialStatus>,
        candidate_status: Option<TrialStatus>,
    ) -> Result<(), VeridictError> {
        let used =
            baseline_status.is_some() || candidate_status.is_some() || record.result.is_some();
        if !used {
            return Err(VeridictError::SchemaMismatch {
                line,
                context: metric_label(MetricKind::PairedElo),
                detail: "record has no fields usable by this metric and no status fields"
                    .to_string(),
            });
        }
        let outcome = effective_outcome(
            self.failure_policy,
            baseline_status,
            candidate_status,
            record.result.as_deref(),
            line,
        )?;
        self.record(line, record.id.as_deref(), outcome)
    }

    fn finish(self: Box<Self>, failures: &FailureBreakdown) -> Result<MetricOutput, VeridictError> {
        let mut groups: Vec<_> = self.groups.into_iter().collect();
        groups.sort_by_key(|(_, records)| records.iter().map(|r| r.line).min().unwrap());

        let mut buckets = [0u64; 5];
        let mut normalized_scores = Vec::with_capacity(groups.len());
        let mut baseline_wins = 0u64;
        let mut candidate_wins = 0u64;

        for (id, records) in groups {
            if records.len() != 2 {
                return Err(VeridictError::SchemaMismatch {
                    line: records[0].line,
                    context: metric_label(MetricKind::PairedElo),
                    detail: format!(
                        "id '{id}' appears once; paired-elo requires exactly 2 usable records per id"
                    ),
                });
            }
            let usable = records.iter().filter(|r| r.outcome.is_some()).count();
            if usable != 2 {
                return Err(VeridictError::SchemaMismatch {
                    line: records[0].line,
                    context: metric_label(MetricKind::PairedElo),
                    detail: format!(
                        "id '{id}' has {usable} usable outcome(s); paired-elo requires exactly 2 \
                         usable records per id"
                    ),
                });
            }
            let a = records[0].outcome.expect("usable count checked above");
            let b = records[1].outcome.expect("usable count checked above");
            for outcome in [a, b] {
                match outcome {
                    Outcome::BaselineWin => baseline_wins += 1,
                    Outcome::CandidateWin => candidate_wins += 1,
                    Outcome::Draw => {}
                }
            }
            let bucket = half_points(a) + half_points(b);
            buckets[bucket] += 1;
            normalized_scores.push(bucket as f64 / 4.0);
        }

        let timeouts = failures.baseline.timeout + failures.candidate.timeout;
        let crashes = failures.baseline.crash + failures.candidate.crash;
        let invalid = failures.baseline.invalid + failures.candidate.invalid;
        if normalized_scores.is_empty() {
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
                warning: Some("no complete pairs to compute paired Elo".to_string()),
                records_with_id: 0,
                max_id_count: 0,
                quantile: None,
                cluster_count: None,
                max_cluster_size: None,
                effective_sample_size: None,
                design_effect: None,
                scale_diagnostics: None,
                tied_count: None,
                pair_score_counts: Some(PairScoreCounts::default()),
            });
        }

        let score = mean(&normalized_scores);
        let (score_lo, score_hi) = bootstrap_mean_diff_ci(
            &normalized_scores,
            self.confidence,
            self.resamples,
            self.seed,
        );
        Ok(MetricOutput {
            effect: elo::elo_from_score(score),
            ci_low: elo::elo_from_score(score_lo),
            ci_high: elo::elo_from_score(score_hi),
            baseline_count: baseline_wins,
            candidate_count: candidate_wins,
            paired_count: normalized_scores.len() as u64,
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
            scale_diagnostics: None,
            tied_count: None,
            pair_score_counts: Some(PairScoreCounts::from_buckets(buckets)),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn five_pair_scores_map_to_five_distinct_buckets() {
        let outcomes = [
            (Outcome::BaselineWin, Outcome::BaselineWin),
            (Outcome::BaselineWin, Outcome::Draw),
            (Outcome::CandidateWin, Outcome::BaselineWin),
            (Outcome::CandidateWin, Outcome::Draw),
            (Outcome::CandidateWin, Outcome::CandidateWin),
        ];
        for (expected, (a, b)) in outcomes.into_iter().enumerate() {
            assert_eq!(half_points(a) + half_points(b), expected);
        }
    }
}
