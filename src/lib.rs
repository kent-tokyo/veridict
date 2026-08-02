//! Core statistical decision library. Domain-agnostic: consumes baseline vs
//! candidate trial data and returns a pass/fail/inconclusive verdict with
//! enough detail to explain why. No stdout/stderr side effects; that is the
//! CLI's job (see `main.rs`).

pub mod correction;
pub mod error;
pub mod input;
pub mod matrix;
pub mod metrics;
pub mod plan;
pub mod power;
pub mod report;
pub mod sprt;
pub mod stats;
pub mod time_sensitive;
pub mod verdict;
pub mod verify_run;

pub use error::VeridictError;
pub use report::{MultiReport, Report};

use serde::{Deserialize, Serialize};

/// Final decision returned for a comparison run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Pass,
    Fail,
    Inconclusive,
}

/// Sub-classifies a `Verdict::Inconclusive` result by whether its CI excludes zero - the two
/// situations read identically as `"inconclusive"` otherwise, but call for different next steps.
/// `None` (not this enum) covers every case where the split doesn't apply: `Pass`/`Fail`, an
/// `Inconclusive` caused by zero usable trials rather than a real CI, or one forced by a breached
/// `FailureCaps` - see `verdict::classify_inconclusive` and `build_report`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InconclusiveKind {
    /// The CI still straddles zero (`ci_low <= 0.0 <= ci_high`) - the sign of the effect itself
    /// is undetermined, indistinguishable from noise around zero.
    Noise,
    /// The CI excludes zero (both bounds share a sign) - a real, consistent-direction effect
    /// that simply doesn't clear the pass/fail threshold yet.
    Directional,
}

/// Whether a report's underlying data is trustworthy enough to read a
/// `Verdict` off of at all - independent of what that verdict says.
/// `Invalid` means a hard technical-failure cap (`FailureCaps`) was
/// breached: not "the evidence was weak" (that's `Inconclusive`, still
/// `Valid`), but "the run itself can't be trusted to have measured the
/// candidate at all." Keeping this a separate axis from `Verdict` is the
/// point: a `Pass`/`Fail` produced from data that also breached a failure
/// cap (possible under `--failure-policy loss`, where a crash can tip the
/// numeric verdict) must never be reported as a clean `Pass`/`Fail` - see
/// `verdict::apply_failure_caps`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Validity {
    Valid,
    Invalid,
}

/// The one field a deployment/promotion pipeline should actually gate on -
/// collapses `Validity` and `Verdict` into a single go/no-go: `Promoted`
/// only when the run is both `Validity::Valid` and `Verdict::Pass`. Every
/// other combination (invalid data, a fail, an inconclusive result) is
/// `NotPromoted`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Promotion {
    Promoted,
    NotPromoted,
}

impl Promotion {
    pub fn decide(validity: Validity, verdict: Verdict) -> Self {
        if validity == Validity::Valid && verdict == Verdict::Pass {
            Promotion::Promoted
        } else {
            Promotion::NotPromoted
        }
    }
}

/// Hard per-run caps on technical failure counts (`timeout`/`crash`/
/// `invalid` - the same categories `TrialStatus`/`FailureBreakdown` already
/// track; no new domain-specific categories). `None` (the `Default`) means
/// uncapped - today's existing behavior, unchanged, unless a cap is opted
/// into. These are zero-tolerance-style gates (e.g. `max_crashes = 0`), not
/// a rate threshold: a single technical failure can matter regardless of
/// how many clean trials surround it, unlike `data_quality.high_failure_rate`
/// (a rate-based, purely advisory warning that never changes `verdict`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FailureCaps {
    pub max_timeouts: Option<u64>,
    pub max_crashes: Option<u64>,
    pub max_invalid: Option<u64>,
}

impl FailureCaps {
    /// `Valid` with no reason if every configured cap is respected (always
    /// true when every cap is `None`); otherwise `Invalid` with a reason
    /// naming the first breached cap, checked in timeout/crash/invalid
    /// order.
    pub fn check(&self, timeouts: u64, crashes: u64, invalid: u64) -> (Validity, Option<String>) {
        if let Some(max) = self.max_timeouts
            && timeouts > max
        {
            return (
                Validity::Invalid,
                Some(format!(
                    "{timeouts} timeout(s) exceeds the configured cap of {max}"
                )),
            );
        }
        if let Some(max) = self.max_crashes
            && crashes > max
        {
            return (
                Validity::Invalid,
                Some(format!(
                    "{crashes} crash(es) exceeds the configured cap of {max}"
                )),
            );
        }
        if let Some(max) = self.max_invalid
            && invalid > max
        {
            return (
                Validity::Invalid,
                Some(format!(
                    "{invalid} invalid result(s) exceeds the configured cap of {max}"
                )),
            );
        }
        (Validity::Valid, None)
    }
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
    #[serde(rename = "quantile-diff")]
    QuantileDiff,
    /// `(candidate - baseline) / baseline`, bootstrapped the same way `mean-diff` bootstraps
    /// `candidate - baseline` - a distinct metric, not a `mean-diff` display mode: it answers "what
    /// proportional change" rather than "what absolute change," needs `baseline > 0` (an absolute
    /// difference has no such constraint), and reports in ratio units, not the input's own units.
    /// Independent `MetricConfig` variant (not a `--relative` flag on `MeanDiff`) so the two can
    /// run side by side in one multi-metric invocation and so each metric's own JSON/Markdown
    /// rendering, power/claim-correction support, and baseline validation stay type-distinguished
    /// rather than branching on a bool at every use site - see `docs/metrics.md`.
    #[serde(rename = "relative-diff")]
    RelativeDiff,
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
/// to recover it. `PartialEq` only, not `Eq`: `QuantileDiff`'s `quantile: f64` field has no total
/// equality (`f64` isn't `Eq`), so the derive would fail across the whole enum otherwise.
#[derive(Debug, Clone, Copy, PartialEq)]
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
    /// `quantile` is always in `(0, 1)` (validated in `new`) - the sample min/max has no
    /// well-behaved bootstrap distribution, so those endpoints are rejected rather than allowed
    /// through. `bootstrap_method` is never `Bca` here (also validated in `new`) - see
    /// `VeridictError::IncompatibleBootstrapMethod`'s doc for why.
    QuantileDiff {
        quantile: f64,
        bootstrap_method: BootstrapMethod,
    },
    /// All three `--bootstrap-method` variants apply exactly as they do for `MeanDiff` - unlike
    /// `QuantileDiff`, the relative-diff sample mean is a smooth statistic, so BCa's jackknife
    /// acceleration term has the same solid footing here it has for `mean-diff`.
    RelativeDiff {
        bootstrap_method: BootstrapMethod,
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
            Self::QuantileDiff { .. } => MetricKind::QuantileDiff,
            Self::RelativeDiff { .. } => MetricKind::RelativeDiff,
        }
    }

    /// Builds a validated config from flat, CLI-flag-shaped inputs - the
    /// same compatibility checks `compute_many` used to run on every call
    /// (`ci_method` other than `Wilson` is only valid for `WinRate`/
    /// `SignTest`; `failure_policy` other than `ReportOnly` is only valid
    /// for `WinRate`/`Elo`; `Bca` isn't a valid `bootstrap_method` for
    /// `QuantileDiff` - see `VeridictError::IncompatibleBootstrapMethod`), now
    /// run once at construction instead of once per call. A caller that
    /// already knows which knobs its metric needs can construct a variant
    /// directly (e.g. `MetricConfig::MeanDiff { .. }`) and skip this - it's
    /// valid by construction, no runtime check needed.
    ///
    /// `quantile` for any kind other than `QuantileDiff` is silently unused,
    /// not a config error - same precedent as `bootstrap_method` for
    /// `WinRate`/`SignTest`/`Elo` (which don't read it either): it lets a
    /// single `compare --metric mean-diff --metric quantile-diff --quantile
    /// 0.9` invocation share one `--quantile` flag across the whole run
    /// without erroring on the metric that doesn't need it.
    pub fn new(
        kind: MetricKind,
        ci_method: CiMethod,
        bootstrap_method: BootstrapMethod,
        failure_policy: FailurePolicy,
        quantile: Option<f64>,
    ) -> Result<Self, VeridictError> {
        match kind {
            MetricKind::WinRate => Ok(Self::WinRate {
                ci_method,
                failure_policy,
            }),
            MetricKind::SignTest
            | MetricKind::MeanDiff
            | MetricKind::QuantileDiff
            | MetricKind::RelativeDiff
                if failure_policy != FailurePolicy::ReportOnly =>
            {
                Err(VeridictError::IncompatibleFailurePolicy {
                    policy: metrics::failure_policy_label(failure_policy),
                    metric: metrics::metric_label(kind),
                })
            }
            MetricKind::MeanDiff
            | MetricKind::Elo
            | MetricKind::QuantileDiff
            | MetricKind::RelativeDiff
                if ci_method != CiMethod::Wilson =>
            {
                Err(VeridictError::IncompatibleCiMethod {
                    method: metrics::ci_method_label(ci_method),
                    metric: metrics::metric_label(kind),
                })
            }
            MetricKind::QuantileDiff if bootstrap_method == BootstrapMethod::Bca => {
                Err(VeridictError::IncompatibleBootstrapMethod {
                    method: metrics::bootstrap_method_label(bootstrap_method),
                    metric: metrics::metric_label(kind),
                })
            }
            MetricKind::SignTest => Ok(Self::SignTest { ci_method }),
            MetricKind::MeanDiff => Ok(Self::MeanDiff { bootstrap_method }),
            MetricKind::Elo => Ok(Self::Elo { failure_policy }),
            MetricKind::QuantileDiff => {
                let q = quantile.unwrap_or(0.5);
                if !q.is_finite() || q <= 0.0 || q >= 1.0 {
                    return Err(VeridictError::InvalidQuantile(q));
                }
                Ok(Self::QuantileDiff {
                    quantile: q,
                    bootstrap_method,
                })
            }
            MetricKind::RelativeDiff => Ok(Self::RelativeDiff { bootstrap_method }),
        }
    }

    /// The `CiMethod` to feed `build_report`/`estimate_additional_trials` -
    /// only ever actually read there for `WinRate`/`SignTest`
    /// (`estimate_additional_trials` hardcodes its own Wilson-based branch
    /// for `Elo` before this value would be read, and returns early for
    /// `MeanDiff`/`QuantileDiff` before it too - see
    /// `verdict::estimate_additional_trials`). `Wilson` here for
    /// `MeanDiff`/`Elo`/`QuantileDiff` is a safe placeholder, not a real
    /// choice being made on their behalf.
    fn ci_method(&self) -> CiMethod {
        match self {
            Self::WinRate { ci_method, .. } | Self::SignTest { ci_method } => *ci_method,
            Self::MeanDiff { .. }
            | Self::Elo { .. }
            | Self::QuantileDiff { .. }
            | Self::RelativeDiff { .. } => CiMethod::Wilson,
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
/// `cluster_by_id` enables a cluster-robust bootstrap CI instead (winrate/elo
/// only - mutually exclusive with `paired_by_id`, see
/// `VeridictError::IncompatibleClusterById`/`ClusterByIdConflictsWithPairedById`).
#[allow(clippy::too_many_arguments)]
pub fn compare_one<I>(
    records: I,
    metric: MetricConfig,
    confidence: f64,
    thresholds: &verdict::Thresholds,
    resamples: usize,
    seed: u64,
    paired_by_id: bool,
    cluster_by_id: bool,
) -> Result<Report, VeridictError>
where
    I: IntoIterator,
    I::Item: IntoRecordResult,
{
    let out = metrics::compute(
        records,
        metric,
        confidence,
        resamples,
        seed,
        paired_by_id,
        cluster_by_id,
    )?;
    Ok(build_report(
        metric.kind(),
        confidence,
        thresholds,
        out,
        metric.ci_method(),
        paired_by_id,
    ))
}

/// Runs several metrics against the same records in a single pass over
/// `records` (see `metrics::compute_many`), each against its own threshold
/// (`thresholds[i]` for `metrics[i]` - a per-metric CLI override, or the same
/// value repeated if the caller wants one threshold shared across all of
/// them), and combines them into one overall verdict: `Fail` if any metric
/// fails, else `Inconclusive` if any metric is inconclusive, else `Pass`.
/// Matches the "a false pass is worse than an inconclusive result" rule: one
/// metric failing sinks the whole run.
#[allow(clippy::too_many_arguments)]
pub fn compare_many<I>(
    records: I,
    metrics: &[MetricConfig],
    confidence: f64,
    thresholds: &[verdict::Thresholds],
    resamples: usize,
    seed: u64,
    paired_by_id: bool,
    cluster_by_id: bool,
) -> Result<MultiReport, VeridictError>
where
    I: IntoIterator,
    I::Item: IntoRecordResult,
{
    debug_assert_eq!(metrics.len(), thresholds.len());
    let outs = metrics::compute_many(
        records,
        metrics,
        confidence,
        resamples,
        seed,
        paired_by_id,
        cluster_by_id,
    )?;
    let reports: Vec<Report> = metrics
        .iter()
        .zip(thresholds)
        .zip(outs)
        .map(|((&config, thresholds), out)| {
            build_report(
                config.kind(),
                confidence,
                thresholds,
                out,
                config.ci_method(),
                paired_by_id,
            )
        })
        .collect();
    let verdict = verdict::aggregate(reports.iter().map(|r| r.verdict));
    Ok(MultiReport {
        schema_version: report::REPORT_SCHEMA_VERSION,
        verdict,
        validity: Validity::Valid,
        promotion: Promotion::decide(Validity::Valid, verdict),
        simultaneous_claims_promotion: None,
        reports,
    })
}

fn build_report(
    metric: MetricKind,
    confidence: f64,
    thresholds: &verdict::Thresholds,
    out: metrics::MetricOutput,
    ci_method: CiMethod,
    paired_by_id: bool,
) -> Report {
    // Zero usable trials means "no signal", not "the CLI ran a threshold
    // check on a fabricated zero": force Inconclusive rather than letting
    // (0.0, 0.0) accidentally satisfy a threshold that includes zero.
    let (verdict, reason) = match &out.warning {
        Some(warning) => (Verdict::Inconclusive, warning.clone()),
        None => verdict::decide(out.ci_low, out.ci_high, thresholds),
    };
    // `None` for `Pass`/`Fail`, and for the `out.warning.is_some()` zero-trial path above (where
    // ci_low == ci_high == 0.0 would misleadingly read as `Noise` - it's a data-availability
    // problem, not a real CI judgment). `apply_failure_caps` clears this the same way it clears
    // `estimated_additional_trials` below, for the same reason.
    let inconclusive_kind = if verdict == Verdict::Inconclusive && out.warning.is_none() {
        Some(verdict::classify_inconclusive(out.ci_low, out.ci_high))
    } else {
        None
    };
    // `estimate_additional_trials` binary-searches wilson/jeffreys/exact -
    // none of which describe a cluster bootstrap CI's width at a hypothetical
    // n, and the independent unit under clustering is the cluster, not the
    // trial, so `paired_count` isn't even the right n to scale from.
    let estimated_additional_trials = if out.cluster_count.is_some() {
        None
    } else {
        verdict::estimate_additional_trials(
            metric,
            ci_method,
            verdict,
            out.effect,
            out.ci_low,
            out.ci_high,
            out.paired_count,
            thresholds,
            confidence,
        )
    };
    let (data_quality, warnings) = collect_data_quality(metric, &out, paired_by_id);
    let promotion = Promotion::decide(Validity::Valid, verdict);

    Report {
        schema_version: report::REPORT_SCHEMA_VERSION,
        verdict,
        validity: Validity::Valid,
        promotion,
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
        inconclusive_kind,
        warnings,
        data_quality,
        quantile: out.quantile,
        cluster_count: out.cluster_count,
        max_cluster_size: out.max_cluster_size,
        effective_sample_size: out.effective_sample_size,
        design_effect: out.design_effect,
        scale_diagnostics: out.scale_diagnostics,
        tied_count: out.tied_count,
        correction_method: None,
        family_size: None,
        achieved_alpha: None,
        adjusted_alpha_threshold: None,
        unadjusted_verdict: None,
        family_adjusted_verdict: None,
        family_adjusted_promotion: None,
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
    paired_by_id: bool,
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

    // `quantile-diff` only. Distribution-free proxy for "is there enough data in the thinner
    // tail to estimate this quantile at all" - same shape as the binomial `np >= 10` rule of
    // thumb, using the expected count in whichever tail is smaller. Deliberately NOT guarded by
    // `!tiny_sample`: unlike `effect_within_noise_floor` (a redundant restatement of "the sample
    // is small" once n alone already flags it), this carries information `tiny_sample`'s n-only
    // threshold can't see - e.g. n=100 at q=0.95 has only 5 expected tail observations and should
    // fire even though `paired_count < 30` is false.
    if let Some(q) = out.quantile
        && out.paired_count > 0
    {
        quality.thin_quantile_tail = out.paired_count as f64 * q.min(1.0 - q) < 10.0;
        if quality.thin_quantile_tail {
            warnings.push(format!(
                "thin quantile tail: only ~{:.1} expected observation(s) in the thinner tail at q={:.2} with {} paired trial(s) - this quantile estimate is likely unreliable",
                out.paired_count as f64 * q.min(1.0 - q),
                q,
                out.paired_count
            ));
        }
    }

    // `mean-diff` only - `relative-diff` ships `scale_diagnostics` too (for transparency/
    // machine-readability) but never sets `wide_baseline_scale` on itself: it's already the
    // proportional-change metric, so it has nothing to warn its own user to switch away from.
    // Computed from `out.scale_diagnostics` alone, which was itself built from baseline values
    // only (see `ScaleDiagnostics`'s doc) - this never inspects `out.effect`/`out.ci_low`/
    // `out.ci_high`, so the warning can't be, even accidentally, a function of the observed result.
    if metric == MetricKind::MeanDiff
        && let Some(diag) = &out.scale_diagnostics
    {
        // Robust (p95/p05) span is the primary signal once there's enough data for a single
        // outlier not to define it alone; below that floor, the raw min/max span is all there is.
        let effective_orders =
            if diag.positive_baseline_count >= metrics::ROBUST_SPAN_MIN_POSITIVE_BASELINES as u64 {
                diag.robust_orders_of_magnitude
                    .unwrap_or(diag.raw_orders_of_magnitude)
            } else {
                diag.raw_orders_of_magnitude
            };
        quality.wide_baseline_scale = effective_orders >= report::WIDE_BASELINE_SCALE_ORDERS;
        if quality.wide_baseline_scale {
            if diag.non_positive_baseline_count == 0 {
                warnings.push(format!(
                    "baseline values span {effective_orders:.1} orders of magnitude; absolute \
                     differences may be dominated by larger-scale cases. If the scientific \
                     question is proportional change, consider --metric relative-diff. Choose the \
                     metric before confirmatory analysis; switching after inspecting the verdict \
                     is exploratory."
                ));
            } else {
                warnings.push(
                    "baseline values vary widely, but some baselines are zero or negative, so \
                     relative-diff is not well-defined for this dataset. Use a domain-justified \
                     normalization rather than adding an arbitrary denominator offset."
                        .to_string(),
                );
            }
        }
    }

    // `relative-diff` only, and silent entirely under `--paired-by-id` (same convention as
    // `low_id_diversity` below - repeated ids mean something different there). `out.tied_count` is
    // counted at ingest (pre-netting - see `RelativeDiffAggregator::ingest`), while
    // `out.baseline_count` is post-netting; under `--paired-by-id` this isn't a small
    // approximation but a real, unbounded skew - two records from the same tied pair both count
    // toward `tied_count` but net to a single post-netting record, so the naive ratio can run past
    // 100% (verified: two exact-match pairs plus a distinct netted-to-zero pair produces
    // `tied_count == 2`, `baseline_count == 2`, i.e. a 2x-inflated 100% reading against the true
    // 50% pre-netting rate). Reporting a real pre-netting denominator would need new plumbing this
    // round doesn't add without a concrete request for it - see docs/research-map.md's
    // "subset-only relative-diff" entry, which already covers the closely related question of a
    // pre-netting-aware subset effect size.
    if !paired_by_id
        && let Some(tied_count) = out.tied_count
        && out.baseline_count > 0
    {
        let tied_fraction = tied_count as f64 / out.baseline_count as f64;
        quality.diluted_by_ties = tied_fraction >= report::TIE_DILUTION_FRACTION;
        if quality.diluted_by_ties {
            warnings.push(format!(
                "more than half of paired records show no change between candidate and baseline \
                 ({tied_count} of {}); if only a subset of cases was actually affected by this \
                 change, the pooled relative-diff effect is diluted toward zero by the unaffected \
                 majority - see docs/research-map.md's \"subset-only relative-diff\" entry for a \
                 deferred idea to report a target-subset effect size separately",
                out.baseline_count
            ));
        }
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
            false,
        )
        .unwrap();
        assert_eq!(report.verdict, Verdict::Inconclusive);
        assert_eq!(report.timeouts, 1);
        // Zero usable trials is a data-availability problem, not a real CI judgment - ci_low ==
        // ci_high == 0.0 here would misleadingly read as `Noise` if this weren't guarded.
        assert_eq!(report.inconclusive_kind, None);
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
        let thresholds = [
            Thresholds::symmetric(0.1).unwrap(),
            Thresholds::symmetric(0.1).unwrap(),
        ];
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
        let thresholds = [
            Thresholds::symmetric(0.1).unwrap(),
            Thresholds::symmetric(0.1).unwrap(),
        ];
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
            false,
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

    // --- wide_baseline_scale / scale_diagnostics ---

    fn scale_records(pairs: &[(f64, f64)]) -> Vec<(usize, Record)> {
        pairs
            .iter()
            .enumerate()
            .map(|(i, &(b, c))| {
                (
                    i + 1,
                    rec(&format!("s{i}"), Some(b), Some(c), None, None, None),
                )
            })
            .collect()
    }

    #[test]
    fn wide_baseline_scale_silent_below_ten_x_spread() {
        let records = scale_records(&[
            (10.0, 10.5),
            (20.0, 21.0),
            (30.0, 31.5),
            (40.0, 42.0),
            (50.0, 52.0),
        ]);
        let thresholds = Thresholds::symmetric(0.0).unwrap();
        let report = compare_one(
            records.iter().cloned(),
            MetricConfig::MeanDiff {
                bootstrap_method: BootstrapMethod::Percentile,
            },
            0.95,
            &thresholds,
            500,
            DEFAULT_SEED,
            false,
            false,
        )
        .unwrap();
        assert!(!report.data_quality.wide_baseline_scale);
        let diag = report
            .scale_diagnostics
            .expect("mean-diff always carries scale_diagnostics for nonempty data");
        assert!(diag.raw_orders_of_magnitude < report::WIDE_BASELINE_SCALE_ORDERS);
    }

    #[test]
    fn wide_baseline_scale_fires_at_or_above_ten_x_spread() {
        let records = scale_records(&[
            (10.0, 10.5),
            (40.0, 42.0),
            (70.0, 73.0),
            (100.0, 105.0),
            (150.0, 157.0),
        ]);
        let thresholds = Thresholds::symmetric(0.0).unwrap();
        let report = compare_one(
            records.iter().cloned(),
            MetricConfig::MeanDiff {
                bootstrap_method: BootstrapMethod::Percentile,
            },
            0.95,
            &thresholds,
            500,
            DEFAULT_SEED,
            false,
            false,
        )
        .unwrap();
        assert!(report.data_quality.wide_baseline_scale);
        assert!(
            report
                .warnings
                .iter()
                .any(|w| w.contains("orders of magnitude") && w.contains("--metric relative-diff"))
        );
    }

    #[test]
    fn wide_baseline_scale_uses_robust_span_ignoring_a_single_outlier_at_n40() {
        // Same construction verified directly against `compute_scale_diagnostics` in
        // `metrics::common`'s own tests - this confirms the same property survives the full
        // `compare_one` -> `collect_data_quality` pipeline, not just the underlying function.
        let mut pairs: Vec<(f64, f64)> = (0..39)
            .map(|i| {
                let b = 100.0 + i as f64;
                (b, b * 1.05)
            })
            .collect();
        pairs.push((10_000_000.0, 10_500_000.0));
        let records = scale_records(&pairs);
        let thresholds = Thresholds::symmetric(0.0).unwrap();
        let report = compare_one(
            records.iter().cloned(),
            MetricConfig::MeanDiff {
                bootstrap_method: BootstrapMethod::Percentile,
            },
            0.95,
            &thresholds,
            500,
            DEFAULT_SEED,
            false,
            false,
        )
        .unwrap();
        let diag = report.scale_diagnostics.unwrap();
        assert_eq!(diag.positive_baseline_count, 40);
        assert!(diag.raw_orders_of_magnitude > 4.0);
        assert!(diag.robust_orders_of_magnitude.unwrap() < 1.0);
        assert!(
            !report.data_quality.wide_baseline_scale,
            "a single outlier at n=40 should not trip the robust-span-gated warning"
        );
    }

    #[test]
    fn wide_baseline_scale_with_a_non_positive_baseline_recommends_normalization_not_relative_diff()
    {
        let records = scale_records(&[(-5.0, -4.0), (10.0, 10.5), (5000.0, 5250.0)]);
        let thresholds = Thresholds::symmetric(0.0).unwrap();
        let report = compare_one(
            records.iter().cloned(),
            MetricConfig::MeanDiff {
                bootstrap_method: BootstrapMethod::Percentile,
            },
            0.95,
            &thresholds,
            500,
            DEFAULT_SEED,
            false,
            false,
        )
        .unwrap();
        assert!(report.data_quality.wide_baseline_scale);
        let diag = report.scale_diagnostics.unwrap();
        assert_eq!(diag.non_positive_baseline_count, 1);
        assert!(
            report
                .warnings
                .iter()
                .any(|w| w.contains("zero or negative")
                    && w.contains("not well-defined")
                    && !w.contains("--metric relative-diff"))
        );
    }

    #[test]
    fn wide_baseline_scale_diagnostics_do_not_depend_on_candidate_effect_or_verdict() {
        let baselines = [10.0, 40.0, 70.0, 100.0, 150.0];
        let passing = scale_records(&baselines.iter().map(|&b| (b, b * 1.5)).collect::<Vec<_>>());
        let failing = scale_records(&baselines.iter().map(|&b| (b, b * 0.5)).collect::<Vec<_>>());
        let thresholds = Thresholds::symmetric(0.01).unwrap();
        let config = MetricConfig::MeanDiff {
            bootstrap_method: BootstrapMethod::Percentile,
        };
        let report_pass = compare_one(
            passing.iter().cloned(),
            config,
            0.95,
            &thresholds,
            500,
            DEFAULT_SEED,
            false,
            false,
        )
        .unwrap();
        let report_fail = compare_one(
            failing.iter().cloned(),
            config,
            0.95,
            &thresholds,
            500,
            DEFAULT_SEED,
            false,
            false,
        )
        .unwrap();
        // Sanity: these two runs really do land on different verdicts/effects - the point of this
        // test is that the *diagnostic* doesn't move even though the *result* clearly does.
        assert_ne!(report_pass.verdict, report_fail.verdict);
        assert!((report_pass.effect - report_fail.effect).abs() > 1.0);

        let diag_pass = report_pass.scale_diagnostics.unwrap();
        let diag_fail = report_fail.scale_diagnostics.unwrap();
        assert_eq!(
            diag_pass.raw_orders_of_magnitude,
            diag_fail.raw_orders_of_magnitude
        );
        assert_eq!(
            diag_pass.robust_orders_of_magnitude,
            diag_fail.robust_orders_of_magnitude
        );
        assert_eq!(
            report_pass.data_quality.wide_baseline_scale,
            report_fail.data_quality.wide_baseline_scale
        );
    }

    #[test]
    fn relative_diff_never_sets_wide_baseline_scale_even_with_wide_positive_scale() {
        let records = scale_records(&[
            (10.0, 10.5),
            (40.0, 42.0),
            (70.0, 73.0),
            (100.0, 105.0),
            (150.0, 157.0),
        ]);
        let thresholds = Thresholds::symmetric(0.0).unwrap();
        let report = compare_one(
            records.iter().cloned(),
            MetricConfig::RelativeDiff {
                bootstrap_method: BootstrapMethod::Percentile,
            },
            0.95,
            &thresholds,
            500,
            DEFAULT_SEED,
            false,
            false,
        )
        .unwrap();
        assert!(!report.data_quality.wide_baseline_scale);
        let diag = report
            .scale_diagnostics
            .expect("relative-diff also carries scale_diagnostics, for transparency");
        // Same wide baselines as `wide_baseline_scale_fires_at_or_above_ten_x_spread` above -
        // proves the bool's silence here is `relative-diff`-specific, not because the scale
        // happened to look narrow this time.
        assert!(diag.raw_orders_of_magnitude >= report::WIDE_BASELINE_SCALE_ORDERS);
    }

    #[test]
    fn inconclusive_kind_is_none_on_a_pass_verdict() {
        let records = [
            (1, rec("a", Some(1.0), Some(2.0), None, None, None)),
            (2, rec("b", Some(1.0), Some(2.0), None, None, None)),
        ];
        let thresholds = Thresholds::symmetric(0.5).unwrap();
        let report = compare_one(
            records.iter().cloned(),
            MetricConfig::MeanDiff {
                bootstrap_method: BootstrapMethod::Percentile,
            },
            0.95,
            &thresholds,
            500,
            DEFAULT_SEED,
            false,
            false,
        )
        .unwrap();
        assert_eq!(report.verdict, Verdict::Pass);
        assert_eq!(report.inconclusive_kind, None);
    }

    #[test]
    fn inconclusive_kind_is_directional_when_ci_excludes_zero_but_stays_in_the_dead_zone() {
        // Every diff is identical (+0.5): resampling an identical value can't move the mean, so
        // the bootstrap CI collapses to a single point at 0.5 - deterministically excludes zero,
        // deterministically inside a +-5.0 dead zone.
        let records: Vec<_> = (0..30)
            .map(|i| {
                (
                    i + 1,
                    rec(&format!("r{i}"), Some(1.0), Some(1.5), None, None, None),
                )
            })
            .collect();
        let thresholds = Thresholds::symmetric(5.0).unwrap();
        let report = compare_one(
            records.iter().cloned(),
            MetricConfig::MeanDiff {
                bootstrap_method: BootstrapMethod::Percentile,
            },
            0.95,
            &thresholds,
            500,
            DEFAULT_SEED,
            false,
            false,
        )
        .unwrap();
        assert_eq!(report.verdict, Verdict::Inconclusive);
        assert_eq!(
            report.inconclusive_kind,
            Some(InconclusiveKind::Directional)
        );
        // Directly ties into the dead-zone estimated_additional_trials contract documented above.
        assert_eq!(report.estimated_additional_trials, None);
    }

    #[test]
    fn inconclusive_kind_is_noise_when_ci_straddles_zero() {
        // Diffs alternate +0.1/-0.1: effect is exactly 0.0, and the bootstrap CI around a
        // two-point-mass sample straddles zero rather than collapsing to it.
        let records: Vec<_> = (0..40)
            .map(|i| {
                let diff = if i % 2 == 0 { 0.1 } else { -0.1 };
                (
                    i + 1,
                    rec(
                        &format!("r{i}"),
                        Some(1.0),
                        Some(1.0 + diff),
                        None,
                        None,
                        None,
                    ),
                )
            })
            .collect();
        let thresholds = Thresholds::symmetric(5.0).unwrap();
        let report = compare_one(
            records.iter().cloned(),
            MetricConfig::MeanDiff {
                bootstrap_method: BootstrapMethod::Percentile,
            },
            0.95,
            &thresholds,
            500,
            DEFAULT_SEED,
            false,
            false,
        )
        .unwrap();
        assert_eq!(report.verdict, Verdict::Inconclusive);
        assert_eq!(report.inconclusive_kind, Some(InconclusiveKind::Noise));
    }

    #[test]
    fn relative_diff_tied_count_and_dilution_flag_fire_when_most_records_are_exact_matches() {
        // 8 of 10 records are exact matches (candidate == baseline); only 2 carry a real change.
        let mut records = vec![
            (
                1,
                rec("touched-a", Some(10.0), Some(11.0), None, None, None),
            ),
            (
                2,
                rec("touched-b", Some(10.0), Some(11.0), None, None, None),
            ),
        ];
        for i in 0..8 {
            records.push((
                3 + i,
                rec(
                    &format!("untouched{i}"),
                    Some(10.0),
                    Some(10.0),
                    None,
                    None,
                    None,
                ),
            ));
        }
        let thresholds = Thresholds::symmetric(0.0).unwrap();
        let report = compare_one(
            records.iter().cloned(),
            MetricConfig::RelativeDiff {
                bootstrap_method: BootstrapMethod::Percentile,
            },
            0.95,
            &thresholds,
            500,
            DEFAULT_SEED,
            false,
            false,
        )
        .unwrap();
        assert_eq!(report.tied_count, Some(8));
        assert!(report.data_quality.diluted_by_ties);
        assert!(report.warnings.iter().any(|w| w.contains("no change")));
    }

    #[test]
    fn relative_diff_tied_count_is_zero_and_dilution_flag_silent_with_no_ties() {
        let records: Vec<_> = (0..10)
            .map(|i| {
                (
                    i + 1,
                    rec(&format!("r{i}"), Some(10.0), Some(11.0), None, None, None),
                )
            })
            .collect();
        let thresholds = Thresholds::symmetric(0.0).unwrap();
        let report = compare_one(
            records.iter().cloned(),
            MetricConfig::RelativeDiff {
                bootstrap_method: BootstrapMethod::Percentile,
            },
            0.95,
            &thresholds,
            500,
            DEFAULT_SEED,
            false,
            false,
        )
        .unwrap();
        assert_eq!(report.tied_count, Some(0));
        assert!(!report.data_quality.diluted_by_ties);
    }

    #[test]
    fn relative_diff_tied_count_under_paired_by_id_counts_at_ingest_not_post_netting() {
        // "pair" nets two *opposite* nonzero ratios (+10%/-10%) to a diff of exactly 0.0 - neither
        // record was an exact match, so it must not count toward tied_count. "tied" nets two
        // genuine exact matches - both must count.
        let records = [
            (1, rec("pair", Some(100.0), Some(110.0), None, None, None)),
            (2, rec("pair", Some(100.0), Some(90.0), None, None, None)),
            (3, rec("tied", Some(100.0), Some(100.0), None, None, None)),
            (4, rec("tied", Some(100.0), Some(100.0), None, None, None)),
        ];
        let thresholds = Thresholds::symmetric(0.0).unwrap();
        let report = compare_one(
            records.iter().cloned(),
            MetricConfig::RelativeDiff {
                bootstrap_method: BootstrapMethod::Percentile,
            },
            0.95,
            &thresholds,
            500,
            DEFAULT_SEED,
            true,
            false,
        )
        .unwrap();
        assert_eq!(
            report.tied_count,
            Some(2),
            "only the two literal exact-match records count, not the netted-to-zero pair"
        );
        assert_eq!(
            report.baseline_count, 2,
            "post-netting: 2 groups -> 2 diffs"
        );
        // tied_count(2)/baseline_count(2) would naively read as a 100% tied rate here, double the
        // true 50% pre-netting rate - diluted_by_ties must stay silent under --paired-by-id rather
        // than report that inflated number.
        assert!(!report.data_quality.diluted_by_ties);
    }

    #[test]
    fn tied_count_is_none_for_metrics_other_than_relative_diff() {
        let records = [
            (1, rec("a", Some(1.0), Some(1.5), None, None, None)),
            (2, rec("b", Some(1.0), Some(1.5), None, None, None)),
        ];
        let thresholds = Thresholds::symmetric(0.0).unwrap();
        let report = compare_one(
            records.iter().cloned(),
            MetricConfig::MeanDiff {
                bootstrap_method: BootstrapMethod::Percentile,
            },
            0.95,
            &thresholds,
            500,
            DEFAULT_SEED,
            false,
            false,
        )
        .unwrap();
        assert_eq!(report.tied_count, None);
        assert!(!report.data_quality.diluted_by_ties);
    }
}
