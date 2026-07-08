//! Sequential probability ratio test: accumulate a log-likelihood ratio
//! over decisive trials and stop as soon as it crosses one of Wald's
//! boundaries. Unlike `compare`, there's no confidence interval or
//! threshold to configure - the test's own alpha/beta *are* the guaranteed
//! error rates, by construction (see `stats::sprt` for the math and its
//! documented "decisive games only" assumption).

use std::collections::HashMap;

use serde::Serialize;

use crate::error::VeridictError;
use crate::input::Record;
use crate::metrics::{FailureBreakdown, OutcomeCollector, tally_status};
use crate::report::serde_str;
use crate::stats::pentanomial_sprt;
use crate::stats::sprt as math;
use crate::stats::trinomial_sprt;
use crate::{Outcome, Verdict};

/// Which SPRT is run. `Wald` (default): classic two-outcome test, draws
/// excluded, `elo0`/`elo1` are logistic Elo (`stats::sprt::score_from_elo`).
/// `Trinomial`: draw rate estimated as a nuisance parameter from the pooled
/// counts (see `stats::trinomial_sprt`), `elo0`/`elo1` are BayesElo instead,
/// a different scale whenever the estimated draw rate is nonzero; that's
/// why the CLI exposes this through separate `--belo0`/`--belo1` flags
/// rather than reinterpreting `--elo0`/`--elo1`. `Pentanomial`: paired-game
/// (two games sharing an id, e.g. same opening with colors swapped) test
/// over the pair's 5-value combined score instead of two individual
/// win/loss/draw outcomes (see `stats::pentanomial_sprt`'s doc for why this
/// isn't just trinomial run on twice as many games) - `elo0`/`elo1` are
/// logistic Elo, the same scale as `Wald` (this model has no drawelo-style
/// nuisance parameter to make BayesElo meaningful), and it always requires
/// `--paired-by-id` (a 5-value pair score has no meaning for a lone game).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SprtVariant {
    Wald,
    Trinomial,
    Pentanomial,
}

impl SprtVariant {
    fn label(self) -> &'static str {
        match self {
            SprtVariant::Wald => "wald",
            SprtVariant::Trinomial => "trinomial",
            SprtVariant::Pentanomial => "pentanomial",
        }
    }
}

pub struct SprtConfig {
    /// H0 hypothesis gap. Logistic Elo for `SprtVariant::Wald`/`Pentanomial`,
    /// BayesElo for `SprtVariant::Trinomial` - see `SprtVariant`'s doc for
    /// why trinomial alone is a different scale.
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

/// Breakdown of pentanomial pairs by combined candidate score across a pair's two games -
/// `Some` only for `SprtVariant::Pentanomial`. Field names spell out the score rather than
/// using an array/index, since "which bucket is index 2" isn't self-describing in JSON the way
/// a named field is.
#[derive(Debug, Serialize)]
pub struct PentanomialCounts {
    pub score_0_0: u64,
    pub score_0_5: u64,
    pub score_1_0: u64,
    pub score_1_5: u64,
    pub score_2_0: u64,
}

impl PentanomialCounts {
    fn from_buckets(buckets: [u64; 5]) -> Self {
        Self {
            score_0_0: buckets[0],
            score_0_5: buckets[1],
            score_1_0: buckets[2],
            score_1_5: buckets[3],
            score_2_0: buckets[4],
        }
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
    /// estimated from the same data being judged) - `None` for `Wald`/
    /// `Pentanomial`, neither of which models a draw rate.
    pub drawelo: Option<f64>,
    /// Which variant produced this report - additive alongside the fields
    /// above (present for every variant, not just `Pentanomial`) so a
    /// consumer never has to infer it from `drawelo`'s presence.
    pub sprt_variant: &'static str,
    /// `Some` only for `SprtVariant::Pentanomial`: the 5-value breakdown the
    /// LLR was actually computed from (`candidate_wins`/`baseline_wins`/
    /// `draws` above are still populated too, netted from these same 5
    /// buckets, for compatibility with tooling that only understands the
    /// 3-outcome shape).
    pub pentanomial_counts: Option<PentanomialCounts>,
    /// `Some` only for `SprtVariant::Pentanomial`: total input records
    /// (before pairing) - always `2 * paired_count`, since an incomplete
    /// pair is rejected before a report is ever produced (see
    /// `PentanomialCollector::finish`). Reported anyway for a reader to
    /// self-check the pairing math without cross-referencing the input.
    pub raw_trial_count: Option<u64>,
    /// `Some` only for `SprtVariant::Pentanomial`: number of complete pairs
    /// (distinct ids) the LLR was computed over.
    pub paired_count: Option<u64>,
}

/// Strict pairing for `SprtVariant::Pentanomial`: unlike `OutcomeCollector` (which tolerates a
/// lone id as an ordinary unpaired sample), every id here must resolve to *exactly* 2 records.
/// A pentanomial pair's whole statistical value is the cancellation between two games sharing
/// an opening (see `stats::pentanomial_sprt`'s module doc) - a lone game has no partner to
/// cancel bias against, so treating it as a substitute single-game observation (the way
/// `OutcomeCollector` does for other variants) would silently mix bias-cancelled and
/// bias-uncancelled observations into the same LLR sum. Rejecting it outright is the
/// conservative choice: an ambiguous pairing structure should fail loudly, not get judged
/// anyway (per this project's "false pass is worse than inconclusive" bias).
struct PentanomialCollector {
    groups: HashMap<String, Vec<(usize, Outcome)>>,
}

impl PentanomialCollector {
    fn new() -> Self {
        Self {
            groups: HashMap::new(),
        }
    }

    fn record(&mut self, line: usize, id: &str, outcome: Outcome) {
        self.groups
            .entry(id.to_string())
            .or_default()
            .push((line, outcome));
    }

    /// `(bucket counts, raw trial count, paired count)`. Bucket `i` counts pairs whose combined
    /// candidate score (summed over the pair's two games, `win=1`/`draw=0.5`/`loss=0` each, the
    /// same points convention `OutcomeCollector::finish` already uses) was `i / 2.0`.
    fn finish(self) -> Result<([u64; 5], u64, u64), VeridictError> {
        let mut buckets = [0u64; 5];
        let mut raw_trial_count = 0u64;
        let paired_count = self.groups.len() as u64;

        for (id, group) in self.groups {
            raw_trial_count += group.len() as u64;
            match group.as_slice() {
                [(line, _)] => {
                    return Err(VeridictError::SchemaMismatch {
                        line: *line,
                        context: "pentanomial",
                        detail: format!(
                            "id '{id}' appears once; --sprt-variant pentanomial requires \
                             exactly 2 records per id (a lone game can't cancel the pair's own \
                             bias)"
                        ),
                    });
                }
                [(_, a), (_, b)] => {
                    let points = |o: &Outcome| -> f64 {
                        match o {
                            Outcome::CandidateWin => 1.0,
                            Outcome::Draw => 0.5,
                            Outcome::BaselineWin => 0.0,
                        }
                    };
                    let bucket = ((points(a) + points(b)) * 2.0).round() as usize;
                    buckets[bucket] += 1;
                }
                more => {
                    return Err(VeridictError::SchemaMismatch {
                        line: more[0].0,
                        context: "pentanomial",
                        detail: format!(
                            "id '{id}' appears {} times; pentanomial mode expects exactly 2 \
                             records per id",
                            more.len()
                        ),
                    });
                }
            }
        }
        Ok((buckets, raw_trial_count, paired_count))
    }
}

/// `(baseline_wins, candidate_wins, draws)` netted from a pentanomial bucket breakdown, the
/// same ">1/=1/<1 total points" convention `OutcomeCollector::finish` and the "Paired
/// testcases" README section already document - keeps `SprtReport`'s existing 3-outcome fields
/// meaningful for `Pentanomial` too, instead of left at `0`.
fn net_pentanomial_buckets(buckets: &[u64; 5]) -> (u64, u64, u64) {
    let baseline_wins = buckets[0] + buckets[1];
    let draws = buckets[2];
    let candidate_wins = buckets[3] + buckets[4];
    (baseline_wins, candidate_wins, draws)
}

/// `paired_by_id`: see `metrics::compute` - two records sharing an `id` are
/// combined into one net observation (by total points across the pair)
/// instead of two independent trials. `records` is a streaming iterator
/// (see `metrics::compute_many`'s doc for why) - this only ever tallies
/// counters via `OutcomeCollector`, so memory stays bounded regardless of
/// input size (modulo `--paired-by-id`'s in-flight-id buffering).
///
/// `SprtVariant::Pentanomial` always requires `paired_by_id`: rejected up front rather than
/// silently ignored, matching `resolve_sprt_hypotheses`'s existing "never silently ignore
/// invalid data" precedent for the `--elo0`/`--belo0` cross-variant flags.
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
    if variant == SprtVariant::Pentanomial && !paired_by_id {
        return Err(VeridictError::InvalidThreshold(
            "--sprt-variant pentanomial requires --paired-by-id".to_string(),
        ));
    }

    let mut failures = FailureBreakdown::default();
    // Both collectors are always constructed (cheap - an empty `HashMap` allocates nothing),
    // but only the one matching `variant` ever gets fed a record or consumed via `finish()`
    // below; the other is simply dropped unused. Simpler than threading an `Option` through the
    // loop for what's a single small allocation-free struct either way.
    let mut collector = OutcomeCollector::new(paired_by_id);
    let mut pentanomial_collector = PentanomialCollector::new();

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
            let outcome = match Outcome::parse(result) {
                Some(outcome) => outcome,
                None => {
                    return Err(VeridictError::UnrecognizedOutcome {
                        line,
                        value: result.to_string(),
                        expected: "baseline_win|candidate_win|draw",
                    });
                }
            };
            if variant == SprtVariant::Pentanomial {
                let id = record
                    .id
                    .as_deref()
                    .ok_or_else(|| VeridictError::SchemaMismatch {
                        line,
                        context: "pentanomial",
                        detail: "record has no id; --sprt-variant pentanomial requires every \
                             record to carry one"
                            .to_string(),
                    })?;
                pentanomial_collector.record(line, id, outcome);
            } else {
                collector.record(line, record.id.as_deref(), outcome);
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

    let timeouts = failures.baseline.timeout + failures.candidate.timeout;
    let crashes = failures.baseline.crash + failures.candidate.crash;
    let invalid = failures.baseline.invalid + failures.candidate.invalid;

    let bounds = math::bounds(config.alpha, config.beta);
    let (candidate_wins, baseline_wins, draws, llr, drawelo, unit, pentanomial) = match variant {
        SprtVariant::Wald => {
            let (baseline_wins, candidate_wins, draws) = collector.finish()?;
            let p0 = math::score_from_elo(config.elo0);
            let p1 = math::score_from_elo(config.elo1);
            // Every candidate win contributes the same LLR delta, and
            // likewise for every loss (draws are excluded, see stats::sprt),
            // so the accumulated LLR is just each delta times its trial
            // count - no need to loop.
            let llr = candidate_wins as f64 * math::llr_delta(true, p0, p1)
                + baseline_wins as f64 * math::llr_delta(false, p0, p1);
            (candidate_wins, baseline_wins, draws, llr, None, "elo", None)
        }
        SprtVariant::Trinomial => {
            let (baseline_wins, candidate_wins, draws) = collector.finish()?;
            let (llr, drawelo) = trinomial_sprt::llr(
                config.elo0,
                config.elo1,
                candidate_wins,
                draws,
                baseline_wins,
            );
            (
                candidate_wins,
                baseline_wins,
                draws,
                llr,
                Some(drawelo),
                "belo",
                None,
            )
        }
        SprtVariant::Pentanomial => {
            let (buckets, raw_trial_count, paired_count) = pentanomial_collector.finish()?;
            let llr = pentanomial_sprt::pentanomial_llr(config.elo0, config.elo1, &buckets);
            let (baseline_wins, candidate_wins, draws) = net_pentanomial_buckets(&buckets);
            (
                candidate_wins,
                baseline_wins,
                draws,
                llr,
                None,
                "elo",
                Some((buckets, raw_trial_count, paired_count)),
            )
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

    let (pentanomial_counts, raw_trial_count, paired_count) = match pentanomial {
        Some((buckets, raw_trial_count, paired_count)) => (
            Some(PentanomialCounts::from_buckets(buckets)),
            Some(raw_trial_count),
            Some(paired_count),
        ),
        None => (None, None, None),
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
        sprt_variant: variant.label(),
        pentanomial_counts,
        raw_trial_count,
        paired_count,
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
             Trials: candidate_wins={candidate_wins}, baseline_wins={baseline_wins}, draws={draws}\n\
             {pentanomial_line}\n\
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
            pentanomial_line = match &self.pentanomial_counts {
                Some(p) => format!(
                    "\nPentanomial pairs ({} of {} raw trials): 0-0={} 0.5-0={} 1-1={} 1.5-0.5={} 2-0={}\n",
                    self.paired_count.unwrap_or(0),
                    self.raw_trial_count.unwrap_or(0),
                    p.score_0_0,
                    p.score_0_5,
                    p.score_1_0,
                    p.score_1_5,
                    p.score_2_0,
                ),
                None => String::new(),
            },
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

    /// `n` pairs (`2n` records), each pair scoring `outcomes` (candidate's two per-game
    /// outcomes for that pair, summed to one pentanomial bucket).
    fn pentanomial_records(n: usize, outcomes: (&str, &str)) -> Vec<(usize, Record)> {
        (0..n)
            .flat_map(|i| {
                let id = format!("op{i}");
                [
                    (i * 2 + 1, rec_with_id(Some(&id), Some(outcomes.0))),
                    (i * 2 + 2, rec_with_id(Some(&id), Some(outcomes.1))),
                ]
            })
            .collect()
    }

    #[test]
    fn pentanomial_paired_2_0_favors_candidate() {
        let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();
        let records = pentanomial_records(200, ("candidate_win", "candidate_win"));
        let report = run(ok_iter(&records), &config, SprtVariant::Pentanomial, true).unwrap();
        assert_eq!(report.sprt_variant, "pentanomial");
        assert_eq!(report.paired_count, Some(200));
        assert_eq!(report.raw_trial_count, Some(400));
        let counts = report.pentanomial_counts.as_ref().unwrap();
        assert_eq!(counts.score_2_0, 200);
        assert_eq!(
            [
                counts.score_0_0,
                counts.score_0_5,
                counts.score_1_0,
                counts.score_1_5
            ],
            [0, 0, 0, 0]
        );
        assert_eq!(report.candidate_wins, 200);
        assert!(report.llr > 0.0);
        assert_eq!(report.verdict, Verdict::Pass);
        assert_eq!(report.drawelo, None);
    }

    #[test]
    fn pentanomial_paired_1_5_0_5_favors_candidate() {
        let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();
        let records = pentanomial_records(200, ("candidate_win", "draw"));
        let report = run(ok_iter(&records), &config, SprtVariant::Pentanomial, true).unwrap();
        let counts = report.pentanomial_counts.as_ref().unwrap();
        assert_eq!(counts.score_1_5, 200);
        assert_eq!(report.candidate_wins, 200);
        assert!(report.llr > 0.0);
    }

    #[test]
    fn pentanomial_paired_1_1_is_neutral() {
        let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();
        let records = pentanomial_records(200, ("candidate_win", "baseline_win"));
        let report = run(ok_iter(&records), &config, SprtVariant::Pentanomial, true).unwrap();
        let counts = report.pentanomial_counts.as_ref().unwrap();
        assert_eq!(counts.score_1_0, 200);
        assert_eq!(report.draws, 200);
        assert_eq!(report.candidate_wins, 0);
        assert_eq!(report.baseline_wins, 0);
    }

    #[test]
    fn pentanomial_paired_0_5_1_5_favors_baseline() {
        let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();
        let records = pentanomial_records(200, ("baseline_win", "draw"));
        let report = run(ok_iter(&records), &config, SprtVariant::Pentanomial, true).unwrap();
        let counts = report.pentanomial_counts.as_ref().unwrap();
        assert_eq!(counts.score_0_5, 200);
        assert_eq!(report.baseline_wins, 200);
        assert!(report.llr < 0.0);
    }

    #[test]
    fn pentanomial_paired_0_2_favors_baseline() {
        let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();
        let records = pentanomial_records(200, ("baseline_win", "baseline_win"));
        let report = run(ok_iter(&records), &config, SprtVariant::Pentanomial, true).unwrap();
        let counts = report.pentanomial_counts.as_ref().unwrap();
        assert_eq!(counts.score_0_0, 200);
        assert_eq!(report.baseline_wins, 200);
        assert!(report.llr < 0.0);
        assert_eq!(report.verdict, Verdict::Fail);
    }

    #[test]
    fn pentanomial_incomplete_pair_is_an_error() {
        let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();
        let mut records = pentanomial_records(50, ("candidate_win", "baseline_win"));
        records.push((999, rec_with_id(Some("lonely"), Some("candidate_win"))));
        assert!(matches!(
            run(ok_iter(&records), &config, SprtVariant::Pentanomial, true),
            Err(VeridictError::SchemaMismatch {
                context: "pentanomial",
                ..
            })
        ));
    }

    #[test]
    fn pentanomial_triple_id_is_an_error() {
        let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();
        let mut records = pentanomial_records(50, ("candidate_win", "baseline_win"));
        records.push((997, rec_with_id(Some("op0"), Some("candidate_win"))));
        assert!(matches!(
            run(ok_iter(&records), &config, SprtVariant::Pentanomial, true),
            Err(VeridictError::SchemaMismatch {
                context: "pentanomial",
                ..
            })
        ));
    }

    #[test]
    fn pentanomial_record_without_id_is_an_error() {
        let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();
        let records = vec![(1, rec(Some("candidate_win")))];
        assert!(matches!(
            run(ok_iter(&records), &config, SprtVariant::Pentanomial, true),
            Err(VeridictError::SchemaMismatch {
                context: "pentanomial",
                ..
            })
        ));
    }

    #[test]
    fn pentanomial_without_paired_by_id_is_rejected() {
        let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();
        let records = pentanomial_records(50, ("candidate_win", "baseline_win"));
        assert!(matches!(
            run(ok_iter(&records), &config, SprtVariant::Pentanomial, false),
            Err(VeridictError::InvalidThreshold(_))
        ));
    }
}
