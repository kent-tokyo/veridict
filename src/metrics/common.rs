//! Shared per-record collectors used by the metric aggregators: gather one
//! observation per record, then (if `paired_by_id`) reduce same-id pairs
//! into a single net observation at the end. Kept out of the per-metric
//! files since both WinRate/Elo and MeanDiff/SignTest reuse the same
//! collection shape.

use std::collections::{HashMap, HashSet};

use crate::Outcome;
use crate::error::VeridictError;

/// Shared by WinRate and Elo: one win/loss/draw observation per record.
/// Order-independent (only integer tallies come out), so no ordering
/// concern the way `DiffCollector` has.
pub(crate) struct OutcomeCollector<'a> {
    paired_by_id: bool,
    baseline_wins: u64,
    candidate_wins: u64,
    draws: u64,
    groups: HashMap<&'a str, Vec<(usize, Outcome)>>,
}

impl<'a> OutcomeCollector<'a> {
    pub(crate) fn new(paired_by_id: bool) -> Self {
        Self {
            paired_by_id,
            baseline_wins: 0,
            candidate_wins: 0,
            draws: 0,
            groups: HashMap::new(),
        }
    }

    pub(crate) fn record(&mut self, line: usize, id: Option<&'a str>, outcome: Outcome) {
        match (self.paired_by_id, id) {
            (true, Some(id)) => {
                self.groups.entry(id).or_default().push((line, outcome));
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

/// Shared by MeanDiff and SignTest: one paired `(candidate - baseline)`
/// numeric diff per record.
pub(crate) struct DiffCollector<'a> {
    paired_by_id: bool,
    seen_ids: HashSet<&'a str>,
    diffs: Vec<f64>,
    groups: HashMap<&'a str, Vec<(usize, f64)>>,
}

impl<'a> DiffCollector<'a> {
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
        id: Option<&'a str>,
        diff: f64,
    ) -> Result<(), VeridictError> {
        if self.paired_by_id {
            match id {
                Some(id) => {
                    self.groups.entry(id).or_default().push((line, diff));
                }
                None => self.diffs.push(diff),
            }
        } else {
            // Without pairing, a repeated id is almost always a data mistake,
            // so it's rejected up front. With pairing, a repeated id is the
            // whole point - `finish` validates it there instead (exactly 2,
            // not more).
            if let Some(id) = id
                && !self.seen_ids.insert(id)
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
