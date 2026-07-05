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

/// Shared by WinRate and Elo: one win/loss/draw observation per record.
/// Order-independent (only integer tallies come out), so no ordering
/// concern the way `DiffCollector` has. Without `paired_by_id`, this is
/// O(1) memory (three counters); with it, memory scales with the number of
/// distinct ids not yet resolved into a pair, not with total record count.
pub(crate) struct OutcomeCollector {
    paired_by_id: bool,
    baseline_wins: u64,
    candidate_wins: u64,
    draws: u64,
    groups: HashMap<String, Vec<(usize, Outcome)>>,
}

impl OutcomeCollector {
    pub(crate) fn new(paired_by_id: bool) -> Self {
        Self {
            paired_by_id,
            baseline_wins: 0,
            candidate_wins: 0,
            draws: 0,
            groups: HashMap::new(),
        }
    }

    pub(crate) fn record(&mut self, line: usize, id: Option<&str>, outcome: Outcome) {
        match (self.paired_by_id, id) {
            (true, Some(id)) => {
                self.groups
                    .entry(id.to_string())
                    .or_default()
                    .push((line, outcome));
            }
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

    /// Returns `(baseline_wins, candidate_wins, draws)`.
    pub(crate) fn finish(mut self) -> Result<(u64, u64, u64), VeridictError> {
        for (id, group) in std::mem::take(&mut self.groups) {
            match group.as_slice() {
                [(_, outcome)] => self.tally(*outcome),
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
