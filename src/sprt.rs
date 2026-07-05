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
use crate::stats::trinomial_sprt;
use crate::{Outcome, Verdict};

/// Which SPRT is run. `Wald` (default): classic two-outcome test, draws
/// excluded, `elo0`/`elo1` are logistic Elo (`stats::sprt::score_from_elo`).
/// `Trinomial`: draw rate estimated as a nuisance parameter from the pooled
/// counts (see `stats::trinomial_sprt`), `elo0`/`elo1` are BayesElo instead,
/// a different scale whenever the estimated draw rate is nonzero; that's
/// why the CLI exposes this through separate `--belo0`/`--belo1` flags
/// rather than reinterpreting `--elo0`/`--elo1`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SprtVariant {
    Wald,
    Trinomial,
}

pub struct SprtConfig {
    /// H0 hypothesis gap. Logistic Elo for `SprtVariant::Wald`, BayesElo for
    /// `SprtVariant::Trinomial` - see `SprtVariant`'s doc for why those are
    /// different scales.
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
    pub schema_version: u32,
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
    /// The estimated draw-rate nuisance parameter, `Some` only for
    /// `SprtVariant::Trinomial` (reported for transparency, since it's
    /// estimated from the same data being judged) - `None` for
    /// `SprtVariant::Wald`, which doesn't model a draw rate at all.
    pub drawelo: Option<f64>,
}

/// `paired_by_id`: see `metrics::compute` - two records sharing an `id` are
/// combined into one net observation (by total points across the pair)
/// instead of two independent trials. `records` is a streaming iterator
/// (see `metrics::compute_many`'s doc for why) - this only ever tallies
/// counters via `OutcomeCollector`, so memory stays bounded regardless of
/// input size (modulo `--paired-by-id`'s in-flight-id buffering).
pub fn run<I>(
    records: I,
    config: &SprtConfig,
    variant: SprtVariant,
    paired_by_id: bool,
) -> Result<SprtReport, VeridictError>
where
    I: IntoIterator<Item = Result<(usize, Record), VeridictError>>,
{
    let mut records = records.into_iter().peekable();
    if records.peek().is_none() {
        return Err(VeridictError::EmptyInput);
    }

    let mut failures = FailureBreakdown::default();
    let mut collector = OutcomeCollector::new(paired_by_id);

    for item in records {
        let (line, record) = item?;
        let mut used = false;

        if let Some(status) = record.baseline_status.as_deref() {
            used = true;
            tally_status(status, line, "baseline_status", &mut failures.baseline)?;
        }
        if let Some(status) = record.candidate_status.as_deref() {
            used = true;
            tally_status(status, line, "candidate_status", &mut failures.candidate)?;
        }

        if let Some(result) = record.result.as_deref() {
            used = true;
            match Outcome::parse(result) {
                Some(outcome) => collector.record(line, record.id.as_deref(), outcome),
                None => {
                    return Err(VeridictError::UnrecognizedOutcome {
                        line,
                        value: result.to_string(),
                        expected: "baseline_win|candidate_win|draw",
                    });
                }
            }
        }

        if !used {
            return Err(VeridictError::SchemaMismatch {
                line,
                context: "sprt",
                detail: "record has no result and no status fields".to_string(),
            });
        }
    }

    let (baseline_wins, candidate_wins, draws) = collector.finish()?;

    let timeouts = failures.baseline.timeout + failures.candidate.timeout;
    let crashes = failures.baseline.crash + failures.candidate.crash;
    let invalid = failures.baseline.invalid + failures.candidate.invalid;

    let bounds = math::bounds(config.alpha, config.beta);
    let (llr, drawelo, unit) = match variant {
        SprtVariant::Wald => {
            let p0 = math::score_from_elo(config.elo0);
            let p1 = math::score_from_elo(config.elo1);
            // Every candidate win contributes the same LLR delta, and
            // likewise for every loss (draws are excluded, see stats::sprt),
            // so the accumulated LLR is just each delta times its trial
            // count - no need to loop.
            let llr = candidate_wins as f64 * math::llr_delta(true, p0, p1)
                + baseline_wins as f64 * math::llr_delta(false, p0, p1);
            (llr, None, "elo")
        }
        SprtVariant::Trinomial => {
            let (llr, drawelo) = trinomial_sprt::llr(
                config.elo0,
                config.elo1,
                candidate_wins,
                draws,
                baseline_wins,
            );
            (llr, Some(drawelo), "belo")
        }
    };

    let (verdict, reason) = if llr >= bounds.upper {
        (
            Verdict::Pass,
            format!(
                "LLR {llr:.3} reached the upper bound {:.3}: reject H0 ({unit} <= {:+.1}), accept H1 ({unit} >= {:+.1})",
                bounds.upper, config.elo0, config.elo1
            ),
        )
    } else if llr <= bounds.lower {
        (
            Verdict::Fail,
            format!(
                "LLR {llr:.3} reached the lower bound {:.3}: reject H1 ({unit} >= {:+.1}), accept H0 ({unit} <= {:+.1})",
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
        schema_version: crate::report::REPORT_SCHEMA_VERSION,
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
        drawelo,
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
        let unit = if self.drawelo.is_some() {
            "belo"
        } else {
            "elo"
        };
        format!(
            "# Veridict SPRT Report\n\n\
             Verdict: {verdict}\n\n\
             H0: {unit} <= {elo0:+.1} / H1: {unit} >= {elo1:+.1} (alpha={alpha}, beta={beta})\n\
             LLR: {llr:.4} (bounds: {lower:.4} to {upper:.4})\n\
             {drawelo_line}\n\
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
            drawelo_line = match self.drawelo {
                Some(d) => format!("Estimated drawelo: {d:+.1}\n"),
                None => String::new(),
            },
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

    fn ok_iter(
        records: &[(usize, Record)],
    ) -> impl Iterator<Item = Result<(usize, Record), VeridictError>> + '_ {
        records.iter().cloned().map(Ok)
    }

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
        let report = run(ok_iter(&records), &config, SprtVariant::Wald, false).unwrap();
        assert_eq!(report.verdict, Verdict::Pass);
        assert!(report.llr >= report.upper_bound);
        assert_eq!(report.drawelo, None);
    }

    #[test]
    fn trinomial_clear_h1_stream_passes_and_reports_drawelo() {
        let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();
        let records: Vec<_> = (0..2000)
            .map(|i| (i + 1, rec(Some("candidate_win"))))
            .collect();
        let report = run(ok_iter(&records), &config, SprtVariant::Trinomial, false).unwrap();
        assert_eq!(report.verdict, Verdict::Pass);
        assert!(report.llr >= report.upper_bound);
        assert!(report.drawelo.is_some());
    }

    #[test]
    fn trinomial_draw_heavy_stream_still_reaches_a_verdict() {
        // A draw-heavy but clearly candidate-favored stream - the scenario
        // this variant exists for. Not asserting a specific verdict (that
        // depends on the exact mix), just that it computes a finite report
        // rather than getting stuck on the draw-rate estimation.
        let mut records = Vec::new();
        for i in 0..300 {
            records.push((i * 2 + 1, rec(Some("draw"))));
            records.push((i * 2 + 2, rec(Some("candidate_win"))));
        }
        let config = SprtConfig::new(0.0, 30.0, 0.05, 0.05).unwrap();
        let report = run(ok_iter(&records), &config, SprtVariant::Trinomial, false).unwrap();
        assert!(report.llr.is_finite());
        assert!(report.drawelo.unwrap().is_finite());
    }

    #[test]
    fn trinomial_zero_draws_matches_wald_verdict() {
        // Integration-level companion to stats::trinomial_sprt's exact
        // pure-math reduction test: with no draws in the actual record
        // stream, both variants should reach the same verdict end to end.
        let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();
        let records: Vec<_> = (0..1000)
            .map(|i| {
                (
                    i + 1,
                    rec(Some(if i % 7 == 0 {
                        "baseline_win"
                    } else {
                        "candidate_win"
                    })),
                )
            })
            .collect();
        let wald = run(ok_iter(&records), &config, SprtVariant::Wald, false).unwrap();
        let trinomial = run(ok_iter(&records), &config, SprtVariant::Trinomial, false).unwrap();
        assert_eq!(wald.verdict, trinomial.verdict);
        assert!((wald.llr - trinomial.llr).abs() < 1e-6);
    }

    #[test]
    fn clear_h0_stream_fails() {
        let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();
        let records: Vec<_> = (0..2000)
            .map(|i| (i + 1, rec(Some("baseline_win"))))
            .collect();
        let report = run(ok_iter(&records), &config, SprtVariant::Wald, false).unwrap();
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
        let report = run(ok_iter(&records), &config, SprtVariant::Wald, false).unwrap();
        assert_eq!(report.verdict, Verdict::Inconclusive);
    }

    #[test]
    fn draws_do_not_move_the_llr() {
        let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();
        let records = vec![(1, rec(Some("draw"))), (2, rec(Some("draw")))];
        let report = run(ok_iter(&records), &config, SprtVariant::Wald, false).unwrap();
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
            run(ok_iter(&[]), &config, SprtVariant::Wald, false),
            Err(VeridictError::EmptyInput)
        ));
    }

    #[test]
    fn unusable_record_is_schema_mismatch() {
        let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();
        let records = vec![(1, rec(None))];
        assert!(matches!(
            run(ok_iter(&records), &config, SprtVariant::Wald, false),
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
        let report = run(ok_iter(&records), &config, SprtVariant::Wald, true).unwrap();
        assert_eq!(report.llr, 0.0);
        assert_eq!(report.draws, 1000);
    }
}
