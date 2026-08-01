//! Shared per-record collectors used by the metric aggregators: gather one
//! observation per record, then (if `paired_by_id`) reduce same-id pairs
//! into a single net observation at the end. Kept out of the per-metric
//! files since WinRate/Elo and SignTest reuse the same collection shape
//! (MeanDiff's `DiffCollector` is close but needs to retain every raw diff
//! for bootstrap resampling, so it isn't shared further than that).
//!
//! Ids are owned `String`s, not borrowed `&str`, even though every caller
//! today happens to have a `Record` alive for the duration of a `finish()`
//! call: these collectors are fed one record at a time from a streaming
//! iterator (see `metrics::compute_many`), so nothing can borrow past a
//! single `ingest` call. One allocation per distinct id is a small, honest
//! constant-factor cost - not a memory-scaling regression.

use std::collections::{HashMap, HashSet};

use crate::Outcome;
use crate::error::VeridictError;
use crate::report::ScaleDiagnostics;
use crate::stats::bootstrap;

/// Below this many positive baselines, `raw_orders_of_magnitude` (the full min/max span) is the
/// only signal available - not enough data yet for a percentile-trimmed span to mean anything more
/// than the raw one. At or above it, `robust_orders_of_magnitude` (p95/p05) becomes the primary
/// signal instead, so one extreme outlier can't single-handedly trigger `wide_baseline_scale` on an
/// otherwise tightly-scaled dataset - see `collect_data_quality` in `lib.rs`.
pub(crate) const ROBUST_SPAN_MIN_POSITIVE_BASELINES: usize = 20;

/// Computes `mean-diff`/`relative-diff`'s scale-mismatch diagnostic from raw, per-record baseline
/// values alone - deliberately the *only* input this takes. Candidate values, the metric's effect,
/// its CI, and its verdict are all in scope by the time an aggregator's `finish()` calls this, but
/// none of them are passed in: if this function could see any of them, "does the scale look wide"
/// could end up correlated with "did the result look good," and a diagnostic that only exists to
/// recommend *before* looking at results would quietly become one more thing tuned by looking at
/// results. Order-independent (only counts/min/max/percentiles of the multiset matter), so callers
/// can pass baselines in ingestion order without sorting first.
pub(crate) fn compute_scale_diagnostics(baselines: &[f64]) -> ScaleDiagnostics {
    let mut positive: Vec<f64> = baselines.iter().copied().filter(|&b| b > 0.0).collect();
    let non_positive_baseline_count = (baselines.len() - positive.len()) as u64;
    if positive.is_empty() {
        return ScaleDiagnostics {
            positive_baseline_count: 0,
            non_positive_baseline_count,
            min_positive_baseline: 0.0,
            max_positive_baseline: 0.0,
            raw_orders_of_magnitude: 0.0,
            robust_orders_of_magnitude: None,
        };
    }
    positive.sort_by(f64::total_cmp);
    let min_positive_baseline = positive[0];
    let max_positive_baseline = positive[positive.len() - 1];
    // log10(max) - log10(min), not log10(max / min): both baselines are finite, but their
    // quotient can overflow to inf (e.g. 1e-300 vs 1e100), which would serialize as JSON `null`
    // and violate the schema's `"type": "number"`. Subtracting logs never overflows here.
    let raw_orders_of_magnitude = max_positive_baseline.log10() - min_positive_baseline.log10();
    let robust_orders_of_magnitude = if positive.len() >= ROBUST_SPAN_MIN_POSITIVE_BASELINES {
        // `bootstrap::quantile` re-sorts internally; `positive` is already sorted, but the input is
        // small enough (a per-run baseline list, not a bootstrap resample) that re-sorting it is not
        // worth a second, sort-skipping code path just for this call site.
        let p05 = bootstrap::quantile(&positive, 0.05);
        let p95 = bootstrap::quantile(&positive, 0.95);
        Some(p95.log10() - p05.log10())
    } else {
        None
    };
    ScaleDiagnostics {
        positive_baseline_count: positive.len() as u64,
        non_positive_baseline_count,
        min_positive_baseline,
        max_positive_baseline,
        raw_orders_of_magnitude,
        robust_orders_of_magnitude,
    }
}

/// Shared by WinRate and Elo: one win/loss/draw observation per record.
/// Order-independent (only integer tallies come out), so no ordering
/// concern the way `DiffCollector` has. Without `paired_by_id`/
/// `cluster_by_id`, this is O(1) memory (three counters); with either,
/// memory scales with the number of distinct ids not yet resolved (paired)
/// or the number of distinct clusters (clustered), not with total record
/// count.
pub(crate) struct OutcomeCollector {
    paired_by_id: bool,
    cluster_by_id: bool,
    /// Upgrades a lone id under `paired_by_id` from a tolerated unpaired sample into a hard
    /// error - see `finish`. Set by `sprt --require-complete-pairs`; `compare`'s call sites
    /// always pass `false` (not requested there).
    require_complete_pairs: bool,
    baseline_wins: u64,
    candidate_wins: u64,
    draws: u64,
    groups: HashMap<String, Vec<(usize, Outcome)>>,
    /// `cluster_by_id` only: id-less records, each its own singleton
    /// cluster (see `finish_clusters`).
    singletons: Vec<Outcome>,
}

impl OutcomeCollector {
    pub(crate) fn new(
        paired_by_id: bool,
        cluster_by_id: bool,
        require_complete_pairs: bool,
    ) -> Self {
        Self {
            paired_by_id,
            cluster_by_id,
            require_complete_pairs,
            baseline_wins: 0,
            candidate_wins: 0,
            draws: 0,
            groups: HashMap::new(),
            singletons: Vec::new(),
        }
    }

    pub(crate) fn record(&mut self, line: usize, id: Option<&str>, outcome: Outcome) {
        match (self.paired_by_id, self.cluster_by_id, id) {
            (true, _, Some(id)) | (_, true, Some(id)) => {
                self.groups
                    .entry(id.to_string())
                    .or_default()
                    .push((line, outcome));
            }
            (_, true, None) => self.singletons.push(outcome),
            _ => self.tally(outcome),
        }
    }

    fn tally(&mut self, outcome: Outcome) {
        match outcome {
            Outcome::BaselineWin => self.baseline_wins += 1,
            Outcome::CandidateWin => self.candidate_wins += 1,
            Outcome::Draw => self.draws += 1,
        }
    }

    /// `cluster_by_id` only: every distinct id becomes one resampling
    /// cluster (any size, not netted or capped at 2 the way `finish`'s
    /// pairing is), plus one singleton cluster per id-less record - so
    /// every input record ends up in exactly one cluster, ready for
    /// `stats::bootstrap`'s cluster bootstrap. Sorted by each cluster's
    /// minimum line number for the same determinism reason `DiffCollector::
    /// finish` sorts its groups (unspecified `HashMap` iteration order would
    /// otherwise make the seeded bootstrap RNG draw a different cluster
    /// order - and so a different resampled statistic - between process
    /// runs on the same input and seed).
    pub(crate) fn finish_clusters(mut self) -> Vec<Vec<Outcome>> {
        let mut groups: Vec<_> = self.groups.drain().collect();
        groups.sort_by_key(|(_, g)| g.iter().map(|(line, _)| *line).min().unwrap());
        let mut clusters: Vec<Vec<Outcome>> = groups
            .into_iter()
            .map(|(_, g)| g.into_iter().map(|(_, o)| o).collect())
            .collect();
        clusters.extend(self.singletons.drain(..).map(|o| vec![o]));
        clusters
    }

    /// Returns `(baseline_wins, candidate_wins, draws)`.
    pub(crate) fn finish(mut self) -> Result<(u64, u64, u64), VeridictError> {
        let require_complete_pairs = self.require_complete_pairs;
        for (id, group) in std::mem::take(&mut self.groups) {
            match group.as_slice() {
                [(line, outcome)] => {
                    if require_complete_pairs {
                        return Err(VeridictError::SchemaMismatch {
                            line: *line,
                            context: "paired-by-id",
                            detail: format!(
                                "id '{id}' appears once; --require-complete-pairs requires \
                                 exactly 2 records per id"
                            ),
                        });
                    }
                    self.tally(*outcome)
                }
                [(_, a), (_, b)] => {
                    let points = |o: &Outcome| match o {
                        Outcome::CandidateWin => 1.0,
                        Outcome::Draw => 0.5,
                        Outcome::BaselineWin => 0.0,
                    };
                    let total = points(a) + points(b);
                    #[allow(clippy::float_cmp)]
                    let net = if total > 1.0 {
                        Outcome::CandidateWin
                    } else if total < 1.0 {
                        Outcome::BaselineWin
                    } else {
                        Outcome::Draw
                    };
                    self.tally(net);
                }
                more => {
                    return Err(VeridictError::SchemaMismatch {
                        line: more[0].0,
                        context: "paired-by-id",
                        detail: format!(
                            "id '{id}' appears {} times; paired mode expects at most 2 records per id",
                            more.len()
                        ),
                    });
                }
            }
        }
        Ok((self.baseline_wins, self.candidate_wins, self.draws))
    }
}

/// MeanDiff only: one paired `(candidate - baseline)` numeric diff per
/// record. Always O(n) memory - `bootstrap_mean_diff_ci[_bca/_basic]` need
/// random access to every diff for resampling, so this can never go
/// streaming the way `OutcomeCollector`/`SignCounts` can.
pub(crate) struct DiffCollector {
    paired_by_id: bool,
    seen_ids: HashSet<String>,
    diffs: Vec<f64>,
    groups: HashMap<String, Vec<(usize, f64)>>,
}

impl DiffCollector {
    pub(crate) fn new(paired_by_id: bool) -> Self {
        Self {
            paired_by_id,
            seen_ids: HashSet::new(),
            diffs: Vec::new(),
            groups: HashMap::new(),
        }
    }

    pub(crate) fn record(
        &mut self,
        line: usize,
        id: Option<&str>,
        diff: f64,
    ) -> Result<(), VeridictError> {
        if self.paired_by_id {
            match id {
                Some(id) => {
                    self.groups
                        .entry(id.to_string())
                        .or_default()
                        .push((line, diff));
                }
                None => self.diffs.push(diff),
            }
        } else {
            // Without pairing, a repeated id is almost always a data mistake,
            // so it's rejected up front. With pairing, a repeated id is the
            // whole point - `finish` validates it there instead (exactly 2,
            // not more).
            if let Some(id) = id
                && !self.seen_ids.insert(id.to_string())
            {
                return Err(VeridictError::DuplicateId {
                    id: id.to_string(),
                    line,
                });
            }
            self.diffs.push(diff);
        }
        Ok(())
    }

    pub(crate) fn finish(mut self) -> Result<Vec<f64>, VeridictError> {
        // `HashMap` iteration order is unspecified; without sorting, which
        // underlying diff value lands at a given index (and so gets drawn by
        // the seeded bootstrap RNG) would vary between process runs even
        // with a fixed --seed, silently breaking the "same input + same
        // seed = bit-identical output" guarantee for paired mean-diff.
        let mut groups: Vec<_> = self.groups.drain().collect();
        groups.sort_by_key(|(_, g)| g.iter().map(|(line, _)| *line).min().unwrap());
        for (id, group) in groups {
            match group.as_slice() {
                [(_, d)] => self.diffs.push(*d),
                [(_, a), (_, b)] => self.diffs.push((a + b) / 2.0),
                more => {
                    return Err(VeridictError::SchemaMismatch {
                        line: more[0].0,
                        context: "paired-by-id",
                        detail: format!(
                            "id '{id}' appears {} times; paired mode expects at most 2 records per id",
                            more.len()
                        ),
                    });
                }
            }
        }
        Ok(self.diffs)
    }
}

/// SignTest only: unlike `DiffCollector`, never retains a raw diff value
/// once its sign is resolved - SignTest's own math only ever needs
/// positive/negative counts (see `metrics::sign_test`), so there's no
/// reason to pay `DiffCollector`'s O(n) `Vec<f64>` cost. Still O(distinct
/// ids), not O(1): `seen_ids` (duplicate rejection, unpaired mode) and
/// `groups` (unresolved pairs, paired mode) both scale with distinct ids
/// seen, for the same reasons `DiffCollector` needs them - this removes the
/// values buffer, not the id-bookkeeping floor.
pub(crate) struct SignCounts {
    paired_by_id: bool,
    seen_ids: HashSet<String>,
    positive: u64,
    negative: u64,
    groups: HashMap<String, Vec<(usize, f64)>>,
}

impl SignCounts {
    pub(crate) fn new(paired_by_id: bool) -> Self {
        Self {
            paired_by_id,
            seen_ids: HashSet::new(),
            positive: 0,
            negative: 0,
            groups: HashMap::new(),
        }
    }

    pub(crate) fn record(
        &mut self,
        line: usize,
        id: Option<&str>,
        diff: f64,
    ) -> Result<(), VeridictError> {
        if self.paired_by_id {
            match id {
                Some(id) => {
                    self.groups
                        .entry(id.to_string())
                        .or_default()
                        .push((line, diff));
                }
                None => self.tally(diff),
            }
        } else {
            if let Some(id) = id
                && !self.seen_ids.insert(id.to_string())
            {
                return Err(VeridictError::DuplicateId {
                    id: id.to_string(),
                    line,
                });
            }
            self.tally(diff);
        }
        Ok(())
    }

    fn tally(&mut self, diff: f64) {
        if diff > 0.0 {
            self.positive += 1;
        } else if diff < 0.0 {
            self.negative += 1;
        }
        // == 0.0 (tie) counts toward neither, matching sign-test's existing
        // "ties excluded from n" convention.
    }

    /// Returns `(positive, negative)`. Unlike `DiffCollector::finish`, no
    /// sort-by-line-number is needed before resolving buffered pairs: the
    /// result is a commutative sum of +1/-1 tallies, not index positions fed
    /// to a seeded RNG, so `HashMap`'s unspecified drain order can't affect
    /// the output.
    pub(crate) fn finish(mut self) -> Result<(u64, u64), VeridictError> {
        for (id, group) in std::mem::take(&mut self.groups) {
            match group.as_slice() {
                [(_, d)] => self.tally(*d),
                [(_, a), (_, b)] => self.tally((a + b) / 2.0),
                more => {
                    return Err(VeridictError::SchemaMismatch {
                        line: more[0].0,
                        context: "paired-by-id",
                        detail: format!(
                            "id '{id}' appears {} times; paired mode expects at most 2 records per id",
                            more.len()
                        ),
                    });
                }
            }
        }
        Ok((self.positive, self.negative))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_baselines_has_zero_positive_count_and_no_robust_span() {
        let diag = compute_scale_diagnostics(&[]);
        assert_eq!(diag.positive_baseline_count, 0);
        assert_eq!(diag.non_positive_baseline_count, 0);
        assert_eq!(diag.raw_orders_of_magnitude, 0.0);
        assert!(diag.robust_orders_of_magnitude.is_none());
    }

    #[test]
    fn all_non_positive_baselines_has_zero_positive_count_and_no_span() {
        let diag = compute_scale_diagnostics(&[0.0, -5.0, -10.0]);
        assert_eq!(diag.positive_baseline_count, 0);
        assert_eq!(diag.non_positive_baseline_count, 3);
        assert_eq!(diag.raw_orders_of_magnitude, 0.0);
    }

    #[test]
    fn raw_orders_of_magnitude_matches_log10_of_max_over_min() {
        let diag = compute_scale_diagnostics(&[10.0, 100.0, 1_000.0]);
        assert!((diag.raw_orders_of_magnitude - 2.0).abs() < 1e-9); // log10(1000/10)
        assert_eq!(diag.positive_baseline_count, 3);
        assert_eq!(diag.non_positive_baseline_count, 0);
        assert_eq!(diag.min_positive_baseline, 10.0);
        assert_eq!(diag.max_positive_baseline, 1_000.0);
    }

    #[test]
    fn mixed_sign_baselines_count_positives_and_non_positives_separately() {
        let diag = compute_scale_diagnostics(&[10.0, -5.0, 0.0, 1_000.0]);
        assert_eq!(diag.positive_baseline_count, 2);
        assert_eq!(diag.non_positive_baseline_count, 2);
    }

    #[test]
    fn robust_span_is_none_below_the_20_positive_baseline_floor() {
        let baselines: Vec<f64> = (1..=19).map(|i| i as f64).collect();
        let diag = compute_scale_diagnostics(&baselines);
        assert_eq!(diag.positive_baseline_count, 19);
        assert!(diag.robust_orders_of_magnitude.is_none());
    }

    #[test]
    fn robust_span_appears_at_the_20_positive_baseline_floor() {
        let baselines: Vec<f64> = (1..=20).map(|i| i as f64).collect();
        let diag = compute_scale_diagnostics(&baselines);
        assert_eq!(diag.positive_baseline_count, 20);
        assert!(diag.robust_orders_of_magnitude.is_some());
    }

    #[test]
    fn a_single_extreme_outlier_does_not_move_the_robust_span_at_n40() {
        // 39 baselines clustered in [100, 138], plus one wild outlier at 10,000,000 - the outlier
        // is the sample max, but at n=40 type-7's p95 (h = 0.95*39 = 37.05) interpolates between
        // sorted indices 37/38 (values 137/138), never touching index 39 (the outlier) - so it
        // never enters the p95 estimate. n=20 would NOT have this property (h = 0.95*19 = 18.05,
        // which does land on the max) - this is why the robust-span floor matters, not just its
        // presence/absence.
        let mut baselines: Vec<f64> = (0..39).map(|i| 100.0 + i as f64).collect();
        baselines.push(10_000_000.0);
        let diag = compute_scale_diagnostics(&baselines);
        assert_eq!(diag.positive_baseline_count, 40);
        let robust = diag.robust_orders_of_magnitude.unwrap();
        assert!(
            robust < 1.0,
            "robust span {robust} was pulled up by the single outlier"
        );
        // The raw (non-robust) span, by contrast, DOES include the outlier and is far wider -
        // confirming the outlier is real data, just correctly excluded from the robust summary.
        assert!(diag.raw_orders_of_magnitude > 4.0);
    }

    #[test]
    fn result_does_not_depend_on_input_order() {
        let a = compute_scale_diagnostics(&[5.0, 500.0, 50.0]);
        let b = compute_scale_diagnostics(&[50.0, 5.0, 500.0]);
        assert_eq!(a.raw_orders_of_magnitude, b.raw_orders_of_magnitude);
        assert_eq!(a.min_positive_baseline, b.min_positive_baseline);
        assert_eq!(a.max_positive_baseline, b.max_positive_baseline);
    }
}
