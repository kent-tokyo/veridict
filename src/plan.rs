//! `veridict plan`: given the same input `matrix` accepts, recommends which pairs would most
//! benefit from more trials to narrow their Elo-difference confidence interval to `--min-elo`
//! half-width - a ranked "what to test next" list, built directly on top of `matrix::run`'s
//! output rather than duplicating its edge-aggregation logic.
//!
//! Report-only, like `matrix`: no verdict, always succeeds once the underlying `matrix::run`
//! does. Recommendations sort disconnected/no-CI pairs first (no estimate is even possible until
//! more data connects or stabilizes them - a stronger need than any finite-but-wide CI), then the
//! rest by largest `estimated_additional_trials` first (the most-uncertain pairs).
//!
//! The trial estimate itself deliberately mirrors `verdict::estimate_additional_trials`'s
//! two-branch shape rather than reusing that function directly: `estimate_additional_trials` is
//! keyed on a verdict/threshold crossing (`Inconclusive` + `Thresholds`), which has no equivalent
//! here - `plan` has no verdict at all, just a target half-width (`--min-elo`) to narrow toward.
//! The two branches: an exact Wilson-CI binary search (same technique, real math, not an
//! approximation) for a `baseline`-vs-one-candidate cell, where the cell's CI *is* that one
//! candidate's own Wilson CI; the same `O(1/sqrt(n))` CLT-scaling fallback `mean-diff` already
//! uses for every other cell shape (candidate-vs-candidate in star-graph mode, whose CI is a
//! `hypot` of two Wilson margins; and every general-graph cell, whose CI comes from bootstrap
//! resampling) - neither has a closed-form CI-width-at-n function to search against.

use serde::Serialize;

use crate::BootstrapMethod;
use crate::error::VeridictError;
use crate::input::{MatchRecord, Record};
use crate::matrix::{self, CandidateSummary, CellStatus, ComparisonMatrix, MatrixEntry};
use crate::stats::{elo, wilson};

#[derive(Debug, Serialize)]
pub struct RecommendedMatch {
    pub row: String,
    pub col: String,
    pub status: CellStatus,
    /// `None` when no CI exists yet to narrow at all - `Disconnected`, or a fragile-bridge
    /// `Direct`/`Inferred` cell (see `matrix.rs`'s module doc) - not when the CI is merely wide.
    pub current_ci_half_width: Option<f64>,
    /// `Some(0)` when the current CI already meets `--min-elo`. `None` alongside a `note`
    /// explaining why no estimate is possible yet, for the same cells `current_ci_half_width`
    /// is `None` for.
    pub estimated_additional_trials: Option<u64>,
    pub note: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct PlanReport {
    pub schema_version: u32,
    pub min_elo: f64,
    /// Disconnected/no-CI pairs first, then most-uncertain-first
    /// (largest `estimated_additional_trials`), ties broken by (row, col) for determinism.
    pub recommendations: Vec<RecommendedMatch>,
}

#[allow(clippy::too_many_arguments)]
pub fn run<I, R, J, K>(
    named_records: I,
    matches: J,
    confidence: f64,
    paired_by_id: bool,
    resamples: usize,
    seed: u64,
    bootstrap_method: BootstrapMethod,
    min_elo: f64,
) -> Result<PlanReport, VeridictError>
where
    I: IntoIterator<Item = Result<(String, R), VeridictError>>,
    R: IntoIterator<Item = Result<(usize, Record), VeridictError>>,
    J: IntoIterator<Item = Result<K, VeridictError>>,
    K: IntoIterator<Item = Result<(usize, MatchRecord), VeridictError>>,
{
    if !min_elo.is_finite() || min_elo <= 0.0 {
        return Err(VeridictError::InvalidThreshold(
            "--min-elo must be finite and positive".to_string(),
        ));
    }

    let m = matrix::run(
        named_records,
        matches,
        confidence,
        paired_by_id,
        resamples,
        seed,
        bootstrap_method,
    )?;

    let mut recommendations: Vec<RecommendedMatch> = m
        .matrix
        .iter()
        .map(|cell| build_recommendation(cell, &m, min_elo, confidence))
        .collect();

    recommendations.sort_by(|a, b| {
        let key = |r: &RecommendedMatch| {
            (
                r.current_ci_half_width.is_some(),
                std::cmp::Reverse(r.estimated_additional_trials.unwrap_or(u64::MAX)),
                r.row.clone(),
                r.col.clone(),
            )
        };
        key(a).cmp(&key(b))
    });

    Ok(PlanReport {
        schema_version: crate::report::REPORT_SCHEMA_VERSION,
        min_elo,
        recommendations,
    })
}

fn build_recommendation(
    cell: &MatrixEntry,
    m: &ComparisonMatrix,
    min_elo: f64,
    confidence: f64,
) -> RecommendedMatch {
    let (Some(ci_low), Some(ci_high)) = (cell.ci_low, cell.ci_high) else {
        let note = if cell.status == CellStatus::Disconnected {
            "disconnected - no head-to-head path exists yet; a direct game between these two is \
             needed before any trial estimate is possible"
        } else {
            "connected in the observed data but too fragile under resampling for a reliable CI \
             yet (see matrix.rs's fragile-bridge note) - more games on this pair or its \
             connecting bridge would help"
        };
        return RecommendedMatch {
            row: cell.row.clone(),
            col: cell.col.clone(),
            status: cell.status,
            current_ci_half_width: None,
            estimated_additional_trials: None,
            note: Some(note.to_string()),
        };
    };

    let half_width = (ci_high - ci_low) / 2.0;
    if half_width <= min_elo {
        return RecommendedMatch {
            row: cell.row.clone(),
            col: cell.col.clone(),
            status: cell.status,
            current_ci_half_width: Some(half_width),
            estimated_additional_trials: Some(0),
            note: None,
        };
    }

    let find = |name: &str| m.candidates.iter().find(|c| c.name == name);
    let (row, col) = (find(&cell.row), find(&cell.col));
    let estimate = exact_wilson_pair_estimate(row, col, half_width, min_elo, confidence)
        .or_else(|| clt_pair_estimate(row, col, half_width, min_elo));

    RecommendedMatch {
        row: cell.row.clone(),
        col: cell.col.clone(),
        status: cell.status,
        current_ci_half_width: Some(half_width),
        estimated_additional_trials: estimate,
        note: None,
    }
}

/// See the module doc's "trial estimate" section. `baseline` never appears in `m.candidates` in
/// EITHER star- or general-graph mode (see `matrix::run`), so "exactly one of row/col is absent"
/// alone doesn't distinguish a star-graph cell (Wilson CI) from a general-graph baseline-vs-
/// candidate cell (bootstrap CI) - the internal recomputed-width-must-match guard below is what
/// actually confines this branch to the star-graph case, not this function's dispatch.
fn exact_wilson_pair_estimate(
    row: Option<&CandidateSummary>,
    col: Option<&CandidateSummary>,
    current_half_width: f64,
    target_half_width: f64,
    confidence: f64,
) -> Option<u64> {
    let summary = match (row, col) {
        (None, Some(c)) | (Some(c), None) => c,
        _ => return None,
    };
    let n = summary.paired_count;
    if n == 0 {
        return None;
    }
    let draws = n
        .saturating_sub(summary.candidate_count)
        .saturating_sub(summary.baseline_count);
    let p_hat = (summary.candidate_count as f64 + 0.5 * draws as f64) / n as f64;

    let half_width_at = |trial_n: u64| -> Option<f64> {
        let (lo, hi) = wilson::wilson_ci_from_proportion(p_hat, trial_n as f64, confidence).ok()?;
        Some((elo::elo_from_score(hi) - elo::elo_from_score(lo)) / 2.0)
    };
    if half_width_at(n).is_none_or(|w| (w - current_half_width).abs() > 1e-6) {
        // Two-sided: the recomputed Wilson CI at the reported n must match the cell's own
        // half-width almost exactly, not just be no wider. `baseline` never appears in
        // `m.candidates` in EITHER mode (see matrix::run), so the (None, Some(c)) pattern above
        // matches a general-graph baseline-vs-candidate cell too - one whose CI actually came
        // from `bootstrap_pairwise_elo_diff_cis`, not this candidate's own Wilson CI. A one-sided
        // "not wider" check let that case slip through here (the bootstrap CI is typically wider
        // than the naive Wilson one, so it always passed "not wider"), silently mixing two
        // different CI models in the same row and under-estimating trials versus the wider
        // interval actually shown - the wrong direction for this project. Only a genuine
        // star-graph match (recomputed width equals the shown width) may use this branch;
        // everything else falls through to `clt_pair_estimate`.
        return None;
    }

    let ratio = current_half_width / target_half_width;
    let naive_n = n as f64 * ratio * ratio;
    if !naive_n.is_finite() || naive_n >= u64::MAX as f64 {
        return Some(u64::MAX);
    }
    let mut lo_n = n;
    let mut hi_n = ((naive_n * 2.0) as u64).max(n + 1);
    if half_width_at(hi_n).is_none_or(|w| w > target_half_width) {
        return Some((naive_n as u64).saturating_sub(n));
    }
    while lo_n < hi_n {
        let mid = lo_n + (hi_n - lo_n) / 2;
        match half_width_at(mid) {
            Some(w) if w <= target_half_width => hi_n = mid,
            _ => lo_n = mid + 1,
        }
    }
    Some(lo_n.saturating_sub(n))
}

/// `O(1/sqrt(n))` CLT-scaling fallback (see `verdict::estimate_additional_trials`'s doc for this
/// model's own known bias, which applies equally here). `current_n` is the bottleneck side's own
/// trial count - narrowing a pair's CI is gated by whichever side has fewer trials; `baseline`
/// (absent from `m.candidates`) contributes nothing so the surviving side alone decides it.
fn clt_pair_estimate(
    row: Option<&CandidateSummary>,
    col: Option<&CandidateSummary>,
    current_half_width: f64,
    target_half_width: f64,
) -> Option<u64> {
    let current_n = [row, col]
        .into_iter()
        .flatten()
        .map(|c| c.paired_count)
        .min()?;
    if current_n == 0 {
        return None;
    }
    let ratio = current_half_width / target_half_width;
    let naive_n = current_n as f64 * ratio * ratio;
    if !naive_n.is_finite() {
        return None;
    }
    if naive_n >= u64::MAX as f64 {
        return Some(u64::MAX);
    }
    Some((naive_n as u64).saturating_sub(current_n))
}

impl PlanReport {
    pub fn to_json_pretty(&self) -> String {
        serde_json::to_string_pretty(self)
            .expect("PlanReport contains only finite fields and strings; serialization cannot fail")
    }

    pub fn to_markdown(&self) -> String {
        let mut out = String::from("# Veridict Plan\n\n");
        out.push_str(&format!(
            "Recommended matches to narrow toward a {:.1}-Elo confidence interval half-width, \
             most uncertain first.\n\n",
            self.min_elo
        ));
        out.push_str("| row | col | status | CI half-width | est. additional trials |\n");
        out.push_str("|---|---|---|---|---|\n");
        for r in &self.recommendations {
            let status = match r.status {
                CellStatus::Direct => "direct",
                CellStatus::Inferred => "inferred",
                CellStatus::Disconnected => "disconnected",
            };
            let half_width = r
                .current_ci_half_width
                .map(|w| format!("{w:.1}"))
                .unwrap_or_else(|| "n/a".to_string());
            let trials = r
                .estimated_additional_trials
                .map(|n| n.to_string())
                .unwrap_or_else(|| "n/a".to_string());
            out.push_str(&format!(
                "| {} | {} | {status} | {half_width} | {trials} |\n",
                r.row, r.col
            ));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::MatchRecord;
    use crate::stats::bootstrap::DEFAULT_SEED as TEST_SEED;

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

    #[allow(clippy::too_many_arguments)]
    fn plan(
        named: Vec<NamedRecords>,
        matches: Vec<Result<MatchFile, VeridictError>>,
        min_elo: f64,
    ) -> Result<PlanReport, VeridictError> {
        run(
            named,
            matches,
            0.95,
            false,
            TEST_RESAMPLES,
            TEST_SEED,
            BootstrapMethod::Percentile,
            min_elo,
        )
    }

    #[test]
    fn min_elo_must_be_positive_and_finite() {
        let data = vec![("a".to_string(), stream("candidate_win", 20))];
        for bad in [0.0, -5.0, f64::NAN, f64::INFINITY] {
            assert!(matches!(
                plan(ok_named(data.clone()), no_matches(), bad),
                Err(VeridictError::InvalidThreshold(_))
            ));
        }
    }

    #[test]
    fn tiny_star_graph_sample_gets_a_large_estimate_that_shrinks_with_more_data() {
        let small = plan(
            ok_named(vec![("a".to_string(), stream("candidate_win", 10))]),
            no_matches(),
            10.0,
        )
        .unwrap();
        let large = plan(
            ok_named(vec![("a".to_string(), stream("candidate_win", 500))]),
            no_matches(),
            10.0,
        )
        .unwrap();
        let small_estimate = small.recommendations[0]
            .estimated_additional_trials
            .unwrap();
        let large_estimate = large.recommendations[0]
            .estimated_additional_trials
            .unwrap();
        assert!(
            small_estimate > large_estimate,
            "a smaller sample should need a larger additional-trials estimate: {small_estimate} vs {large_estimate}"
        );
    }

    #[test]
    fn already_within_target_reports_zero_additional_trials() {
        let report = plan(
            ok_named(vec![("a".to_string(), stream("candidate_win", 500))]),
            no_matches(),
            1000.0,
        )
        .unwrap();
        let rec = &report.recommendations[0];
        assert_eq!(rec.estimated_additional_trials, Some(0));
        assert!(rec.current_ci_half_width.unwrap() <= 1000.0);
    }

    #[test]
    fn disconnected_pair_has_no_estimate_and_an_explanatory_note() {
        let matches = ok_matches(vec![vec![
            match_rec("m1", "x", "y", "a_win"),
            match_rec("m2", "z", "w", "a_win"),
        ]]);
        let report = plan(ok_named(vec![]), matches, 20.0).unwrap();
        let cross = report
            .recommendations
            .iter()
            .find(|r| (r.row == "x" && r.col == "w") || (r.row == "w" && r.col == "x"))
            .unwrap();
        assert_eq!(cross.status, CellStatus::Disconnected);
        assert!(cross.current_ci_half_width.is_none());
        assert!(cross.estimated_additional_trials.is_none());
        assert!(cross.note.is_some());
    }

    #[test]
    fn general_graph_connected_pair_gets_a_clt_estimate() {
        let mut records = Vec::new();
        for _ in 0..3 {
            records.push(match_rec("g", "x", "y", "a_win"));
        }
        for _ in 0..3 {
            records.push(match_rec("g", "x", "y", "b_win"));
        }
        let matches = ok_matches(vec![records]);
        let report = plan(ok_named(vec![]), matches, 50.0).unwrap();
        assert_eq!(report.recommendations.len(), 1);
        let rec = &report.recommendations[0];
        assert_eq!(rec.status, CellStatus::Direct);
        assert!(rec.current_ci_half_width.is_some());
        assert!(rec.estimated_additional_trials.is_some());
    }

    #[test]
    fn recommendations_sort_most_uncertain_star_graph_candidate_first() {
        // Pure star-graph (legacy files only, no --matches) so both candidates stay on the
        // closed-form Wilson path - mixing in a non-baseline match record would push everything
        // through the general-graph solver instead, where a's/b's pure win streaks against
        // baseline (no return games) would themselves become disconnected (see
        // matrix.rs's shutout-routing doc) - a different scenario, not what this test is about.
        let named = ok_named(vec![
            ("a".to_string(), stream("candidate_win", 500)),
            ("b".to_string(), stream("candidate_win", 5)),
        ]);
        let report = plan(named, no_matches(), 10.0).unwrap();

        // Compare the two baseline-vs-candidate cells specifically (not the a-vs-b cross cell,
        // which mentions both names and would match either search).
        let b_pos = report
            .recommendations
            .iter()
            .position(|r| r.row == "baseline" && r.col == "b")
            .unwrap();
        let a_pos = report
            .recommendations
            .iter()
            .position(|r| r.row == "baseline" && r.col == "a")
            .unwrap();
        assert!(
            b_pos < a_pos,
            "the barely-sampled candidate should sort before the well-sampled one"
        );
    }

    #[test]
    fn recommendations_sort_disconnected_pairs_before_finite_ci_pairs() {
        // Two internally-mixed (non-shutout) clusters {a,b} and {c,d} that never share a
        // competitor - each cluster's own pair is Direct with a finite CI, but every
        // cross-cluster pair is genuinely Disconnected.
        let mut records = Vec::new();
        for _ in 0..3 {
            records.push(match_rec("ab", "a", "b", "a_win"));
        }
        for _ in 0..3 {
            records.push(match_rec("ab", "a", "b", "b_win"));
        }
        for _ in 0..3 {
            records.push(match_rec("cd", "c", "d", "a_win"));
        }
        for _ in 0..3 {
            records.push(match_rec("cd", "c", "d", "b_win"));
        }
        let matches = ok_matches(vec![records]);
        let report = plan(ok_named(vec![]), matches, 10.0).unwrap();

        let first_finite = report
            .recommendations
            .iter()
            .position(|r| r.status != CellStatus::Disconnected)
            .unwrap();
        assert!(
            report.recommendations[..first_finite]
                .iter()
                .all(|r| r.status == CellStatus::Disconnected),
            "every disconnected recommendation must sort before every finite-CI one"
        );
        assert!(
            report.recommendations[first_finite..]
                .iter()
                .all(|r| r.status != CellStatus::Disconnected)
        );
    }

    #[test]
    fn empty_input_is_an_error() {
        assert!(matches!(
            plan(ok_named(vec![]), no_matches(), 10.0),
            Err(VeridictError::EmptyInput)
        ));
    }

    fn summary(candidate_count: u64, baseline_count: u64, paired_count: u64) -> CandidateSummary {
        CandidateSummary {
            name: "x".to_string(),
            elo: 0.0,
            ci_low: None,
            ci_high: None,
            baseline_count,
            candidate_count,
            paired_count,
            timeouts: 0,
            crashes: 0,
            invalid: 0,
            component: 0,
        }
    }

    #[test]
    fn exact_wilson_estimate_matches_a_genuinely_recomputable_wilson_half_width() {
        let s = summary(20, 10, 30);
        let real_half_width =
            wilson::wilson_ci_from_proportion(20.0 / 30.0 + 0.5 * 0.0 / 30.0, 30.0, 0.95)
                .map(|(lo, hi)| (elo::elo_from_score(hi) - elo::elo_from_score(lo)) / 2.0)
                .unwrap();
        assert!(exact_wilson_pair_estimate(None, Some(&s), real_half_width, 1.0, 0.95).is_some());
    }

    // Regression test for the two-sided guard: `baseline` never appears in `m.candidates` in
    // *either* star- or general-graph mode (see matrix::run), so this function's (None, Some(_))
    // dispatch alone can't tell a star-graph baseline-vs-candidate cell (Wilson CI) apart from a
    // general-graph one (bootstrap CI, no relation to this candidate's own Wilson curve). A
    // `current_half_width` that doesn't match what Wilson would actually produce for this
    // summary - standing in for "this cell's CI came from bootstrap resampling instead" - must
    // be rejected, not silently treated as an exact match.
    #[test]
    fn exact_wilson_estimate_rejects_a_half_width_that_does_not_match_a_recomputed_wilson_ci() {
        let s = summary(20, 10, 30);
        // The real Wilson half-width for this summary is ~129 Elo - deliberately larger than
        // that (standing in for a wider bootstrap CI on the same candidate), not smaller: a
        // one-sided "not wider" guard would have wrongly accepted this (128.9 > current is
        // false when current is larger), which is exactly the bug this test pins down.
        let wider_than_real = 500.0;
        assert_eq!(
            exact_wilson_pair_estimate(None, Some(&s), wider_than_real, 1.0, 0.95),
            None
        );
    }
}
