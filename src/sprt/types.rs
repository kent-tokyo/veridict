//! Public SPRT configuration and report types. Internal replay state stays in the parent module.

use crate::error::VeridictError;
use crate::metrics::FailureBreakdown;
use crate::report::{append_markdown_warnings, serde_str};
use crate::stats::sprt as math;
use crate::{FailurePolicy, Promotion, Validity, Verdict};
use serde::{Deserialize, Serialize};

/// SPRT report schema v2 adds the required `warnings` and `data_quality` fields. Keeping this
/// separate from the other report families prevents an SPRT-only contract change from falsely
/// relabeling every JSON report in the crate.
pub const SPRT_REPORT_SCHEMA_VERSION: u32 = 2;

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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SprtVariant {
    Wald,
    Trinomial,
    Pentanomial,
}

impl SprtVariant {
    pub(super) fn label(self) -> &'static str {
        match self {
            SprtVariant::Wald => "wald",
            SprtVariant::Trinomial => "trinomial",
            SprtVariant::Pentanomial => "pentanomial",
        }
    }
}

/// Typed policy options for [`run_with_options`]. Statistical hypotheses and error rates remain
/// in [`SprtConfig`]; this type groups the execution policies that previously occupied six
/// positional arguments in [`run`].
///
/// The default is an unpaired Wald run with failures reported but not synthesized into outcomes,
/// incomplete IDs tolerated, and no paired-ID limits. Variant-specific requirements are validated
/// centrally by [`run_with_options`]; for example, pentanomial runs must opt into pairing.
///
/// ```
/// use veridict::sprt::{SprtRunOptions, SprtVariant};
///
/// let options = SprtRunOptions {
///     variant: SprtVariant::Pentanomial,
///     paired_by_id: true,
///     min_paired_ids: Some(100),
///     ..SprtRunOptions::default()
/// };
/// assert_eq!(options.max_paired_ids, None);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SprtRunOptions {
    pub variant: SprtVariant,
    pub paired_by_id: bool,
    pub failure_policy: FailurePolicy,
    pub require_complete_pairs: bool,
    pub min_paired_ids: Option<u64>,
    pub max_paired_ids: Option<u64>,
}

impl SprtRunOptions {
    /// Creates options for `variant` with every optional policy disabled and
    /// [`FailurePolicy::ReportOnly`].
    pub const fn new(variant: SprtVariant) -> Self {
        Self {
            variant,
            paired_by_id: false,
            failure_policy: FailurePolicy::ReportOnly,
            require_complete_pairs: false,
            min_paired_ids: None,
            max_paired_ids: None,
        }
    }
}

impl Default for SprtRunOptions {
    fn default() -> Self {
        Self::new(SprtVariant::Wald)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
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
        if alpha + beta >= 1.0 {
            return Err(VeridictError::InvalidThreshold(format!(
                "alpha + beta must be less than 1, got {}",
                alpha + beta
            )));
        }
        let bounds = math::bounds(alpha, beta);
        if !bounds.lower.is_finite() || !bounds.upper.is_finite() {
            return Err(VeridictError::InvalidThreshold(
                "alpha/beta are too close to zero to produce finite SPRT bounds".to_string(),
            ));
        }
        for (name, elo) in [("elo0", elo0), ("elo1", elo1)] {
            let score = math::score_from_elo(elo);
            if !score.is_finite() || score <= 0.0 || score >= 1.0 {
                return Err(VeridictError::InvalidThreshold(format!(
                    "{name} ({elo}) is too extreme to produce a representable probability"
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
    pub(super) fn from_buckets(buckets: [u64; 5]) -> Self {
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
    /// Same meaning and default (`Valid`) as `Report::validity` - see
    /// `apply_failure_caps` in this module.
    pub validity: Validity,
    /// Same meaning as `Report::promotion`.
    pub promotion: Promotion,
    /// For `Wald`/`Trinomial`, this retains its historical full-input aggregate meaning; use
    /// `decision_llr` for the sequential verdict prefix. For `SprtVariant::Pentanomial`, this is
    /// the LLR *at the sequential stopping point* (see the
    /// module doc) - never the value a naive final-aggregate recompute over the whole input
    /// would give, which can differ whenever the true stopping point falls before the input's
    /// end. When `stopping_pair_count` is `None` (no stopping point was reached - either the
    /// input ran out first, or `min_paired_ids` was never satisfied), this is instead the raw
    /// running LLR over all available pairs, which was **never compared against the bounds** -
    /// it can sit past `lower_bound`/`upper_bound` while `verdict` is still `Inconclusive`.
    /// Callers must read `verdict`/`promotion`, never re-derive a decision from `llr` vs the
    /// bounds directly.
    pub llr: f64,
    /// LLR at the prefix that actually determined `verdict`. Equal to `llr` for Pentanomial;
    /// may differ for Wald/Trinomial because their legacy `llr` field intentionally remains the
    /// full-input aggregate for backward compatibility.
    pub decision_llr: f64,
    pub lower_bound: f64,
    pub upper_bound: f64,
    pub elo0: f64,
    pub elo1: f64,
    pub alpha: f64,
    pub beta: f64,
    pub candidate_wins: u64,
    pub baseline_wins: u64,
    pub draws: u64,
    /// Outcome counts in the prefix used by `decision_llr`. The unprefixed fields above retain
    /// their historical full-input meaning for Wald/Trinomial.
    pub decision_candidate_wins: u64,
    pub decision_baseline_wins: u64,
    pub decision_draws: u64,
    pub timeouts: u64,
    pub crashes: u64,
    pub invalid: u64,
    pub failure_breakdown: FailureBreakdown,
    pub reason: String,
    /// Human-readable advisory signals. These never alter the LLR, verdict, validity, or
    /// promotion. Structured counterparts live in `data_quality`.
    pub warnings: Vec<String>,
    pub data_quality: SprtDataQuality,
    /// The full-input estimated draw-rate nuisance parameter, `Some` only for
    /// `SprtVariant::Trinomial`. Use `decision_drawelo` for the sequential verdict prefix.
    /// `None` for `Wald`/`Pentanomial`, neither of which models a draw rate.
    pub drawelo: Option<f64>,
    /// Trinomial nuisance estimate at the sequential stopping prefix. The unprefixed `drawelo`
    /// remains the full-input estimate for compatibility. `None` for Wald/Pentanomial.
    pub decision_drawelo: Option<f64>,
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
    /// (before pairing) - always `2 * available_paired_count`, since an incomplete pair is
    /// rejected before a report is ever produced (see `PentanomialCollector::finish`). Reported
    /// anyway for a reader to self-check the pairing math without cross-referencing the input.
    pub raw_trial_count: Option<u64>,
    /// `Some` only for `SprtVariant::Pentanomial`: number of complete pairs actually analyzed,
    /// i.e. folded into `llr`/`pentanomial_counts` up to the sequential stopping point (see the
    /// module doc). May be less than `available_paired_count` when the walk stopped early -
    /// see `stopping_pair_count`/`ignored_pairs_after_stop`.
    pub paired_count: Option<u64>,
    /// `Some` only for `SprtVariant::Pentanomial`: total complete pairs present in the input,
    /// before any sequential truncation. Equal to `paired_count` unless the walk stopped early.
    pub available_paired_count: Option<u64>,
    /// `Some` only for `SprtVariant::Pentanomial`: the pair count at which the sequential walk
    /// actually stopped - a decisive verdict or a `--max-paired-ids` cutoff. `None` means the
    /// walk consumed every available pair without either (the ordinary "keep testing" state);
    /// in that case `paired_count == available_paired_count`.
    pub stopping_pair_count: Option<u64>,
    /// `Some` only for `SprtVariant::Pentanomial`: one of `"upper_bound_crossed"` /
    /// `"lower_bound_crossed"` / `"max_paired_ids_reached"` / `"insufficient_data"`.
    pub stopping_reason: Option<&'static str>,
    /// `Some` only for `SprtVariant::Pentanomial`: `available_paired_count - paired_count` -
    /// pairs present in the input but never folded into the analysis because they completed
    /// after the sequential stopping point. `Some(0)` (not `None`) when nothing was ignored, so
    /// a consumer can tell "no pairs ignored" from "not a pentanomial run" (the latter is
    /// `None`).
    pub ignored_pairs_after_stop: Option<u64>,
    /// Unit used by the generic sequential-replay counts: `"trial"` for unpaired Wald/Trinomial,
    /// `"pair"` for `--paired-by-id` Wald/Trinomial and for Pentanomial.
    pub analysis_unit: &'static str,
    /// Total reduced observations available in the input before sequential truncation.
    pub available_observation_count: u64,
    /// Reduced observations folded into `decision_llr` through the stopping prefix.
    pub analyzed_observation_count: u64,
    /// Observation count at a decisive boundary crossing; `None` when input ended inconclusively.
    pub stopping_observation_count: Option<u64>,
    /// `upper_bound_crossed`, `lower_bound_crossed`, `max_paired_ids_reached`, or
    /// `insufficient_data`, expressed generically for all variants.
    pub stopping_observation_reason: &'static str,
    /// Available observations not analyzed because they occur after the stopping prefix.
    pub ignored_observations_after_stop: u64,
    /// Raw input records that produced the available/analyzed observations. Under pairing, one
    /// observation usually represents two records; a tolerated lone/id-less trial represents one.
    pub available_record_count: u64,
    pub analyzed_record_count: u64,
    pub ignored_records_after_stop: u64,
    /// Echoes `--min-paired-ids`, `Some` only when it was passed.
    pub min_paired_ids: Option<u64>,
    /// Echoes `--max-paired-ids`, `Some` only when it was passed.
    pub max_paired_ids: Option<u64>,
    /// Echoes `--require-complete-pairs` - a report produced under strict pairing must be
    /// distinguishable from one produced without it, the same provenance concern
    /// `verify-run`'s manifest/record consistency checks exist for.
    pub require_complete_pairs: bool,
}

/// Machine-readable, verdict-independent SPRT input-quality signals.
#[derive(Debug, Serialize, Default, Clone, Copy, PartialEq, Eq)]
pub struct SprtDataQuality {
    /// One id occurs at least three times among at least ten id-tagged records in unpaired mode.
    /// Always false under `--paired-by-id`, where repeated ids define statistical pairs.
    pub low_id_diversity: bool,
}

impl SprtReport {
    pub fn to_json_pretty(&self) -> String {
        serde_json::to_string_pretty(self)
            .expect("SprtReport contains only finite fields and strings; serialization cannot fail")
    }

    pub fn to_markdown(&self) -> String {
        let b = &self.failure_breakdown.baseline;
        let c = &self.failure_breakdown.candidate;
        let unit = if self.sprt_variant == "trinomial" {
            "belo"
        } else {
            "elo"
        };
        let mut markdown = format!(
            "# Veridict SPRT Report\n\n\
             Verdict: {verdict}\n\
             Validity: {validity}\n\
             Promotion: {promotion}\n\n\
             H0: {unit} <= {elo0:+.1} / H1: {unit} >= {elo1:+.1} (alpha={alpha}, beta={beta})\n\
             Decision LLR: {decision_llr:.4} (bounds: {lower:.4} to {upper:.4})\n\
             Full-input LLR: {llr:.4}\n\
             {drawelo_line}\
             {reason}\n\n\
             Schedule: {analyzed_observations} of {available_observations} {analysis_unit}(s) analyzed; stop={stopping_reason}; ignored={ignored_observations} {analysis_unit}(s) / {ignored_records} raw record(s)\n\
             Decision outcomes: candidate_wins={decision_candidate_wins}, baseline_wins={decision_baseline_wins}, draws={decision_draws}\n\
             Full-input outcomes: candidate_wins={candidate_wins}, baseline_wins={baseline_wins}, draws={draws}\n\
             {pentanomial_line}\n\
             Status counts:\n\
             - timeout: {timeouts} (baseline={b_timeout}, candidate={c_timeout})\n\
             - crash: {crashes} (baseline={b_crash}, candidate={c_crash})\n\
             - invalid: {invalid} (baseline={b_invalid}, candidate={c_invalid})\n",
            verdict = serde_str(&self.verdict),
            validity = serde_str(&self.validity),
            promotion = serde_str(&self.promotion),
            elo0 = self.elo0,
            elo1 = self.elo1,
            alpha = self.alpha,
            beta = self.beta,
            decision_llr = self.decision_llr,
            llr = self.llr,
            lower = self.lower_bound,
            upper = self.upper_bound,
            drawelo_line = match (self.decision_drawelo, self.drawelo) {
                (Some(decision), Some(full)) => format!(
                    "Decision-prefix drawelo: {decision:+.1}\nFull-input drawelo: {full:+.1}\n"
                ),
                _ => String::new(),
            },
            reason = self.reason,
            analyzed_observations = self.analyzed_observation_count,
            available_observations = self.available_observation_count,
            analysis_unit = self.analysis_unit,
            stopping_reason = self.stopping_observation_reason,
            ignored_observations = self.ignored_observations_after_stop,
            ignored_records = self.ignored_records_after_stop,
            decision_candidate_wins = self.decision_candidate_wins,
            decision_baseline_wins = self.decision_baseline_wins,
            decision_draws = self.decision_draws,
            candidate_wins = self.candidate_wins,
            baseline_wins = self.baseline_wins,
            draws = self.draws,
            pentanomial_line = match &self.pentanomial_counts {
                Some(p) => {
                    let ignored = self.ignored_pairs_after_stop.unwrap_or(0);
                    let stopped_early =
                        format!(" ({ignored} pair(s) after the stopping point ignored)");
                    format!(
                        "\nPentanomial pairs ({} analyzed of {} available, {} raw trials): \
                         0-0={} 0.5-0={} 1-1={} 1.5-0.5={} 2-0={}{}\n",
                        self.paired_count.unwrap_or(0),
                        self.available_paired_count.unwrap_or(0),
                        self.raw_trial_count.unwrap_or(0),
                        p.score_0_0,
                        p.score_0_5,
                        p.score_1_0,
                        p.score_1_5,
                        p.score_2_0,
                        if ignored > 0 {
                            stopped_early
                        } else {
                            String::new()
                        },
                    )
                }
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
        );
        append_markdown_warnings(&mut markdown, &self.warnings);
        markdown
    }
}
