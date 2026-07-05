//! Comparison matrix across more than two candidates.
//!
//! Two ways to feed data in, freely combinable in one run:
//! - **Legacy (`files`)**: one file per candidate, each measured against the
//!   same shared baseline, using the baseline/candidate `Record` schema -
//!   this is a "star" topology (every candidate only plays the baseline,
//!   never each other).
//! - **`--matches`**: head-to-head records between named competitors
//!   (`input::MatchRecord`), which can connect candidates directly to each
//!   other, not just to baseline.
//!
//! **If the resulting graph is still topologically a star** (every edge
//! touches "baseline", whether that edge came from a legacy file or a
//! `--matches` record), this module uses the existing closed-form shortcut:
//! each candidate's rating is exactly its own win-rate-vs-baseline
//! (`elo_from_score`) - no iterative solver needed, and every pre-existing
//! field's value is unchanged from prior behavior when only legacy `files`
//! are given (the JSON gains new additive fields like `status`/`component`,
//! but nothing old changes). This is not just a
//! performance shortcut: `star_graph_elo_matches_bradley_terry_mle_reduction`
//! below proves the general solver converges to the exact same number on
//! star data, so routing star-shaped input through the closed form changes
//! nothing about correctness - it only avoids an unnecessary iteration, and
//! (more importantly) it preserves this project's existing shutout
//! guarantee: a clean 5-0 sweep is not *strongly* connected in general-graph
//! terms (no return edge), so routing it through the general solver instead
//! would turn a large, finite, useful Elo into "disconnected, no rating" -
//! a real regression against an existing, required edge case
//! (`stats::elo::shutout_scores_clamp_to_a_finite_magnitude`). Only genuinely
//! mixed data (at least one candidate-vs-candidate edge) invokes the general
//! solver.
//!
//! **In general-graph mode**, ratings come from `stats::bradley_terry`'s
//! Zermelo/Hunter MM fixed point, fit independently per strongly connected
//! component (see that module's docs for why: some subset of competitors
//! never conceding a point to the rest means no finite MLE exists between
//! components, not merely an uncertain one). Each `MatrixEntry` is marked
//! `direct` (a real head-to-head edge exists), `inferred` (both rated, same
//! component, but never played - a model-extrapolated `elo_i - elo_j`), or
//! `disconnected` (different components - no finite comparison exists, so
//! `elo_diff`/`ci_low`/`ci_high` are `None`, not a fabricated number).
//!
//! **`MatrixEntry.ci_low`/`ci_high` get a real bootstrap percentile CI** in
//! general-graph mode (`stats::bradley_terry::bootstrap_pairwise_elo_diff_cis`;
//! see that function's docs for the resampling design and why the
//! degrade-to-`None` threshold is deliberately conservative). A `Direct`/
//! `Inferred` cell can therefore still show `ci_low`/`ci_high: None` even
//! though `elo_diff` is `Some` - that combination means the two competitors
//! are connected in the observed data, but the connection is too fragile
//! under resampling (fewer than 90% of bootstrap resamples kept them in the
//! same component) for a reliable interval; it's a distinct case from
//! `Disconnected`, which has no `elo_diff` at all.
//!
//! **`CandidateSummary.ci_low`/`ci_high` stay `None` in general-graph mode**,
//! deliberately, even though `MatrixEntry` now has real CIs: an individual
//! `elo_i` is only meaningful relative to its component's arbitrary
//! lowest-index pin, so a CI on it would mean "CI of advantage over an
//! unnamed reference competitor" - worse than `None`, not more informative.
//! `elo_i - elo_j` (what `MatrixEntry` reports) has no such problem, since
//! it's pin-invariant by construction - that asymmetry is why one struct
//! gets real CIs this sprint and the other still doesn't. Star-graph
//! entries are unaffected either way and keep their existing real Wilson
//! CIs on both structs.

use std::collections::{HashMap, HashSet};

use serde::Serialize;

use crate::error::VeridictError;
use crate::input::{MatchRecord, Record};
use crate::stats::bradley_terry::{self, PairRecord};
use crate::stats::{elo as elo_math, wilson};
use crate::{BootstrapMethod, CiMethod, MatchOutcome, MetricKind, metrics};

const BASELINE: &str = "baseline";

#[derive(Debug, Serialize)]
pub struct CandidateSummary {
    pub name: String,
    pub elo: f64,
    /// `Some` in star-graph mode (a real Wilson CI, as before). `None` in
    /// general-graph mode: a real joint CI for a jointly-fit rating is
    /// meaningfully harder than the star-graph case and is deliberately not
    /// approximated here - see module docs.
    pub ci_low: Option<f64>,
    pub ci_high: Option<f64>,
    pub baseline_count: u64,
    pub candidate_count: u64,
    pub paired_count: u64,
    pub timeouts: u64,
    pub crashes: u64,
    pub invalid: u64,
    /// Which mutually-comparable group this candidate belongs to. Always
    /// `0` in star-graph mode (a single implicit component via baseline).
    /// In general-graph mode, two candidates in different components have
    /// no finite Bradley-Terry comparison - see module docs.
    pub component: u32,
}

/// Whether a `MatrixEntry` reflects real head-to-head data, a same-
/// component model extrapolation, or no comparison at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CellStatus {
    Direct,
    Inferred,
    Disconnected,
}

#[derive(Debug, Serialize)]
pub struct MatrixEntry {
    pub row: String,
    pub col: String,
    /// Row's Elo advantage over col; the reverse direction is `-elo_diff`.
    /// `None` only for `Disconnected` cells (no finite estimate exists).
    pub elo_diff: Option<f64>,
    pub ci_low: Option<f64>,
    pub ci_high: Option<f64>,
    /// `true` iff `status == Direct`. Kept alongside `status` for backward
    /// compatibility with star-graph-only consumers.
    pub direct: bool,
    pub status: CellStatus,
}

#[derive(Debug, Serialize)]
pub struct ComparisonMatrix {
    pub schema_version: u32,
    pub confidence: f64,
    pub candidates: Vec<CandidateSummary>,
    /// Upper triangle only, "baseline" first (if present) then `candidates`
    /// in input order; `to_markdown` mirrors it into a full grid.
    pub matrix: Vec<MatrixEntry>,
}

/// Merges `(x_wins, y_wins, draws)` for the pair `{x, y}` into `edges`,
/// keyed by the pair's two names in alphabetical order so a pair seen from
/// either a legacy file or a `--matches` record (in either order) always
/// lands in the same bucket.
fn add_wins(
    edges: &mut HashMap<(String, String), (u64, u64, u64)>,
    x: &str,
    x_wins: u64,
    y: &str,
    y_wins: u64,
    draws: u64,
) {
    let (lo, hi, lo_wins, hi_wins) = if x <= y {
        (x, y, x_wins, y_wins)
    } else {
        (y, x, y_wins, x_wins)
    };
    let entry = edges
        .entry((lo.to_string(), hi.to_string()))
        .or_insert((0, 0, 0));
    entry.0 += lo_wins;
    entry.1 += hi_wins;
    entry.2 += draws;
}

fn canonical_pair(a: &str, b: &str) -> (String, String) {
    if a <= b {
        (a.to_string(), b.to_string())
    } else {
        (b.to_string(), a.to_string())
    }
}

/// Tallies `--matches` records into `edges`. `--paired-by-id` nets two
/// same-id records into one observation, scoped per competitor pair (an id
/// shared across two *different* pairs does not net across them) - grouping
/// key is `(canonical pair, id)`, mirroring `metrics::common::OutcomeCollector`'s
/// netting rule (majority of the two records' points decides the net
/// outcome) but applied per pair instead of globally.
fn tally_matches<J, K>(
    matches: J,
    paired_by_id: bool,
    edges: &mut HashMap<(String, String), (u64, u64, u64)>,
    match_order: &mut Vec<String>,
    seen: &mut HashSet<String>,
) -> Result<(), VeridictError>
where
    J: IntoIterator<Item = Result<K, VeridictError>>,
    K: IntoIterator<Item = Result<(usize, MatchRecord), VeridictError>>,
{
    let mut groups: HashMap<(String, String, String), Vec<(usize, f64)>> = HashMap::new();
    for file in matches {
        for item in file? {
            let (line, rec) = item?;
            if rec.a.is_empty() || rec.b.is_empty() {
                return Err(VeridictError::SchemaMismatch {
                    line,
                    context: "matrix graph",
                    detail: "a and b must both be non-empty competitor names".to_string(),
                });
            }
            if rec.a == rec.b {
                return Err(VeridictError::SchemaMismatch {
                    line,
                    context: "matrix graph",
                    detail: format!(
                        "a and b are both '{}'; a competitor can't play itself",
                        rec.a
                    ),
                });
            }
            for name in [&rec.a, &rec.b] {
                if name != BASELINE && seen.insert(name.clone()) {
                    match_order.push(name.clone());
                }
            }
            let outcome = MatchOutcome::parse(&rec.result).ok_or_else(|| {
                VeridictError::UnrecognizedOutcome {
                    line,
                    value: rec.result.clone(),
                    expected: "a_win|b_win|draw",
                }
            })?;
            let (lo, hi, points_to_hi) = canonicalize_outcome(&rec.a, &rec.b, outcome);
            match (paired_by_id, rec.id.as_deref()) {
                (true, Some(id)) => {
                    groups
                        .entry((lo, hi, id.to_string()))
                        .or_default()
                        .push((line, points_to_hi));
                }
                _ => tally_points(edges, lo, hi, points_to_hi),
            }
        }
    }
    for ((lo, hi, id), group) in groups {
        match group.as_slice() {
            [(_, p)] => tally_points(edges, lo, hi, *p),
            [(_, a), (_, b)] => {
                let total = a + b;
                #[allow(clippy::float_cmp)]
                let net = if total > 1.0 {
                    1.0
                } else if total < 1.0 {
                    0.0
                } else {
                    0.5
                };
                tally_points(edges, lo, hi, net);
            }
            more => {
                return Err(VeridictError::SchemaMismatch {
                    line: more[0].0,
                    context: "paired-by-id",
                    detail: format!(
                        "id '{id}' appears {} times for pair ({lo}, {hi}); paired mode expects at most 2 records per id per pair",
                        more.len()
                    ),
                });
            }
        }
    }
    Ok(())
}

/// Resolves `outcome` (relative to `a`/`b` as given) into `(lo, hi,
/// points_to_hi)`, `lo`/`hi` being `a`/`b` in alphabetical order and
/// `points_to_hi` the fraction of the point `hi` won (1.0/0.5/0.0).
fn canonicalize_outcome(a: &str, b: &str, outcome: MatchOutcome) -> (String, String, f64) {
    let points_a = match outcome {
        MatchOutcome::AWin => 1.0,
        MatchOutcome::Draw => 0.5,
        MatchOutcome::BWin => 0.0,
    };
    if a <= b {
        (a.to_string(), b.to_string(), 1.0 - points_a)
    } else {
        (b.to_string(), a.to_string(), points_a)
    }
}

fn tally_points(
    edges: &mut HashMap<(String, String), (u64, u64, u64)>,
    lo: String,
    hi: String,
    points_to_hi: f64,
) {
    let entry = edges.entry((lo, hi)).or_insert((0, 0, 0));
    if points_to_hi > 0.5 {
        entry.1 += 1;
    } else if points_to_hi < 0.5 {
        entry.0 += 1;
    } else {
        entry.2 += 1;
    }
}

/// A candidate's own (wins, draws, n) aggregated across every edge it
/// participates in - used for general-graph mode's per-candidate CI margin
/// and count fields, where "vs baseline only" no longer applies once a
/// candidate can also play other candidates directly.
fn node_totals(name: &str, edges: &HashMap<(String, String), (u64, u64, u64)>) -> (u64, u64, u64) {
    let mut wins = 0u64;
    let mut draws = 0u64;
    let mut n = 0u64;
    for ((lo, hi), &(lo_wins, hi_wins, pair_draws)) in edges {
        if lo == name {
            wins += lo_wins;
            draws += pair_draws;
            n += lo_wins + hi_wins + pair_draws;
        } else if hi == name {
            wins += hi_wins;
            draws += pair_draws;
            n += lo_wins + hi_wins + pair_draws;
        }
    }
    (wins, draws, n)
}

/// Elo point estimate and Wilson-derived CI directly from aggregate counts
/// (no `metrics::compute` involved) - used for star-graph candidates that
/// only appear via `--matches` (no legacy file of their own).
fn elo_ci_from_counts(
    wins: u64,
    draws: u64,
    n: u64,
    confidence: f64,
) -> Result<(f64, f64, f64), VeridictError> {
    if n == 0 {
        return Ok((0.0, 0.0, 0.0));
    }
    let score = (wins as f64 + 0.5 * draws as f64) / n as f64;
    let (lo, hi) = wilson::wilson_ci_from_proportion(score, n as f64, confidence)?;
    Ok((
        elo_math::elo_from_score(score),
        elo_math::elo_from_score(lo),
        elo_math::elo_from_score(hi),
    ))
}

/// `named_records` and `matches` are both streaming: only one legacy
/// candidate file is read at a time (as before), and `--matches` files are
/// read lazily too - but building the general-graph solver's input (unlike
/// the star-graph path) requires holding one small aggregate per distinct
/// pair, not the raw records themselves, so memory there scales with the
/// number of distinct competitor pairs, not total match record count.
#[allow(clippy::too_many_arguments)]
pub fn run<I, R, J, K>(
    named_records: I,
    matches: J,
    confidence: f64,
    paired_by_id: bool,
    resamples: usize,
    seed: u64,
    bootstrap_method: BootstrapMethod,
) -> Result<ComparisonMatrix, VeridictError>
where
    I: IntoIterator<Item = Result<(String, R), VeridictError>>,
    R: IntoIterator<Item = Result<(usize, Record), VeridictError>>,
    J: IntoIterator<Item = Result<K, VeridictError>>,
    K: IntoIterator<Item = Result<(usize, MatchRecord), VeridictError>>,
{
    let mut legacy_order = Vec::new();
    let mut legacy_summaries: HashMap<String, CandidateSummary> = HashMap::new();
    let mut edges: HashMap<(String, String), (u64, u64, u64)> = HashMap::new();

    for item in named_records {
        let (name, records) = item?;
        // resamples/seed/bootstrap_method are mean-diff-only knobs; the elo
        // metric ignores them. ci_method: elo never supports CiMethod::Exact
        // (fractional p_hat), so this is always Wilson.
        let out = metrics::compute(
            records,
            MetricKind::Elo,
            confidence,
            1,
            0,
            paired_by_id,
            CiMethod::Wilson,
            BootstrapMethod::Percentile,
        )?;
        let draws = out
            .paired_count
            .saturating_sub(out.baseline_count)
            .saturating_sub(out.candidate_count);
        add_wins(
            &mut edges,
            BASELINE,
            out.baseline_count,
            &name,
            out.candidate_count,
            draws,
        );
        legacy_summaries.insert(
            name.clone(),
            CandidateSummary {
                name: name.clone(),
                elo: out.effect,
                ci_low: Some(out.ci_low),
                ci_high: Some(out.ci_high),
                baseline_count: out.baseline_count,
                candidate_count: out.candidate_count,
                paired_count: out.paired_count,
                timeouts: out.timeouts,
                crashes: out.crashes,
                invalid: out.invalid,
                component: 0,
            },
        );
        legacy_order.push(name);
    }

    let mut match_order: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = legacy_order.iter().cloned().collect();
    seen.insert(BASELINE.to_string());
    tally_matches(
        matches,
        paired_by_id,
        &mut edges,
        &mut match_order,
        &mut seen,
    )?;

    let candidate_order: Vec<String> = legacy_order.into_iter().chain(match_order).collect();
    if candidate_order.is_empty() {
        return Err(VeridictError::EmptyInput);
    }

    let has_baseline = edges.keys().any(|(a, b)| a == BASELINE || b == BASELINE);
    let star_shaped = edges.keys().all(|(a, b)| a == BASELINE || b == BASELINE);

    let (candidates, matrix) = if star_shaped {
        build_star_graph(&candidate_order, &legacy_summaries, &edges, confidence)?
    } else {
        build_general_graph(
            &candidate_order,
            has_baseline,
            &edges,
            resamples,
            seed,
            confidence,
            bootstrap_method,
        )?
    };

    Ok(ComparisonMatrix {
        schema_version: crate::report::REPORT_SCHEMA_VERSION,
        confidence,
        candidates,
        matrix,
    })
}

fn build_star_graph(
    candidate_order: &[String],
    legacy_summaries: &HashMap<String, CandidateSummary>,
    edges: &HashMap<(String, String), (u64, u64, u64)>,
    confidence: f64,
) -> Result<(Vec<CandidateSummary>, Vec<MatrixEntry>), VeridictError> {
    let mut candidates = Vec::with_capacity(candidate_order.len());
    for name in candidate_order {
        if let Some(summary) = legacy_summaries.get(name) {
            candidates.push(CandidateSummary {
                name: summary.name.clone(),
                elo: summary.elo,
                ci_low: summary.ci_low,
                ci_high: summary.ci_high,
                baseline_count: summary.baseline_count,
                candidate_count: summary.candidate_count,
                paired_count: summary.paired_count,
                timeouts: summary.timeouts,
                crashes: summary.crashes,
                invalid: summary.invalid,
                component: 0,
            });
        } else {
            let key = canonical_pair(BASELINE, name);
            let (lo_wins, hi_wins, draws) = edges[&key];
            let (baseline_wins, candidate_wins) = if key.0 == BASELINE {
                (lo_wins, hi_wins)
            } else {
                (hi_wins, lo_wins)
            };
            let n = baseline_wins + candidate_wins + draws;
            let (elo, ci_low, ci_high) = elo_ci_from_counts(candidate_wins, draws, n, confidence)?;
            candidates.push(CandidateSummary {
                name: name.clone(),
                elo,
                ci_low: Some(ci_low),
                ci_high: Some(ci_high),
                baseline_count: baseline_wins,
                candidate_count: candidate_wins,
                paired_count: n,
                timeouts: 0,
                crashes: 0,
                invalid: 0,
                component: 0,
            });
        }
    }

    // Every candidate built above (both branches) always sets Some(..) for
    // ci_low/ci_high - only general-graph mode ever leaves them None - so
    // these unwraps can't panic in this (star-graph-only) function.
    let mut matrix = Vec::new();
    for c in &candidates {
        let ci_low = c.ci_low.expect("star-graph candidates always have a CI");
        let ci_high = c.ci_high.expect("star-graph candidates always have a CI");
        matrix.push(MatrixEntry {
            row: BASELINE.to_string(),
            col: c.name.clone(),
            elo_diff: Some(-c.elo),
            ci_low: Some(-ci_high),
            ci_high: Some(-ci_low),
            direct: true,
            status: CellStatus::Direct,
        });
    }
    for i in 0..candidates.len() {
        for j in (i + 1)..candidates.len() {
            let (a, b) = (&candidates[i], &candidates[j]);
            let margin_a = (a.ci_high.unwrap() - a.ci_low.unwrap()) / 2.0;
            let margin_b = (b.ci_high.unwrap() - b.ci_low.unwrap()) / 2.0;
            let margin = margin_a.hypot(margin_b);
            let diff = a.elo - b.elo;
            matrix.push(MatrixEntry {
                row: a.name.clone(),
                col: b.name.clone(),
                elo_diff: Some(diff),
                ci_low: Some(diff - margin),
                ci_high: Some(diff + margin),
                direct: false,
                status: CellStatus::Inferred,
            });
        }
    }

    Ok((candidates, matrix))
}

#[allow(clippy::too_many_arguments)]
fn build_general_graph(
    candidate_order: &[String],
    has_baseline: bool,
    edges: &HashMap<(String, String), (u64, u64, u64)>,
    resamples: usize,
    seed: u64,
    confidence: f64,
    bootstrap_method: BootstrapMethod,
) -> Result<(Vec<CandidateSummary>, Vec<MatrixEntry>), VeridictError> {
    // Baseline, if present, always gets index 0 - `bradley_terry::fit_graph`
    // pins each component's lowest-index member, so this makes baseline the
    // pin (elo 0.0) of whatever component it belongs to, exactly matching
    // today's "elo is relative to baseline" convention.
    let mut all_nodes: Vec<String> = Vec::new();
    if has_baseline {
        all_nodes.push(BASELINE.to_string());
    }
    all_nodes.extend(candidate_order.iter().cloned());
    let index_of: HashMap<&str, usize> = all_nodes
        .iter()
        .enumerate()
        .map(|(i, n)| (n.as_str(), i))
        .collect();

    // Sorted by node index, not left in `edges`'s HashMap iteration order:
    // `bradley_terry::fit_graph`'s component labels and float summation
    // order both depend on pair order, so an unsorted `Vec` here would make
    // `component` ids and last-ULP elo digits vary between runs on the same
    // input - silently breaking reproducibility (and any golden-file test).
    let mut pairs: Vec<PairRecord> = edges
        .iter()
        .map(|((lo, hi), &(lo_wins, hi_wins, draws))| PairRecord {
            a: index_of[lo.as_str()],
            b: index_of[hi.as_str()],
            a_wins: lo_wins,
            b_wins: hi_wins,
            draws,
        })
        .collect();
    pairs.sort_by_key(|p| (p.a, p.b));

    let fits = bradley_terry::fit_graph(all_nodes.len(), &pairs)?;
    let cis = bradley_terry::bootstrap_pairwise_elo_diff_cis(
        all_nodes.len(),
        &pairs,
        &fits,
        resamples,
        seed,
        confidence,
        bootstrap_method,
    );

    // baseline_count/candidate_count generalize to "this node's total
    // losses/wins" summed across every edge it participates in, not just vs
    // baseline (see module docs) - since a general-graph candidate can play
    // opponents other than baseline.
    let mut candidates = Vec::with_capacity(candidate_order.len());
    for name in candidate_order {
        let idx = index_of[name.as_str()];
        let fit = fits[idx];
        let (wins, draws, n) = node_totals(name, edges);
        candidates.push(CandidateSummary {
            name: name.clone(),
            elo: fit.elo,
            // Deliberately None, not approximated: see module docs. A
            // per-candidate Wilson margin on local win rate was considered
            // and rejected - it would silently ship the exact heuristic the
            // CI-methodology decision rejected, just relocated to a field
            // whose type happened to make that easy. Unlike `MatrixEntry`'s
            // `elo_diff` (pin-invariant, so it gets a real bootstrap CI
            // below), an individual `elo_i` is only meaningful relative to
            // an arbitrary per-component pin.
            ci_low: None,
            ci_high: None,
            baseline_count: n.saturating_sub(wins).saturating_sub(draws),
            candidate_count: wins,
            paired_count: n,
            timeouts: 0,
            crashes: 0,
            invalid: 0,
            component: fit.component as u32,
        });
    }

    let mut matrix = Vec::new();
    for i in 0..all_nodes.len() {
        for j in (i + 1)..all_nodes.len() {
            let (row, col) = (&all_nodes[i], &all_nodes[j]);
            let same_component = fits[i].component == fits[j].component;
            let direct_edge = edges.contains_key(&canonical_pair(row, col));
            let status = if !same_component {
                CellStatus::Disconnected
            } else if direct_edge {
                CellStatus::Direct
            } else {
                CellStatus::Inferred
            };
            let elo_diff = same_component.then(|| fits[i].elo - fits[j].elo);
            let ci = cis.get(&(i, j)).copied();
            matrix.push(MatrixEntry {
                row: row.clone(),
                col: col.clone(),
                elo_diff,
                ci_low: ci.map(|(lo, _)| lo),
                ci_high: ci.map(|(_, hi)| hi),
                direct: status == CellStatus::Direct,
                status,
            });
        }
    }

    Ok((candidates, matrix))
}

impl ComparisonMatrix {
    pub fn to_json_pretty(&self) -> String {
        serde_json::to_string_pretty(self).expect(
            "ComparisonMatrix contains only finite fields and strings; serialization cannot fail",
        )
    }

    pub fn to_markdown(&self) -> String {
        let mut names = Vec::new();
        if self
            .matrix
            .iter()
            .any(|e| e.row == BASELINE || e.col == BASELINE)
        {
            names.push(BASELINE.to_string());
        }
        for c in &self.candidates {
            if !names.contains(&c.name) {
                names.push(c.name.clone());
            }
        }

        let lookup = |row: &str, col: &str| -> Option<(Option<f64>, CellStatus)> {
            self.matrix.iter().find_map(|e| {
                if e.row == row && e.col == col {
                    Some((e.elo_diff, e.status))
                } else if e.row == col && e.col == row {
                    Some((e.elo_diff.map(|v| -v), e.status))
                } else {
                    None
                }
            })
        };

        let num_components: HashSet<u32> = self.candidates.iter().map(|c| c.component).collect();

        let mut out = String::from("# Veridict Comparison Matrix\n\n");
        out.push_str(
            "Cell = row's Elo advantage over column. `*` marks an inferred cell (no direct \
             head-to-head data, estimated transitively through the graph). `n/a` marks \
             disconnected candidates (no path between them; not comparable).\n\n",
        );
        if num_components.len() > 1 {
            out.push_str(&format!(
                "Note: candidates span {} disconnected components; only cells within the same \
                 component have a rating comparison.\n\n",
                num_components.len()
            ));
        }
        out.push_str("| vs |");
        for n in &names {
            out.push_str(&format!(" {n} |"));
        }
        out.push('\n');
        out.push_str("|---|");
        for _ in &names {
            out.push_str("---|");
        }
        out.push('\n');
        for row in &names {
            out.push_str(&format!("| {row} |"));
            for col in &names {
                if row == col {
                    out.push_str(" - |");
                    continue;
                }
                match lookup(row, col) {
                    Some((Some(v), CellStatus::Direct)) => out.push_str(&format!(" {v:+.1} |")),
                    Some((Some(v), CellStatus::Inferred)) => out.push_str(&format!(" {v:+.1}* |")),
                    Some((None, CellStatus::Disconnected)) => out.push_str(" n/a |"),
                    _ => out.push_str(" ? |"),
                }
            }
            out.push('\n');
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stats::bootstrap::DEFAULT_SEED as TEST_SEED;

    // Small and fixed: these tests care about routing/status/connectivity
    // behavior, not bootstrap CI precision, so keep resampling cheap.
    const TEST_RESAMPLES: usize = 200;

    fn rec(result: &str) -> Record {
        Record {
            id: None,
            baseline: None,
            candidate: None,
            result: Some(result.to_string()),
            baseline_status: None,
            candidate_status: None,
        }
    }

    fn stream(result: &str, n: usize) -> Vec<(usize, Record)> {
        (0..n).map(|i| (i + 1, rec(result))).collect()
    }

    type NamedRecords =
        Result<(String, Vec<Result<(usize, Record), VeridictError>>), VeridictError>;

    fn ok_named(data: Vec<(String, Vec<(usize, Record)>)>) -> Vec<NamedRecords> {
        data.into_iter()
            .map(|(name, records)| Ok((name, records.into_iter().map(Ok).collect())))
            .collect()
    }

    type MatchFile = Vec<Result<(usize, MatchRecord), VeridictError>>;

    fn no_matches() -> Vec<Result<MatchFile, VeridictError>> {
        Vec::new()
    }

    fn match_rec(id: &str, a: &str, b: &str, result: &str) -> MatchRecord {
        MatchRecord {
            id: Some(id.to_string()),
            a: a.to_string(),
            b: b.to_string(),
            result: result.to_string(),
        }
    }

    fn ok_matches(files: Vec<Vec<MatchRecord>>) -> Vec<Result<MatchFile, VeridictError>> {
        files
            .into_iter()
            .map(|records| {
                Ok(records
                    .into_iter()
                    .enumerate()
                    .map(|(i, r)| Ok((i + 1, r)))
                    .collect())
            })
            .collect()
    }

    #[test]
    fn baseline_vs_candidate_is_direct() {
        let data = vec![("a".to_string(), stream("candidate_win", 20))];
        let m = run(
            ok_named(data),
            no_matches(),
            0.95,
            false,
            TEST_RESAMPLES,
            TEST_SEED,
            BootstrapMethod::Percentile,
        )
        .unwrap();
        assert_eq!(m.matrix.len(), 1);
        assert!(m.matrix[0].direct);
        assert_eq!(m.matrix[0].row, "baseline");
        assert_eq!(m.matrix[0].col, "a");
        assert!(m.matrix[0].elo_diff.unwrap() < 0.0); // baseline lost every game
    }

    #[test]
    fn candidate_vs_candidate_is_extrapolated() {
        let data = vec![
            ("a".to_string(), stream("candidate_win", 20)),
            ("b".to_string(), stream("baseline_win", 20)),
        ];
        let m = run(
            ok_named(data),
            no_matches(),
            0.95,
            false,
            TEST_RESAMPLES,
            TEST_SEED,
            BootstrapMethod::Percentile,
        )
        .unwrap();
        let cross = m
            .matrix
            .iter()
            .find(|e| e.row == "a" && e.col == "b")
            .unwrap();
        assert!(!cross.direct);
        assert_eq!(cross.status, CellStatus::Inferred);
        assert!(cross.elo_diff.unwrap() > 0.0); // a beat baseline, b lost to baseline
    }

    #[test]
    fn markdown_mirrors_into_a_full_grid() {
        let data = vec![("a".to_string(), stream("candidate_win", 20))];
        let md = run(
            ok_named(data),
            no_matches(),
            0.95,
            false,
            TEST_RESAMPLES,
            TEST_SEED,
            BootstrapMethod::Percentile,
        )
        .unwrap()
        .to_markdown();
        assert!(md.contains("| baseline |"));
        assert!(md.contains("| a |"));
        assert!(md.contains(" - |"));
    }

    #[test]
    fn empty_input_is_an_error() {
        assert!(matches!(
            run(
                ok_named(vec![]),
                no_matches(),
                0.95,
                false,
                TEST_RESAMPLES,
                TEST_SEED,
                BootstrapMethod::Percentile,
            ),
            Err(VeridictError::EmptyInput)
        ));
    }

    fn stream_mixed(
        candidate_wins: usize,
        draws: usize,
        baseline_wins: usize,
    ) -> Vec<(usize, Record)> {
        let mut line = 0;
        let mut records = Vec::new();
        for result in ["candidate_win"]
            .repeat(candidate_wins)
            .into_iter()
            .chain(["draw"].repeat(draws))
            .chain(["baseline_win"].repeat(baseline_wins))
        {
            line += 1;
            records.push((line, rec(result)));
        }
        records
    }

    // The concrete check behind the module doc's claim that this module's
    // per-file Elo computation already *is* the Bradley-Terry MLE for a
    // star-graph topology, not merely a documented assertion: this runs the
    // *general* Bradley-Terry MM fixed-point update (Hunter 2004), not the
    // closed-form shortcut `matrix::run` actually uses, specialized to the
    // trivial 2-node graph a single candidate file represents (candidate,
    // baseline), iterated from an arbitrary starting point to convergence -
    // then confirms it lands on the same Elo `matrix::run` reports. If a
    // future change ever broke that equivalence, this test - not just the
    // doc comment - would catch it.
    #[test]
    fn star_graph_elo_matches_bradley_terry_mle_reduction() {
        let (candidate_wins, draws, baseline_wins) = (12, 3, 5);
        let w_candidate = candidate_wins as f64 + 0.5 * draws as f64;
        let w_baseline = baseline_wins as f64 + 0.5 * draws as f64;
        let n = (candidate_wins + draws + baseline_wins) as f64;

        // General MM update for a 2-node graph {baseline, candidate}, with
        // `n` total games shared between them:
        //   pi_i(new) = W_i / (n / (pi_baseline + pi_candidate))
        // Scale is unidentified (the model only fixes ratios), so pin
        // pi_baseline back to 1.0 after each step - standard normalization,
        // not specific to this test.
        let (mut pi_baseline, mut pi_candidate) = (1.0_f64, 1.0_f64);
        for _ in 0..1000 {
            let denom = n / (pi_baseline + pi_candidate);
            let new_baseline = w_baseline / denom;
            let new_candidate = w_candidate / denom;
            pi_candidate = new_candidate / new_baseline;
            pi_baseline = new_baseline / new_baseline;
        }
        let mle_elo = 400.0 * (pi_candidate / pi_baseline).log10();

        let data = vec![(
            "candidate".to_string(),
            stream_mixed(candidate_wins, draws, baseline_wins),
        )];
        let m = run(
            ok_named(data),
            no_matches(),
            0.95,
            false,
            TEST_RESAMPLES,
            TEST_SEED,
            BootstrapMethod::Percentile,
        )
        .unwrap();
        let reported_elo = m.candidates[0].elo;

        assert!(
            (mle_elo - reported_elo).abs() < 1e-6,
            "general MM fixed point {mle_elo} vs matrix::run's reported {reported_elo}"
        );
    }

    // The concrete check for the P0-4 routing decision: a clean shutout
    // (5-0, no draws) submitted entirely through `--matches` (naming
    // "baseline" explicitly, no legacy file at all) must still produce the
    // same large-finite-Elo behavior as `stats::elo`'s
    // `shutout_scores_clamp_to_a_finite_magnitude` - NOT `disconnected`. In
    // general-graph terms a one-directional sweep is not strongly
    // connected (no return edge), so if this were naively routed through
    // `bradley_terry::fit_graph` it would report the two as incomparable
    // singleton components instead. The routing check (every edge touches
    // "baseline" -> star-shaped -> closed form) must catch this even when
    // the star-shaped edge arrived via `--matches` rather than a legacy
    // file.
    #[test]
    fn shutout_via_matches_still_uses_the_closed_form_not_disconnected() {
        // Candidate wins every game (a clean sweep, no draws).
        let records: Vec<MatchRecord> = (0..5)
            .map(|_| match_rec("g", "candidate", "baseline", "a_win"))
            .collect();
        let matches = ok_matches(vec![records]);
        let m = run(
            ok_named(vec![]),
            matches,
            0.95,
            false,
            TEST_RESAMPLES,
            TEST_SEED,
            BootstrapMethod::Percentile,
        )
        .unwrap();

        assert_eq!(m.candidates.len(), 1);
        let candidate = &m.candidates[0];
        assert_eq!(candidate.component, 0);
        assert!(candidate.elo.is_finite() && candidate.elo > 0.0);

        let cell = &m.matrix[0];
        assert_eq!(cell.row, "baseline");
        assert_eq!(cell.col, "candidate");
        assert_eq!(cell.status, CellStatus::Direct);
        assert!(cell.direct);
        // Baseline's own elo_diff against candidate is large and negative
        // (baseline lost every game), not None/disconnected.
        let diff = cell.elo_diff.unwrap();
        assert!(diff.is_finite() && diff < 0.0);
    }

    #[test]
    fn matches_only_pure_head_to_head_ranks_without_a_baseline() {
        // A genuine cycle (everyone has beaten and lost to someone, so the
        // graph is strongly connected - a pure win/win/win chain with no
        // return games, unlike this, would have no finite Bradley-Terry
        // estimate at all, per stats::bradley_terry's module docs). Same
        // win counts as the independently-verified
        // multi_node_cycle_converges_to_expected_ratings test in
        // stats::bradley_terry: alpha beats beta 4-1, beta beats gamma 3-2,
        // gamma beats alpha 1-4.
        let mut records = Vec::new();
        for _ in 0..4 {
            records.push(match_rec("x", "alpha", "beta", "a_win"));
        }
        records.push(match_rec("x", "alpha", "beta", "b_win"));
        for _ in 0..3 {
            records.push(match_rec("x", "beta", "gamma", "a_win"));
        }
        for _ in 0..2 {
            records.push(match_rec("x", "beta", "gamma", "b_win"));
        }
        records.push(match_rec("x", "gamma", "alpha", "a_win"));
        for _ in 0..4 {
            records.push(match_rec("x", "gamma", "alpha", "b_win"));
        }
        let matches = ok_matches(vec![records]);
        let m = run(
            ok_named(vec![]),
            matches,
            0.95,
            false,
            TEST_RESAMPLES,
            TEST_SEED,
            BootstrapMethod::Percentile,
        )
        .unwrap();
        assert_eq!(m.candidates.len(), 3);
        assert!(!m.candidates.iter().any(|c| c.name == "baseline"));
        let elo = |name: &str| m.candidates.iter().find(|c| c.name == name).unwrap().elo;
        assert!(elo("alpha") > elo("beta"));
        assert!(elo("beta") > elo("gamma"));
        assert!(
            (elo("beta") - elo("alpha") - (-215.5356238480202)).abs() < 1e-6,
            "expected the same converged ratio as stats::bradley_terry's cycle test"
        );
    }

    #[test]
    fn mixed_legacy_and_matches_connects_candidates_directly() {
        let named = ok_named(vec![
            ("a".to_string(), stream_mixed(6, 2, 2)),
            ("b".to_string(), stream_mixed(3, 2, 5)),
        ]);
        let matches = ok_matches(vec![vec![match_rec("h2h", "a", "b", "a_win")]]);
        let m = run(
            named,
            matches,
            0.95,
            false,
            TEST_RESAMPLES,
            TEST_SEED,
            BootstrapMethod::Percentile,
        )
        .unwrap();
        let cross = m
            .matrix
            .iter()
            .find(|e| (e.row == "a" && e.col == "b") || (e.row == "b" && e.col == "a"))
            .unwrap();
        assert_eq!(cross.status, CellStatus::Direct);
        assert!(cross.direct);
        assert!(cross.elo_diff.is_some());
    }

    #[test]
    fn genuinely_disconnected_candidates_report_no_comparison() {
        // Two separate head-to-head clusters that never share a competitor.
        let matches = ok_matches(vec![vec![
            match_rec("m1", "a", "b", "a_win"),
            match_rec("m2", "c", "d", "a_win"),
        ]]);
        let m = run(
            ok_named(vec![]),
            matches,
            0.95,
            false,
            TEST_RESAMPLES,
            TEST_SEED,
            BootstrapMethod::Percentile,
        )
        .unwrap();
        let a_component = m
            .candidates
            .iter()
            .find(|c| c.name == "a")
            .unwrap()
            .component;
        let c_component = m
            .candidates
            .iter()
            .find(|c| c.name == "c")
            .unwrap()
            .component;
        assert_ne!(a_component, c_component);
        let cross = m
            .matrix
            .iter()
            .find(|e| (e.row == "a" && e.col == "c") || (e.row == "c" && e.col == "a"))
            .unwrap();
        assert_eq!(cross.status, CellStatus::Disconnected);
        assert!(cross.elo_diff.is_none());
        assert!(cross.ci_low.is_none());
        assert!(cross.ci_high.is_none());
    }

    // Regression test for `tally_matches`'s netting rule: two records with
    // the SAME id but a/b listed in OPPOSITE order (record 1: x beats y;
    // record 2, from y's perspective, y beats x) must net to a single draw
    // observation (n=1), not silently double-count as two separate games
    // (n=2) - `canonicalize_outcome` normalizes a/b to alphabetical (lo,
    // hi) order before grouping specifically so this case nets correctly.
    #[test]
    fn paired_by_id_nets_opposite_order_records_to_a_draw() {
        let matches = ok_matches(vec![vec![
            match_rec("p1", "x", "y", "a_win"),
            match_rec("p1", "y", "x", "a_win"),
        ]]);
        let m = run(
            ok_named(vec![]),
            matches,
            0.95,
            true,
            TEST_RESAMPLES,
            TEST_SEED,
            BootstrapMethod::Percentile,
        )
        .unwrap();
        let x = m.candidates.iter().find(|c| c.name == "x").unwrap();
        assert_eq!(
            x.paired_count, 1,
            "the two records should net to one game, not two"
        );
        assert_eq!(x.candidate_count, 0);
        assert_eq!(x.baseline_count, 0);
        let cross = m
            .matrix
            .iter()
            .find(|e| (e.row == "x" && e.col == "y") || (e.row == "y" && e.col == "x"))
            .unwrap();
        assert!(
            (cross.elo_diff.unwrap()).abs() < 1e-9,
            "a net draw is an even match"
        );
    }

    // Regression test for the plan's explicit scoping requirement: "an id
    // only pairs within the same {a,b} pair, not across unrelated pairs
    // sharing an id." Two records share id "shared" but name different
    // competitor pairs (x/y and z/w) - they must be treated as two
    // independent single-game observations, NOT netted together and NOT
    // rejected as "too many records for this id."
    #[test]
    fn paired_by_id_does_not_net_across_different_pairs_sharing_an_id() {
        let matches = ok_matches(vec![vec![
            match_rec("shared", "x", "y", "a_win"),
            match_rec("shared", "z", "w", "a_win"),
        ]]);
        let m = run(
            ok_named(vec![]),
            matches,
            0.95,
            true,
            TEST_RESAMPLES,
            TEST_SEED,
            BootstrapMethod::Percentile,
        )
        .unwrap();

        let x = m.candidates.iter().find(|c| c.name == "x").unwrap();
        assert_eq!(x.paired_count, 1);
        assert_eq!(x.candidate_count, 1, "x won its single game");

        let z = m.candidates.iter().find(|c| c.name == "z").unwrap();
        assert_eq!(z.paired_count, 1);
        assert_eq!(z.candidate_count, 1, "z won its single game");

        // x/y and z/w never share a competitor, so they're genuinely
        // disconnected components - proof the shared id didn't spuriously
        // link them.
        let x_component = x.component;
        let z_component = z.component;
        assert_ne!(x_component, z_component);
    }

    #[test]
    fn general_graph_direct_cells_get_real_cis_but_candidate_summary_stays_none() {
        // A robust 3-node cycle (large margins, so resampling essentially
        // never flips an edge's bidirectionality) - every cell should clear
        // the connected-fraction threshold and get a real bootstrap CI.
        let mut records = Vec::new();
        for _ in 0..15 {
            records.push(match_rec("ab", "alpha", "beta", "a_win"));
        }
        for _ in 0..5 {
            records.push(match_rec("ab", "alpha", "beta", "b_win"));
        }
        for _ in 0..15 {
            records.push(match_rec("bc", "beta", "gamma", "a_win"));
        }
        for _ in 0..5 {
            records.push(match_rec("bc", "beta", "gamma", "b_win"));
        }
        for _ in 0..15 {
            records.push(match_rec("ca", "gamma", "alpha", "a_win"));
        }
        for _ in 0..5 {
            records.push(match_rec("ca", "gamma", "alpha", "b_win"));
        }
        let matches = ok_matches(vec![records]);
        let m = run(
            ok_named(vec![]),
            matches,
            0.95,
            false,
            300,
            TEST_SEED,
            BootstrapMethod::Percentile,
        )
        .unwrap();

        assert_eq!(m.matrix.len(), 3);
        for cell in &m.matrix {
            assert_eq!(cell.status, CellStatus::Direct);
            assert!(cell.elo_diff.is_some());
            assert!(
                cell.ci_low.is_some() && cell.ci_high.is_some(),
                "{}-{} should get a real bootstrap CI",
                cell.row,
                cell.col
            );
        }
        // CandidateSummary's own CI stays None even though MatrixEntry now
        // has real ones - see module docs (pin-relative reasoning).
        for c in &m.candidates {
            assert!(c.ci_low.is_none());
            assert!(c.ci_high.is_none());
        }
    }

    #[test]
    fn fragile_connection_reports_elo_diff_but_no_ci() {
        // Two robust clusters {alpha,beta} and {gamma,delta} (tied 20-20,
        // essentially never flip under resampling) joined by a single
        // fragile bridge (beta-gamma, tied 1-1): the whole graph is one
        // component in the real fit, but only ~50% of resamples keep it
        // that way - far below the 90% connected-fraction threshold, so
        // cross-bridge cells must report `elo_diff: Some` (the real fit
        // succeeded) but `ci_low`/`ci_high: None` (too fragile under
        // resampling), while same-side cells keep a real CI.
        let mut records = vec![
            match_rec("bridge1", "beta", "gamma", "a_win"),
            match_rec("bridge2", "beta", "gamma", "b_win"),
        ];
        for _ in 0..20 {
            records.push(match_rec("ab_a", "alpha", "beta", "a_win"));
        }
        for _ in 0..20 {
            records.push(match_rec("ab_b", "alpha", "beta", "b_win"));
        }
        for _ in 0..20 {
            records.push(match_rec("cd_a", "gamma", "delta", "a_win"));
        }
        for _ in 0..20 {
            records.push(match_rec("cd_b", "gamma", "delta", "b_win"));
        }
        let matches = ok_matches(vec![records]);
        let m = run(
            ok_named(vec![]),
            matches,
            0.95,
            false,
            2000,
            TEST_SEED,
            BootstrapMethod::Percentile,
        )
        .unwrap();

        let cell = |x: &str, y: &str| {
            m.matrix
                .iter()
                .find(|e| (e.row == x && e.col == y) || (e.row == y && e.col == x))
                .unwrap()
        };

        for (x, y) in [
            ("alpha", "gamma"),
            ("alpha", "delta"),
            ("beta", "gamma"),
            ("beta", "delta"),
        ] {
            let c = cell(x, y);
            assert_ne!(
                c.status,
                CellStatus::Disconnected,
                "{x}-{y} should still be in the same original component"
            );
            assert!(c.elo_diff.is_some(), "{x}-{y} elo_diff should be Some");
            assert!(
                c.ci_low.is_none() && c.ci_high.is_none(),
                "{x}-{y} should have no reliable CI (fragile bridge)"
            );
        }
        for (x, y) in [("alpha", "beta"), ("gamma", "delta")] {
            let c = cell(x, y);
            assert!(
                c.ci_low.is_some() && c.ci_high.is_some(),
                "{x}-{y} should keep a real CI"
            );
        }
    }

    #[test]
    fn empty_competitor_name_is_rejected() {
        let matches = ok_matches(vec![vec![match_rec("m1", "", "b", "a_win")]]);
        assert!(matches!(
            run(
                ok_named(vec![]),
                matches,
                0.95,
                false,
                TEST_RESAMPLES,
                TEST_SEED,
                BootstrapMethod::Percentile,
            ),
            Err(VeridictError::SchemaMismatch { .. })
        ));
    }

    #[test]
    fn general_graph_component_ids_are_deterministic_across_runs() {
        // Regression test: the pairs fed to bradley_terry::fit_graph used to
        // come straight from a HashMap's iteration order, which is
        // per-process-random - component labels (and last-ULP elo digits)
        // would silently vary run to run on the exact same input.
        let build = || {
            let matches = ok_matches(vec![vec![
                match_rec("m1", "a", "b", "a_win"),
                match_rec("m2", "c", "d", "a_win"),
            ]]);
            run(
                ok_named(vec![]),
                matches,
                0.95,
                false,
                TEST_RESAMPLES,
                TEST_SEED,
                BootstrapMethod::Percentile,
            )
            .unwrap()
        };
        let first = build();
        let second = build();
        for name in ["a", "b", "c", "d"] {
            let comp = |m: &ComparisonMatrix| {
                m.candidates
                    .iter()
                    .find(|c| c.name == name)
                    .unwrap()
                    .component
            };
            assert_eq!(
                comp(&first),
                comp(&second),
                "component id for '{name}' differed between runs"
            );
        }
    }
}
