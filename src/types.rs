//! Stable domain types shared by metrics, reports, and decision policies.

/// A fixed-sample result needs at least this many independent analysis units before it may return
/// `pass` or `fail`. One observation produces a point bootstrap distribution and can otherwise
/// look certain despite containing no empirical variance information.
pub const MIN_DECISIVE_UNITS: u64 = 2;

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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
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
    /// Fixed-sample logistic Elo over strict two-trial pairs, with a pair-level bootstrap CI and
    /// all five combined-score buckets retained. This is distinct from sequential pentanomial
    /// SPRT and does not change the existing `Elo` metric's net-pair behavior.
    #[serde(rename = "paired-elo")]
    PairedElo,
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailurePolicy {
    ReportOnly,
    Exclude,
    Loss,
}
