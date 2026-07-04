//! Sequential probability ratio test: accumulate a log-likelihood ratio
//! over decisive trials and stop as soon as it crosses one of Wald's
//! boundaries. Unlike `compare`, there's no confidence interval or
//! threshold to configure - the test's own alpha/beta *are* the guaranteed
//! error rates, by construction (see `stats::sprt` for the math and its
//! documented "decisive games only" assumption).

use serde::Serialize;

use crate::error::VeridictError;
use crate::input::Record;
use crate::metrics::{FailureBreakdown, OutcomeCollector, tally_status};
use crate::report::serde_str;
use crate::stats::sprt as math;
use crate::{Outcome, Verdict};

pub struct SprtConfig {
    pub elo0: f64,
    pub elo1: f64,
    pub alpha: f64,
    pub beta: f64,
}

impl SprtConfig {
    pub fn new(elo0: f64, elo1: f64, alpha: f64, beta: f64) -> Result<Self, VeridictError> {
        if !elo0.is_finite() || !elo1.is_finite() {
            return Err(VeridictError::InvalidThreshold(
                "elo0/elo1 must be finite".to_string(),
            ));
        }
        if elo0 >= elo1 {
            return Err(VeridictError::InvalidThreshold(format!(
                "elo0 ({elo0}) must be less than elo1 ({elo1})"
            )));
        }
        for (name, v) in [("alpha", alpha), ("beta", beta)] {
            if !v.is_finite() || v <= 0.0 || v >= 1.0 {
                return Err(VeridictError::InvalidThreshold(format!(
                    "{name} must be finite and in (0, 1), got {v}"
                )));
            }
        }
        Ok(Self {
            elo0,
            elo1,
            alpha,
            beta,
        })
    }
}

#[derive(Debug, Serialize)]
pub struct SprtReport {
    pub verdict: Verdict,
    pub llr: f64,
    pub lower_bound: f64,
    pub upper_bound: f64,
    pub elo0: f64,
    pub elo1: f64,
    pub alpha: f64,
    pub beta: f64,
    pub candidate_wins: u64,
    pub baseline_wins: u64,
    pub draws: u64,
    pub timeouts: u64,
    pub crashes: u64,
    pub invalid: u64,
    pub failure_breakdown: FailureBreakdown,
    pub reason: String,
}

/// `paired_by_id`: see `metrics::compute` - two records sharing an `id` are
/// combined into one net observation (by total points across the pair)
/// instead of two independent trials.
pub fn run(
    records: &[(usize, Record)],
    config: &SprtConfig,
    paired_by_id: bool,
) -> Result<SprtReport, VeridictError> {
    if records.is_empty() {
        return Err(VeridictError::EmptyInput);
    }

    let mut failures = FailureBreakdown::default();
    let mut collector = OutcomeCollector::new(paired_by_id);

    for (line, record) in records {
        let mut used = false;

        if let Some(status) = record.baseline_status.as_deref() {
            used = true;
            tally_status(status, *line, "baseline_status", &mut failures.baseline)?;
        }
        if let Some(status) = record.candidate_status.as_deref() {
            used = true;
            tally_status(status, *line, "candidate_status", &mut failures.candidate)?;
        }

        if let Some(result) = record.result.as_deref() {
            used = true;
            match Outcome::parse(result) {
                Some(outcome) => collector.record(*line, record.id.as_deref(), outcome),
                None => {
                    return Err(VeridictError::UnrecognizedOutcome {
                        line: *line,
                        value: result.to_string(),
                    });
                }
            }
        }

        if !used {
            return Err(VeridictError::SchemaMismatch {
                line: *line,
                context: "sprt",
                detail: "record has no result and no status fields".to_string(),
            });
        }
    }

    let (baseline_wins, candidate_wins, draws) = collector.finish()?;

    let timeouts = failures.baseline.timeout + failures.candidate.timeout;
    let crashes = failures.baseline.crash + failures.candidate.crash;
    let invalid = failures.baseline.invalid + failures.candidate.invalid;

    let p0 = math::score_from_elo(config.elo0);
    let p1 = math::score_from_elo(config.elo1);
    let bounds = math::bounds(config.alpha, config.beta);
    // Every candidate win contributes the same LLR delta, and likewise for
    // every loss (draws are excluded, see stats::sprt), so the accumulated
    // LLR is just each delta times its trial count - no need to loop.
    let llr = candidate_wins as f64 * math::llr_delta(true, p0, p1)
        + baseline_wins as f64 * math::llr_delta(false, p0, p1);

    let (verdict, reason) = if llr >= bounds.upper {
        (
            Verdict::Pass,
            format!(
                "LLR {llr:.3} reached the upper bound {:.3}: reject H0 (elo <= {:+.1}), accept H1 (elo >= {:+.1})",
                bounds.upper, config.elo0, config.elo1
            ),
        )
    } else if llr <= bounds.lower {
        (
            Verdict::Fail,
            format!(
                "LLR {llr:.3} reached the lower bound {:.3}: reject H1 (elo >= {:+.1}), accept H0 (elo <= {:+.1})",
                bounds.lower, config.elo1, config.elo0
            ),
        )
    } else {
        (
            Verdict::Inconclusive,
            format!(
                "LLR {llr:.3} is within ({:.3}, {:.3}): keep testing",
                bounds.lower, bounds.upper
            ),
        )
    };

    Ok(SprtReport {
        verdict,
        llr,
        lower_bound: bounds.lower,
        upper_bound: bounds.upper,
        elo0: config.elo0,
        elo1: config.elo1,
        alpha: config.alpha,
        beta: config.beta,
        candidate_wins,
        baseline_wins,
        draws,
        timeouts,
        crashes,
        invalid,
        failure_breakdown: failures,
        reason,
    })
}

impl SprtReport {
    pub fn to_json_pretty(&self) -> String {
        serde_json::to_string_pretty(self)
            .expect("SprtReport contains only finite fields and strings; serialization cannot fail")
    }

    pub fn to_markdown(&self) -> String {
        let b = &self.failure_breakdown.baseline;
        let c = &self.failure_breakdown.candidate;
        format!(
            "# Veridict SPRT Report\n\n\
             Verdict: {verdict}\n\n\
             H0: elo <= {elo0:+.1} / H1: elo >= {elo1:+.1} (alpha={alpha}, beta={beta})\n\
             LLR: {llr:.4} (bounds: {lower:.4} to {upper:.4})\n\n\
             {reason}\n\n\
             Trials: candidate_wins={candidate_wins}, baseline_wins={baseline_wins}, draws={draws}\n\n\
             Status counts:\n\
             - timeout: {timeouts} (baseline={b_timeout}, candidate={c_timeout})\n\
             - crash: {crashes} (baseline={b_crash}, candidate={c_crash})\n\
             - invalid: {invalid} (baseline={b_invalid}, candidate={c_invalid})\n",
            verdict = serde_str(&self.verdict),
            elo0 = self.elo0,
            elo1 = self.elo1,
            alpha = self.alpha,
            beta = self.beta,
            llr = self.llr,
            lower = self.lower_bound,
            upper = self.upper_bound,
            reason = self.reason,
            candidate_wins = self.candidate_wins,
            baseline_wins = self.baseline_wins,
            draws = self.draws,
            timeouts = self.timeouts,
            crashes = self.crashes,
            invalid = self.invalid,
            b_timeout = b.timeout,
            c_timeout = c.timeout,
            b_crash = b.crash,
            c_crash = c.crash,
            b_invalid = b.invalid,
            c_invalid = c.invalid,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(result: Option<&str>) -> Record {
        rec_with_id(None, result)
    }

    fn rec_with_id(id: Option<&str>, result: Option<&str>) -> Record {
        Record {
            id: id.map(str::to_string),
            baseline: None,
            candidate: None,
            result: result.map(str::to_string),
            baseline_status: None,
            candidate_status: None,
        }
    }

    #[test]
    fn clear_h1_stream_passes() {
        let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();
        let records: Vec<_> = (0..2000)
            .map(|i| (i + 1, rec(Some("candidate_win"))))
            .collect();
        let report = run(&records, &config, false).unwrap();
        assert_eq!(report.verdict, Verdict::Pass);
        assert!(report.llr >= report.upper_bound);
    }

    #[test]
    fn clear_h0_stream_fails() {
        let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();
        let records: Vec<_> = (0..2000)
            .map(|i| (i + 1, rec(Some("baseline_win"))))
            .collect();
        let report = run(&records, &config, false).unwrap();
        assert_eq!(report.verdict, Verdict::Fail);
        assert!(report.llr <= report.lower_bound);
    }

    #[test]
    fn small_mixed_sample_stays_inconclusive() {
        let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();
        let records = vec![
            (1, rec(Some("candidate_win"))),
            (2, rec(Some("baseline_win"))),
        ];
        let report = run(&records, &config, false).unwrap();
        assert_eq!(report.verdict, Verdict::Inconclusive);
    }

    #[test]
    fn draws_do_not_move_the_llr() {
        let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();
        let records = vec![(1, rec(Some("draw"))), (2, rec(Some("draw")))];
        let report = run(&records, &config, false).unwrap();
        assert_eq!(report.llr, 0.0);
        assert_eq!(report.draws, 2);
    }

    #[test]
    fn rejects_elo0_not_less_than_elo1() {
        assert!(matches!(
            SprtConfig::new(10.0, 10.0, 0.05, 0.05),
            Err(VeridictError::InvalidThreshold(_))
        ));
        assert!(matches!(
            SprtConfig::new(10.0, 0.0, 0.05, 0.05),
            Err(VeridictError::InvalidThreshold(_))
        ));
    }

    #[test]
    fn rejects_alpha_beta_out_of_range() {
        assert!(matches!(
            SprtConfig::new(0.0, 10.0, 0.0, 0.05),
            Err(VeridictError::InvalidThreshold(_))
        ));
        assert!(matches!(
            SprtConfig::new(0.0, 10.0, 0.05, 1.0),
            Err(VeridictError::InvalidThreshold(_))
        ));
    }

    #[test]
    fn empty_input_is_an_error() {
        let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();
        assert!(matches!(
            run(&[], &config, false),
            Err(VeridictError::EmptyInput)
        ));
    }

    #[test]
    fn unusable_record_is_schema_mismatch() {
        let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();
        let records = vec![(1, rec(None))];
        assert!(matches!(
            run(&records, &config, false),
            Err(VeridictError::SchemaMismatch { .. })
        ));
    }

    #[test]
    fn paired_by_id_nets_split_pairs_to_a_draw() {
        let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();
        // Every testcase splits 1-1 (net draw) when paired: LLR stays at 0,
        // even though un-paired this would be 1000 wins + 1000 losses too -
        // same total either way here, so assert the more telling case below.
        let records: Vec<_> = (0..1000)
            .flat_map(|i| {
                [
                    (
                        i * 2 + 1,
                        rec_with_id(Some(&format!("op{i}")), Some("candidate_win")),
                    ),
                    (
                        i * 2 + 2,
                        rec_with_id(Some(&format!("op{i}")), Some("baseline_win")),
                    ),
                ]
            })
            .collect();
        let report = run(&records, &config, true).unwrap();
        assert_eq!(report.llr, 0.0);
        assert_eq!(report.draws, 1000);
    }
}
