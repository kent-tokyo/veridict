//! Core statistical decision library. Domain-agnostic: consumes baseline vs
//! candidate trial data and returns a pass/fail/inconclusive verdict with
//! enough detail to explain why. No stdout/stderr side effects; that is the
//! CLI's job (see `main.rs`).

pub mod error;
pub mod input;
pub mod matrix;
pub mod metrics;
pub mod plan;
pub mod report;
pub mod sprt;
pub mod stats;
pub mod verdict;

pub use error::VeridictError;
pub use report::{MultiReport, Report};

use serde::Serialize;

/// Final decision returned for a comparison run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Pass,
    Fail,
    Inconclusive,
}

/// Health of a single trial's execution, independent of any score it produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrialStatus {
    Ok,
    Timeout,
    Crash,
    Invalid,
}

impl TrialStatus {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "ok" => Some(Self::Ok),
            "timeout" => Some(Self::Timeout),
            "crash" => Some(Self::Crash),
            "invalid" => Some(Self::Invalid),
            _ => None,
        }
    }
}

/// Result of a single win/loss/draw comparison.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    BaselineWin,
    CandidateWin,
    Draw,
}

impl Outcome {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "baseline_win" => Some(Self::BaselineWin),
            "candidate_win" => Some(Self::CandidateWin),
            "draw" => Some(Self::Draw),
            _ => None,
        }
    }
}

/// Result of a single named-competitor match (see `input::MatchRecord`,
/// `matrix --matches`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchOutcome {
    AWin,
    BWin,
    Draw,
}

impl MatchOutcome {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "a_win" => Some(Self::AWin),
            "b_win" => Some(Self::BWin),
            "draw" => Some(Self::Draw),
            _ => None,
        }
    }
}

/// Which statistical method computed the effect size and confidence interval.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum MetricKind {
    #[serde(rename = "winrate")]
    WinRate,
    #[serde(rename = "mean-diff")]
    MeanDiff,
    #[serde(rename = "sign-test")]
    SignTest,
    #[serde(rename = "elo")]
    Elo,
}

/// Which confidence-interval method `winrate`/`sign-test` use. `Exact`
/// (Clopper-Pearson) and `Jeffreys` don't apply to `elo` (fractional
/// successes) or `mean-diff` (not a binomial proportion at all) - both are
/// derived from a true Beta-Binomial model, so requesting either for those
/// metrics is a config error (`VeridictError::IncompatibleCiMethod`), not a
/// silent fallback to `Wilson`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CiMethod {
    Wilson,
    Exact,
    Jeffreys,
}

/// Which bootstrap variant `mean-diff` uses. `Percentile` is the default
/// (unchanged from before this existed, so existing output doesn't shift);
/// `Bca` corrects for bias and skewness at the cost of a little extra
/// computation (a jackknife pass, still O(n)); `Basic` reflects the
/// percentile interval around the point estimate - simpler than `Bca`, but
/// with no bias-correction of its own (see `stats::bootstrap`'s doc).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootstrapMethod {
    Percentile,
    Bca,
    Basic,
}

/// How a failed trial (`baseline_status`/`candidate_status` other than `ok`) affects a
/// win/loss/draw-shaped metric (`winrate`/`elo`) or `sprt`. `ReportOnly` (default) is exactly
/// today's existing behavior, unchanged: a failure is still tallied into `failure_breakdown`,
/// but never itself contributes an outcome - only a literal `result` field does, and a status-
/// only record (the common case) already contributes nothing today regardless of this enum.
/// `Exclude`/`Loss` only diverge from `ReportOnly` in the less common case of a record carrying
/// *both* a failure status and a `result` (the schema doesn't forbid this combination). Only
/// meaningful for outcome-based metrics - `mean-diff`/`sign-test` have no win/loss/draw outcome
/// for a failed numeric trial to become, so requesting `Exclude`/`Loss` with either is a config
/// error (`VeridictError::IncompatibleFailurePolicy`), not an arbitrary numeric penalty.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailurePolicy {
    ReportOnly,
    Exclude,
    Loss,
}

/// Which knob(s) a metric actually uses, replacing the `(MetricKind,
/// CiMethod, BootstrapMethod)` trio `compare_one`/`compare_many` used to take
/// as three independent parameters. `elo` reads neither `ci_method` nor
/// `bootstrap_method`, and `mean-diff` doesn't read `ci_method` - passing one
/// anyway used to be silently ignored or a runtime `IncompatibleCiMethod`
/// error. Carrying only the field(s) a metric actually reads makes an
/// invalid pairing a compile error instead of a runtime one.
/// `MetricKind`-keyed code (`Report.metric`, `build_report`,
/// `estimate_additional_trials`) is unchanged - call [`MetricConfig::kind`]
/// to recover it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetricConfig {
    WinRate {
        ci_method: CiMethod,
        failure_policy: FailurePolicy,
    },
    SignTest {
        ci_method: CiMethod,
    },
    MeanDiff {
        bootstrap_method: BootstrapMethod,
    },
    Elo {
        failure_policy: FailurePolicy,
    },
}

impl MetricConfig {
    /// The plain metric identity, for report labeling/serialization - every
    /// variant maps to exactly one `MetricKind`, so this never fails.
    pub fn kind(&self) -> MetricKind {
        match self {
            Self::WinRate { .. } => MetricKind::WinRate,
            Self::SignTest { .. } => MetricKind::SignTest,
            Self::MeanDiff { .. } => MetricKind::MeanDiff,
            Self::Elo { .. } => MetricKind::Elo,
        }
    }

    /// Builds a validated config from flat, CLI-flag-shaped inputs - the
    /// same compatibility checks `compute_many` used to run on every call
    /// (`ci_method` other than `Wilson` is only valid for `WinRate`/
    /// `SignTest`; `failure_policy` other than `ReportOnly` is only valid
    /// for `WinRate`/`Elo`), now run once at construction instead of once
    /// per call. A caller that already knows which knobs its metric needs
    /// can construct a variant directly (e.g. `MetricConfig::MeanDiff { .. }`)
    /// and skip this - it's valid by construction, no runtime check needed.
    pub fn new(
        kind: MetricKind,
        ci_method: CiMethod,
        bootstrap_method: BootstrapMethod,
        failure_policy: FailurePolicy,
    ) -> Result<Self, VeridictError> {
        match kind {
            MetricKind::WinRate => Ok(Self::WinRate {
                ci_method,
                failure_policy,
            }),
            MetricKind::SignTest | MetricKind::MeanDiff
                if failure_policy != FailurePolicy::ReportOnly =>
            {
                Err(VeridictError::IncompatibleFailurePolicy {
                    policy: metrics::failure_policy_label(failure_policy),
                    metric: metrics::metric_label(kind),
                })
            }
            MetricKind::MeanDiff | MetricKind::Elo if ci_method != CiMethod::Wilson => {
                Err(VeridictError::IncompatibleCiMethod {
                    method: metrics::ci_method_label(ci_method),
                    metric: metrics::metric_label(kind),
                })
            }
            MetricKind::SignTest => Ok(Self::SignTest { ci_method }),
            MetricKind::MeanDiff => Ok(Self::MeanDiff { bootstrap_method }),
            MetricKind::Elo => Ok(Self::Elo { failure_policy }),
        }
    }

    /// The `CiMethod` to feed `build_report`/`estimate_additional_trials` -
    /// only ever actually read there for `WinRate`/`SignTest`
    /// (`estimate_additional_trials` hardcodes its own Wilson-based branch
    /// for `Elo` before this value would be read, and returns early for
    /// `MeanDiff` before it too - see `verdict::estimate_additional_trials`).
    /// `Wilson` here for `MeanDiff`/`Elo` is a safe placeholder, not a real
    /// choice being made on their behalf.
    fn ci_method(&self) -> CiMethod {
        match self {
            Self::WinRate { ci_method, .. } | Self::SignTest { ci_method } => *ci_method,
            Self::MeanDiff { .. } | Self::Elo { .. } => CiMethod::Wilson,
        }
    }
}

/// Lets `compare_one`/`compare_many` (and `metrics::compute`/
/// `metrics::compute_many`) accept either a streaming `Result`-yielding
/// iterator (the CLI's real use case - parsing JSONL/CSV can fail mid-
/// stream) or a plain iterator over already-valid `(usize, Record)` pairs (a
/// caller that already has a validated slice/`Vec` in memory, with no
/// `Result` to thread through) through the *same* function, instead of
/// forcing every in-memory caller to write `.map(Ok)` just to satisfy the
/// type. A blanket `impl<T> From<T> for Result<T, E>` isn't available here
/// (implementing a foreign trait, `From`, for a foreign type, `Result` -
/// even parameterized by a local type - violates Rust's orphan rule), hence
/// this small local trait instead.
pub trait IntoRecordResult {
    fn into_record_result(self) -> Result<(usize, input::Record), VeridictError>;
}

impl IntoRecordResult for Result<(usize, input::Record), VeridictError> {
    fn into_record_result(self) -> Self {
        self
    }
}

impl IntoRecordResult for (usize, input::Record) {
    fn into_record_result(self) -> Result<(usize, input::Record), VeridictError> {
        Ok(self)
    }
}

/// Runs one metric end to end: classify records, compute its effect and
/// confidence interval, and apply the pass/fail thresholds. `paired_by_id`
/// enables paired-testcase variance reduction (see `metrics::compute`).
pub fn compare_one<I>(
    records: I,
    metric: MetricConfig,
    confidence: f64,
    thresholds: &verdict::Thresholds,
    resamples: usize,
    seed: u64,
    paired_by_id: bool,
) -> Result<Report, VeridictError>
where
    I: IntoIterator,
    I::Item: IntoRecordResult,
{
    let out = metrics::compute(records, metric, confidence, resamples, seed, paired_by_id)?;
    Ok(build_report(
        metric.kind(),
        confidence,
        thresholds,
        out,
        metric.ci_method(),
    ))
}

/// Runs several metrics against the same records and thresholds in a single
/// pass over `records` (see `metrics::compute_many`), and combines them into
/// one overall verdict: `Fail` if any metric fails, else `Inconclusive` if
/// any metric is inconclusive, else `Pass`. Matches the "a false pass is
/// worse than an inconclusive result" rule: one metric failing sinks the
/// whole run.
pub fn compare_many<I>(
    records: I,
    metrics: &[MetricConfig],
    confidence: f64,
    thresholds: &verdict::Thresholds,
    resamples: usize,
    seed: u64,
    paired_by_id: bool,
) -> Result<MultiReport, VeridictError>
where
    I: IntoIterator,
    I::Item: IntoRecordResult,
{
    let outs = metrics::compute_many(records, metrics, confidence, resamples, seed, paired_by_id)?;
    let reports: Vec<Report> = metrics
        .iter()
        .zip(outs)
        .map(|(&config, out)| {
            build_report(
                config.kind(),
                confidence,
                thresholds,
                out,
                config.ci_method(),
            )
        })
        .collect();
    let verdict = verdict::aggregate(reports.iter().map(|r| r.verdict));
    Ok(MultiReport {
        schema_version: report::REPORT_SCHEMA_VERSION,
        verdict,
        reports,
    })
}

fn build_report(
    metric: MetricKind,
    confidence: f64,
    thresholds: &verdict::Thresholds,
    out: metrics::MetricOutput,
    ci_method: CiMethod,
) -> Report {
    // Zero usable trials means "no signal", not "the CLI ran a threshold
    // check on a fabricated zero": force Inconclusive rather than letting
    // (0.0, 0.0) accidentally satisfy a threshold that includes zero.
    let (verdict, reason) = match &out.warning {
        Some(warning) => (Verdict::Inconclusive, warning.clone()),
        None => verdict::decide(out.ci_low, out.ci_high, thresholds),
    };
    let estimated_additional_trials = verdict::estimate_additional_trials(
        metric,
        ci_method,
        verdict,
        out.effect,
        out.ci_low,
        out.ci_high,
        out.paired_count,
        thresholds,
        confidence,
    );
    let (data_quality, warnings) = collect_data_quality(metric, &out);

    Report {
        schema_version: report::REPORT_SCHEMA_VERSION,
        verdict,
        metric,
        baseline_count: out.baseline_count,
        candidate_count: out.candidate_count,
        paired_count: out.paired_count,
        effect: out.effect,
        confidence,
        ci_low: out.ci_low,
        ci_high: out.ci_high,
        pass_above: thresholds.pass_above,
        fail_below: thresholds.fail_below,
        timeouts: out.timeouts,
        crashes: out.crashes,
        invalid: out.invalid,
        failure_breakdown: out.failures,
        reason,
        estimated_additional_trials,
        warnings,
        data_quality,
    }
}

/// Advisory, verdict-independent data-quality flags and their human-readable
/// counterpart, computed together from the same rates/counts so the two
/// representations can't drift out of sync with each other. Kept separate
/// from `MetricOutput.warning` (which forces `Inconclusive` on zero usable
/// trials, a real verdict-changing decision) - these never affect `verdict`.
fn collect_data_quality(
    metric: MetricKind,
    out: &metrics::MetricOutput,
) -> (report::DataQuality, Vec<String>) {
    let mut quality = report::DataQuality::default();
    let mut warnings = Vec::new();

    quality.tiny_sample = out.paired_count < 30;
    if quality.tiny_sample {
        warnings.push(format!(
            "small sample: {} paired trial(s), below the conventional 30-trial threshold for confidence-interval methods to be reliable",
            out.paired_count
        ));
    }

    // ponytail: this treats `paired_count` and the failure counts as disjoint - true under
    // `FailurePolicy::ReportOnly`/`Exclude` for the common status-only-record case, but a record
    // carrying both a failure status and a counted outcome (a mixed status+result record under
    // `ReportOnly`, or *any* failure under `Loss`, whose synthesized outcome lands in
    // `paired_count` too) is double-counted here: once as a failure, once as a trial. This can
    // under-report `high_failure_rate` for a true failure rate a little above 20% (e.g. true 22%
    // reports as ~18%, a real miss). Advisory-only - never affects `verdict` - and the practical
    // miss window is narrow (only near the 20% boundary; far above or below it the discount
    // doesn't change which side of 20% it lands on). Fix properly if this bites in practice: track
    // "outcome came from a failure" separately per aggregator and exclude it from `paired_count`
    // here, rather than trying to disentangle it from this already-summed total.
    let total_trials = out.paired_count + out.timeouts + out.crashes + out.invalid;
    if total_trials > 0 {
        let failure_rate = (out.timeouts + out.crashes + out.invalid) as f64 / total_trials as f64;
        quality.high_failure_rate = failure_rate > 0.2;
        if quality.high_failure_rate {
            warnings.push(format!(
                "{:.0}% of trials failed to execute (timeout/crash/invalid) rather than producing a usable result",
                failure_rate * 100.0
            ));
        }
    }

    // winrate/sign-test discard their tie/draw count before it reaches
    // MetricOutput, so extending this warning to them would need a new
    // tracked field - deferred, not silently dropped.
    if metric == MetricKind::Elo && out.paired_count > 0 {
        let draws = out
            .paired_count
            .saturating_sub(out.baseline_count + out.candidate_count);
        let draw_rate = draws as f64 / out.paired_count as f64;
        quality.draw_heavy = draw_rate > 0.5;
        if quality.draw_heavy {
            warnings.push(format!(
                "{:.0}% of trials were draws, leaving few decisive outcomes to estimate Elo from",
                draw_rate * 100.0
            ));
        }
    }

    // Deliberately guarded by !tiny_sample - see DataQuality's doc comment
    // for why a wide CI from a tiny sample shouldn't also trip this.
    quality.effect_within_noise_floor =
        !quality.tiny_sample && out.effect.abs() < (out.ci_high - out.ci_low) / 2.0;
    if quality.effect_within_noise_floor {
        warnings.push(
            "the measured effect is smaller than the CI's own half-width: it could plausibly be noise around zero, even though the sample isn't tiny"
                .to_string(),
        );
    }

    // records_with_id/max_id_count are 0 when --paired-by-id is set - paired
    // mode already has its own meaning for a repeated id, so this is skipped
    // there (see MetricOutput's doc). >= 3 (not >= 2) is load-bearing:
    // someone who simply forgot --paired-by-id on genuinely paired data has
    // every id at exactly 2, and that must stay silent - firing on it would
    // be noise on a common, innocent mistake. >= 10 is a floor so the signal
    // isn't computed from a handful of records.
    quality.low_id_diversity = out.records_with_id >= 10 && out.max_id_count >= 3;
    if quality.low_id_diversity {
        warnings.push(format!(
            "low id diversity: one id repeated {} times among {} id-tagged trial(s) - looks like the same test case was logged multiple times, not independent samples",
            out.max_id_count, out.records_with_id
        ));
    }

    (quality, warnings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::Record;
    use crate::stats::bootstrap::DEFAULT_SEED;
    use crate::verdict::Thresholds;

    fn rec(
        id: &str,
        baseline: Option<f64>,
        candidate: Option<f64>,
        result: Option<&str>,
        baseline_status: Option<&str>,
        candidate_status: Option<&str>,
    ) -> Record {
        Record {
            id: Some(id.to_string()),
            baseline,
            candidate,
            result: result.map(str::to_string),
            baseline_status: baseline_status.map(str::to_string),
            candidate_status: candidate_status.map(str::to_string),
        }
    }

    #[test]
    fn end_to_end_winrate_pass() {
        let mut records = Vec::new();
        for i in 0..80 {
            records.push((
                i + 1,
                rec(
                    &format!("c{i}"),
                    None,
                    None,
                    Some("candidate_win"),
                    None,
                    None,
                ),
            ));
        }
        for i in 0..20 {
            records.push((
                80 + i + 1,
                rec(
                    &format!("b{i}"),
                    None,
                    None,
                    Some("baseline_win"),
                    None,
                    None,
                ),
            ));
        }
        let thresholds = Thresholds::symmetric(0.02).unwrap();
        let report = compare_one(
            records.iter().cloned(),
            MetricConfig::WinRate {
                ci_method: CiMethod::Wilson,
                failure_policy: FailurePolicy::ReportOnly,
            },
            0.95,
            &thresholds,
            2000,
            DEFAULT_SEED,
            false,
        )
        .unwrap();
        assert_eq!(report.verdict, Verdict::Pass);
        assert_eq!(report.candidate_count, 80);
        assert_eq!(report.baseline_count, 20);
    }

    #[test]
    fn end_to_end_mean_diff_inconclusive_on_tiny_sample() {
        let records = [
            (1, rec("a", Some(1.0), Some(1.1), None, None, None)),
            (2, rec("b", Some(2.0), Some(1.9), None, None, None)),
        ];
        let thresholds = Thresholds::symmetric(0.02).unwrap();
        let report = compare_one(
            records.iter().cloned(),
            MetricConfig::MeanDiff {
                bootstrap_method: BootstrapMethod::Percentile,
            },
            0.95,
            &thresholds,
            2000,
            DEFAULT_SEED,
            false,
        )
        .unwrap();
        assert_eq!(report.verdict, Verdict::Inconclusive);
    }

    #[test]
    fn zero_usable_trials_is_inconclusive_not_error() {
        let records = [(1, rec("a", None, None, None, Some("timeout"), None))];
        let thresholds = Thresholds::symmetric(0.02).unwrap();
        let report = compare_one(
            records.iter().cloned(),
            MetricConfig::WinRate {
                ci_method: CiMethod::Wilson,
                failure_policy: FailurePolicy::ReportOnly,
            },
            0.95,
            &thresholds,
            2000,
            DEFAULT_SEED,
            false,
        )
        .unwrap();
        assert_eq!(report.verdict, Verdict::Inconclusive);
        assert_eq!(report.timeouts, 1);
    }

    #[test]
    fn empty_input_is_an_error() {
        let records: Vec<(usize, Record)> = Vec::new();
        let thresholds = Thresholds::symmetric(0.02).unwrap();
        let result = compare_one(
            records.iter().cloned(),
            MetricConfig::WinRate {
                ci_method: CiMethod::Wilson,
                failure_policy: FailurePolicy::ReportOnly,
            },
            0.95,
            &thresholds,
            2000,
            DEFAULT_SEED,
            false,
        );
        assert!(matches!(result, Err(VeridictError::EmptyInput)));
    }

    #[test]
    fn compare_many_passes_overall_when_every_metric_passes() {
        let records: Vec<_> = (0..20)
            .map(|i| {
                (
                    i + 1,
                    rec(
                        &format!("r{i}"),
                        Some(1.0),
                        Some(2.0),
                        Some("candidate_win"),
                        None,
                        None,
                    ),
                )
            })
            .collect();
        let thresholds = Thresholds::symmetric(0.1).unwrap();
        let report = compare_many(
            records.iter().cloned(),
            &[
                MetricConfig::WinRate {
                    ci_method: CiMethod::Wilson,
                    failure_policy: FailurePolicy::ReportOnly,
                },
                MetricConfig::MeanDiff {
                    bootstrap_method: BootstrapMethod::Percentile,
                },
            ],
            0.95,
            &thresholds,
            2000,
            DEFAULT_SEED,
            false,
        )
        .unwrap();
        assert_eq!(report.verdict, Verdict::Pass);
        assert_eq!(report.reports.len(), 2);
    }

    #[test]
    fn compare_many_fails_overall_if_any_metric_fails() {
        // Each record carries both fields: result says the candidate always
        // loses (winrate -> Fail), but the numeric score always favors the
        // candidate (mean-diff -> Pass). Fail must dominate the aggregate.
        let records: Vec<_> = (0..20)
            .map(|i| {
                (
                    i + 1,
                    rec(
                        &format!("r{i}"),
                        Some(1.0),
                        Some(2.0),
                        Some("baseline_win"),
                        None,
                        None,
                    ),
                )
            })
            .collect();
        let thresholds = Thresholds::symmetric(0.1).unwrap();
        let report = compare_many(
            records.iter().cloned(),
            &[
                MetricConfig::WinRate {
                    ci_method: CiMethod::Wilson,
                    failure_policy: FailurePolicy::ReportOnly,
                },
                MetricConfig::MeanDiff {
                    bootstrap_method: BootstrapMethod::Percentile,
                },
            ],
            0.95,
            &thresholds,
            2000,
            DEFAULT_SEED,
            false,
        )
        .unwrap();
        assert_eq!(report.reports[0].verdict, Verdict::Fail);
        assert_eq!(report.reports[1].verdict, Verdict::Pass);
        assert_eq!(report.verdict, Verdict::Fail);
    }

    // --- Report.warnings ---

    #[test]
    fn tiny_sample_produces_a_warning() {
        let records: Vec<_> = (0..10)
            .map(|i| {
                (
                    i + 1,
                    rec(
                        &format!("r{i}"),
                        None,
                        None,
                        Some("candidate_win"),
                        None,
                        None,
                    ),
                )
            })
            .collect();
        let thresholds = Thresholds::symmetric(0.0).unwrap();
        let report = compare_one(
            records.iter().cloned(),
            MetricConfig::WinRate {
                ci_method: CiMethod::Wilson,
                failure_policy: FailurePolicy::ReportOnly,
            },
            0.95,
            &thresholds,
            2000,
            DEFAULT_SEED,
            false,
        )
        .unwrap();
        assert!(report.warnings.iter().any(|w| w.contains("small sample")));
        assert!(report.data_quality.tiny_sample);
        // A wide CI from a tiny sample shouldn't ALSO trip the noise-floor
        // flag - see DataQuality's doc comment.
        assert!(!report.data_quality.effect_within_noise_floor);
    }

    // --- low_id_diversity: the case table that actually matters here ---

    fn winrate_records_with_ids(ids: &[String]) -> Vec<(usize, Record)> {
        ids.iter()
            .enumerate()
            .map(|(i, id)| {
                (
                    i + 1,
                    rec(id, None, None, Some("candidate_win"), None, None),
                )
            })
            .collect()
    }

    #[test]
    fn id_diversity_healthy_mostly_unique_ids_is_silent() {
        let ids: Vec<String> = (0..12).map(|i| format!("r{i}")).collect();
        let records = winrate_records_with_ids(&ids);
        let thresholds = Thresholds::symmetric(0.0).unwrap();
        let report = compare_one(
            records.iter().cloned(),
            MetricConfig::WinRate {
                ci_method: CiMethod::Wilson,
                failure_policy: FailurePolicy::ReportOnly,
            },
            0.95,
            &thresholds,
            2000,
            DEFAULT_SEED,
            false,
        )
        .unwrap();
        assert!(!report.data_quality.low_id_diversity);
    }

    #[test]
    fn id_diversity_every_id_exactly_twice_is_silent() {
        // The common, innocent mistake: genuinely paired data run without
        // --paired-by-id. Every id at exactly 2 must NOT fire - that would
        // be noise on a case this ordinary, not a real diversity problem.
        let mut ids = Vec::new();
        for i in 0..6 {
            ids.push(format!("pair{i}"));
            ids.push(format!("pair{i}"));
        }
        let records = winrate_records_with_ids(&ids);
        let thresholds = Thresholds::symmetric(0.0).unwrap();
        let report = compare_one(
            records.iter().cloned(),
            MetricConfig::WinRate {
                ci_method: CiMethod::Wilson,
                failure_policy: FailurePolicy::ReportOnly,
            },
            0.95,
            &thresholds,
            2000,
            DEFAULT_SEED,
            false,
        )
        .unwrap();
        assert!(!report.data_quality.low_id_diversity);
    }

    #[test]
    fn id_diversity_one_dominant_id_fires() {
        let mut ids = vec!["dup".to_string(); 5];
        ids.extend((0..7).map(|i| format!("r{i}")));
        let records = winrate_records_with_ids(&ids);
        let thresholds = Thresholds::symmetric(0.0).unwrap();
        let report = compare_one(
            records.iter().cloned(),
            MetricConfig::WinRate {
                ci_method: CiMethod::Wilson,
                failure_policy: FailurePolicy::ReportOnly,
            },
            0.95,
            &thresholds,
            2000,
            DEFAULT_SEED,
            false,
        )
        .unwrap();
        assert!(report.data_quality.low_id_diversity);
        assert!(
            report
                .warnings
                .iter()
                .any(|w| w.contains("low id diversity"))
        );
    }

    #[test]
    fn id_diversity_skipped_entirely_under_paired_by_id() {
        // Same shape as the exactly-twice case, but paired mode: repeated
        // ids mean something different there (net to one observation), so
        // this tracking must stay at its 0 sentinel, not fire.
        let mut ids = Vec::new();
        for i in 0..6 {
            ids.push(format!("pair{i}"));
            ids.push(format!("pair{i}"));
        }
        let records = winrate_records_with_ids(&ids);
        let thresholds = Thresholds::symmetric(0.0).unwrap();
        let report = compare_one(
            records.iter().cloned(),
            MetricConfig::WinRate {
                ci_method: CiMethod::Wilson,
                failure_policy: FailurePolicy::ReportOnly,
            },
            0.95,
            &thresholds,
            2000,
            DEFAULT_SEED,
            true,
        )
        .unwrap();
        assert!(!report.data_quality.low_id_diversity);
    }

    #[test]
    fn id_diversity_below_the_floor_is_silent_even_if_skewed() {
        // Same skew as the dominant-id case (one id x4), but only 6
        // id-tagged records total - below the >= 10 floor, so the signal
        // isn't meaningful enough to report yet.
        let mut ids = vec!["dup".to_string(); 4];
        ids.extend((0..2).map(|i| format!("r{i}")));
        let records = winrate_records_with_ids(&ids);
        let thresholds = Thresholds::symmetric(0.0).unwrap();
        let report = compare_one(
            records.iter().cloned(),
            MetricConfig::WinRate {
                ci_method: CiMethod::Wilson,
                failure_policy: FailurePolicy::ReportOnly,
            },
            0.95,
            &thresholds,
            2000,
            DEFAULT_SEED,
            false,
        )
        .unwrap();
        assert!(!report.data_quality.low_id_diversity);
    }

    #[test]
    fn excessive_failures_produce_a_warning() {
        let mut records: Vec<_> = (0..30)
            .map(|i| {
                (
                    i + 1,
                    rec(
                        &format!("r{i}"),
                        None,
                        None,
                        Some("candidate_win"),
                        None,
                        None,
                    ),
                )
            })
            .collect();
        for i in 0..8 {
            records.push((
                31 + i,
                rec(&format!("t{i}"), None, None, None, Some("timeout"), None),
            ));
        }
        let thresholds = Thresholds::symmetric(0.0).unwrap();
        let report = compare_one(
            records.iter().cloned(),
            MetricConfig::WinRate {
                ci_method: CiMethod::Wilson,
                failure_policy: FailurePolicy::ReportOnly,
            },
            0.95,
            &thresholds,
            2000,
            DEFAULT_SEED,
            false,
        )
        .unwrap();
        assert_eq!(report.paired_count, 30);
        assert!(!report.warnings.iter().any(|w| w.contains("small sample")));
        assert!(
            report
                .warnings
                .iter()
                .any(|w| w.contains("failed to execute"))
        );
    }

    #[test]
    fn excessive_draws_produce_a_warning_for_elo() {
        let mut records = Vec::new();
        for i in 0..3 {
            records.push((
                i + 1,
                rec(
                    &format!("c{i}"),
                    None,
                    None,
                    Some("candidate_win"),
                    None,
                    None,
                ),
            ));
        }
        for i in 0..2 {
            records.push((
                4 + i,
                rec(
                    &format!("b{i}"),
                    None,
                    None,
                    Some("baseline_win"),
                    None,
                    None,
                ),
            ));
        }
        for i in 0..6 {
            records.push((
                6 + i,
                rec(&format!("d{i}"), None, None, Some("draw"), None, None),
            ));
        }
        let thresholds = Thresholds::symmetric(0.0).unwrap();
        let report = compare_one(
            records.iter().cloned(),
            MetricConfig::Elo {
                failure_policy: FailurePolicy::ReportOnly,
            },
            0.95,
            &thresholds,
            2000,
            DEFAULT_SEED,
            false,
        )
        .unwrap();
        assert!(report.warnings.iter().any(|w| w.contains("draws")));
    }

    #[test]
    fn clean_large_sample_has_no_warnings() {
        let records: Vec<_> = (0..40)
            .map(|i| {
                (
                    i + 1,
                    rec(
                        &format!("r{i}"),
                        None,
                        None,
                        Some("candidate_win"),
                        None,
                        None,
                    ),
                )
            })
            .collect();
        let thresholds = Thresholds::symmetric(0.0).unwrap();
        let report = compare_one(
            records.iter().cloned(),
            MetricConfig::WinRate {
                ci_method: CiMethod::Wilson,
                failure_policy: FailurePolicy::ReportOnly,
            },
            0.95,
            &thresholds,
            2000,
            DEFAULT_SEED,
            false,
        )
        .unwrap();
        assert!(report.warnings.is_empty());
        assert_eq!(report.data_quality, report::DataQuality::default());
    }

    #[test]
    fn noise_floor_flag_fires_on_a_large_but_swamped_effect() {
        // n=40 (not tiny), but a near-50/50 split leaves the effect (0.025)
        // far smaller than the CI's own half-width (~0.148) - independently
        // verified against a direct Wilson recompute.
        let mut records: Vec<_> = (0..21)
            .map(|i| {
                (
                    i + 1,
                    rec(
                        &format!("c{i}"),
                        None,
                        None,
                        Some("candidate_win"),
                        None,
                        None,
                    ),
                )
            })
            .collect();
        records.extend((0..19).map(|i| {
            (
                22 + i,
                rec(
                    &format!("b{i}"),
                    None,
                    None,
                    Some("baseline_win"),
                    None,
                    None,
                ),
            )
        }));
        let thresholds = Thresholds::symmetric(0.0).unwrap();
        let report = compare_one(
            records.iter().cloned(),
            MetricConfig::WinRate {
                ci_method: CiMethod::Wilson,
                failure_policy: FailurePolicy::ReportOnly,
            },
            0.95,
            &thresholds,
            2000,
            DEFAULT_SEED,
            false,
        )
        .unwrap();
        assert!(!report.data_quality.tiny_sample);
        assert!(report.data_quality.effect_within_noise_floor);
        assert!(
            report
                .warnings
                .iter()
                .any(|w| w.contains("smaller than the CI's own half-width"))
        );
    }
}
