//! Comparison matrix across more than two candidates, all measured against
//! a shared baseline.
//!
//! ponytail: this only supports "star" topology - every candidate plays
//! only the shared baseline, never each other directly (that's what the
//! existing baseline/candidate input schema gives us, one file per
//! candidate). On a star graph, the Bradley-Terry MLE for each candidate's
//! rating reduces exactly to that candidate's own Elo-vs-baseline (no
//! shared terms to solve jointly), so this reuses `metrics::compute`'s
//! `elo` path per file rather than an iterative solver. candidate-vs-
//! candidate cells are therefore *model-extrapolated*
//! (`elo_i - elo_j`), not measured from head-to-head data, and their CIs
//! come from normal-approximation error propagation across two independent
//! samples (`margin = sqrt(margin_i^2 + margin_j^2)`, using each side's own
//! CI half-width as its margin - an approximation on top of Wilson's own
//! normal approximation, since the two candidates' Elo estimates are
//! independent runs). Upgrade to a real multi-way MLE (with a genuine
//! named-competitor input schema, so candidates can play each other
//! directly) if this star-graph assumption stops holding.

use serde::Serialize;

use crate::error::VeridictError;
use crate::input::Record;
use crate::{BootstrapMethod, CiMethod, MetricKind, metrics};

#[derive(Debug, Serialize)]
pub struct CandidateSummary {
    pub name: String,
    pub elo: f64,
    pub ci_low: f64,
    pub ci_high: f64,
    pub baseline_count: u64,
    pub candidate_count: u64,
    pub paired_count: u64,
    pub timeouts: u64,
    pub crashes: u64,
    pub invalid: u64,
}

#[derive(Debug, Serialize)]
pub struct MatrixEntry {
    pub row: String,
    pub col: String,
    /// Row's Elo advantage over col; the reverse direction is `-elo_diff`.
    pub elo_diff: f64,
    pub ci_low: f64,
    pub ci_high: f64,
    /// `false` for candidate-vs-candidate cells (model-extrapolated, no
    /// direct data); `true` for baseline-vs-candidate cells.
    pub direct: bool,
}

#[derive(Debug, Serialize)]
pub struct ComparisonMatrix {
    pub confidence: f64,
    pub candidates: Vec<CandidateSummary>,
    /// Upper triangle only, "baseline" first then `candidates` in input
    /// order; `to_markdown` mirrors it into a full grid.
    pub matrix: Vec<MatrixEntry>,
}

pub fn run(
    named_records: &[(String, Vec<(usize, Record)>)],
    confidence: f64,
    paired_by_id: bool,
) -> Result<ComparisonMatrix, VeridictError> {
    if named_records.is_empty() {
        return Err(VeridictError::EmptyInput);
    }

    let mut candidates = Vec::with_capacity(named_records.len());
    for (name, records) in named_records {
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
        candidates.push(CandidateSummary {
            name: name.clone(),
            elo: out.effect,
            ci_low: out.ci_low,
            ci_high: out.ci_high,
            baseline_count: out.baseline_count,
            candidate_count: out.candidate_count,
            paired_count: out.paired_count,
            timeouts: out.timeouts,
            crashes: out.crashes,
            invalid: out.invalid,
        });
    }

    let mut matrix = Vec::new();
    for c in &candidates {
        matrix.push(MatrixEntry {
            row: "baseline".to_string(),
            col: c.name.clone(),
            elo_diff: -c.elo,
            ci_low: -c.ci_high,
            ci_high: -c.ci_low,
            direct: true,
        });
    }
    for i in 0..candidates.len() {
        for j in (i + 1)..candidates.len() {
            let (a, b) = (&candidates[i], &candidates[j]);
            let margin_a = (a.ci_high - a.ci_low) / 2.0;
            let margin_b = (b.ci_high - b.ci_low) / 2.0;
            let margin = margin_a.hypot(margin_b);
            let diff = a.elo - b.elo;
            matrix.push(MatrixEntry {
                row: a.name.clone(),
                col: b.name.clone(),
                elo_diff: diff,
                ci_low: diff - margin,
                ci_high: diff + margin,
                direct: false,
            });
        }
    }

    Ok(ComparisonMatrix {
        confidence,
        candidates,
        matrix,
    })
}

impl ComparisonMatrix {
    pub fn to_json_pretty(&self) -> String {
        serde_json::to_string_pretty(self).expect(
            "ComparisonMatrix contains only finite fields and strings; serialization cannot fail",
        )
    }

    pub fn to_markdown(&self) -> String {
        let mut names = vec!["baseline".to_string()];
        names.extend(self.candidates.iter().map(|c| c.name.clone()));

        let lookup = |row: &str, col: &str| -> Option<(f64, bool)> {
            self.matrix.iter().find_map(|e| {
                if e.row == row && e.col == col {
                    Some((e.elo_diff, e.direct))
                } else if e.row == col && e.col == row {
                    Some((-e.elo_diff, e.direct))
                } else {
                    None
                }
            })
        };

        let mut out = String::from("# Veridict Comparison Matrix\n\n");
        out.push_str("Cell = row's Elo advantage over column. `*` marks a model-extrapolated cell (no direct head-to-head data).\n\n");
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
                    Some((v, direct)) => {
                        let marker = if direct { "" } else { "*" };
                        out.push_str(&format!(" {v:+.1}{marker} |"));
                    }
                    None => out.push_str(" ? |"),
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

    #[test]
    fn baseline_vs_candidate_is_direct() {
        let data = vec![("a".to_string(), stream("candidate_win", 20))];
        let m = run(&data, 0.95, false).unwrap();
        assert_eq!(m.matrix.len(), 1);
        assert!(m.matrix[0].direct);
        assert_eq!(m.matrix[0].row, "baseline");
        assert_eq!(m.matrix[0].col, "a");
        assert!(m.matrix[0].elo_diff < 0.0); // baseline lost every game
    }

    #[test]
    fn candidate_vs_candidate_is_extrapolated() {
        let data = vec![
            ("a".to_string(), stream("candidate_win", 20)),
            ("b".to_string(), stream("baseline_win", 20)),
        ];
        let m = run(&data, 0.95, false).unwrap();
        let cross = m
            .matrix
            .iter()
            .find(|e| e.row == "a" && e.col == "b")
            .unwrap();
        assert!(!cross.direct);
        assert!(cross.elo_diff > 0.0); // a beat baseline, b lost to baseline
    }

    #[test]
    fn markdown_mirrors_into_a_full_grid() {
        let data = vec![("a".to_string(), stream("candidate_win", 20))];
        let md = run(&data, 0.95, false).unwrap().to_markdown();
        assert!(md.contains("| baseline |"));
        assert!(md.contains("| a |"));
        assert!(md.contains(" - |"));
    }

    #[test]
    fn empty_input_is_an_error() {
        assert!(matches!(
            run(&[], 0.95, false),
            Err(VeridictError::EmptyInput)
        ));
    }
}
