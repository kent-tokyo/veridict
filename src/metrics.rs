//! Turns raw records into a metric's effect size and confidence interval.
//!
//! A record is "used" if it contributes to either the status tally
//! (timeout/crash/invalid) or the chosen metric's calculation. A record that
//! matches neither is rejected as `SchemaMismatch` rather than silently
//! dropped, per AGENTS.md's "never silently ignore invalid data" rule.
//!
//! Each metric is an independent `MetricAggregator` (see the per-metric
//! submodules), so running several `--metric` flags together scans the
//! input once (`compute_many`), feeding every record to every requested
//! metric's aggregator, rather than one full pass per metric.

mod common;
mod elo;
mod engine;
mod mean_diff;
mod numeric;
mod outcome;
mod paired_elo;
mod quantile_diff;
mod relative_diff;
mod sign_test;
mod winrate;

pub(crate) use common::{DiffCollector, ROBUST_SPAN_MIN_POSITIVE_BASELINES};
pub(crate) use engine::MetricAggregator;
pub use engine::{compute, compute_many};
pub(crate) use numeric::checked_difference;
pub(crate) use numeric::validate_finite_output;
pub(crate) use outcome::{effective_outcome, tally_status};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::error::VeridictError;
use crate::input::Record;
use crate::report::{PairScoreCounts, ScaleDiagnostics};
use crate::{
    BootstrapMethod, CiMethod, FailurePolicy, IntoRecordResult, MetricConfig, MetricKind, Outcome,
    TrialStatus,
};

/// Per-side failure tally, so a report can distinguish "the baseline kept
/// crashing" from "the candidate kept timing out" instead of one opaque
/// combined number.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FailureCounts {
    pub timeout: u64,
    pub crash: u64,
    pub invalid: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FailureBreakdown {
    pub baseline: FailureCounts,
    pub candidate: FailureCounts,
}

/// Everything the report needs from a metric computation, before thresholds
/// are applied.
pub struct MetricOutput {
    pub effect: f64,
    pub ci_low: f64,
    pub ci_high: f64,
    pub baseline_count: u64,
    pub candidate_count: u64,
    pub paired_count: u64,
    pub timeouts: u64,
    pub crashes: u64,
    pub invalid: u64,
    pub failures: FailureBreakdown,
    /// Set when there were zero trials usable by the metric; the caller must
    /// treat this as Inconclusive rather than running it through thresholds.
    pub warning: Option<String>,
    /// Id-repetition stats, tracked centrally in `compute_many` (not by any
    /// one aggregator - it applies identically no matter which metric is
    /// requested). Every aggregator sets these to `0` in its own `finish()`;
    /// `compute_many` overwrites them with the real values on every output
    /// before returning. `0` for both when `paired_by_id` is set - paired
    /// mode already has its own meaning for repeated ids (`SchemaMismatch`
    /// on >2-per-id), so this tracking is skipped there entirely.
    pub records_with_id: u64,
    /// The largest number of records sharing any single id.
    pub max_id_count: u64,
    /// The quantile `quantile-diff` measured (e.g. `0.95` for p95), so the report can say which
    /// one. `None` for every other metric.
    pub quantile: Option<f64>,
    /// `--cluster-by-id` only (winrate/elo): number of distinct resampling clusters (one per
    /// distinct id, plus one per id-less record) the cluster bootstrap CI was computed from.
    /// `None` when `--cluster-by-id` wasn't requested.
    pub cluster_count: Option<u64>,
    /// `--cluster-by-id` only: the largest single cluster's record count - the "how repeated is
    /// the most-repeated id" signal (e.g. a dominant opening in a chess-engine test).
    pub max_cluster_size: Option<u64>,
    /// `--cluster-by-id` only: `paired_count / design_effect` (Kish 1965) - the naive sample
    /// size deflated for intra-cluster correlation, i.e. "how many truly independent trials this
    /// clustered data is actually worth."
    pub effective_sample_size: Option<f64>,
    /// `--cluster-by-id` only: `Var(cluster bootstrap) / Var(i.i.d. bootstrap)` on the same
    /// pooled data - 1.0 means no measurable clustering effect; higher means the CI would have
    /// been overconfident without accounting for the clusters.
    pub design_effect: Option<f64>,
    /// `mean-diff`/`relative-diff` only: the raw distribution of baseline values ingested by this
    /// aggregator, computed from baselines alone - never candidate, effect, CI, or verdict (see
    /// `common::compute_scale_diagnostics`). `None` for every other metric, and for these two
    /// metrics whenever there were zero usable trials (nothing to compute a distribution from).
    pub scale_diagnostics: Option<ScaleDiagnostics>,
    /// `relative-diff` only: count of ingested records where `candidate == baseline` exactly,
    /// counted before any `--paired-by-id` netting (see `RelativeDiffAggregator::ingest`). `None`
    /// for every other metric.
    pub tied_count: Option<u64>,
    /// `paired-elo` only: the five complete-pair score buckets. `None` for every other metric.
    pub pair_score_counts: Option<PairScoreCounts>,
}

/// A record-level `SchemaMismatch` needs a short label for what it failed
/// to match; matches the CLI's `--metric` spelling.
pub(crate) fn metric_label(metric: MetricKind) -> &'static str {
    match metric {
        MetricKind::WinRate => "metric winrate",
        MetricKind::MeanDiff => "metric mean-diff",
        MetricKind::SignTest => "metric sign-test",
        MetricKind::Elo => "metric elo",
        MetricKind::PairedElo => "metric paired-elo",
        MetricKind::QuantileDiff => "metric quantile-diff",
        MetricKind::RelativeDiff => "metric relative-diff",
    }
}

/// Shared by `verdict::estimate_additional_trials` and `correction`'s two compatibility checks
/// (`achieved_alpha`'s early return and `apply_correction`'s upfront guard) - all three need the
/// exact same answer to "does this metric's CI have a closed form to search/recompute against," so
/// this is the one place that set is spelled out, rather than three independently-maintained
/// `matches!` lists that could silently drift apart when a new bootstrap-only metric is added.
pub(crate) fn lacks_closed_form_ci(metric: MetricKind) -> bool {
    matches!(
        metric,
        MetricKind::MeanDiff
            | MetricKind::PairedElo
            | MetricKind::QuantileDiff
            | MetricKind::RelativeDiff
    )
}

/// A label for `IncompatibleCiMethod`'s error message; matches the CLI's
/// `--ci-method` spelling. Only ever reached for a non-`Wilson` method (the
/// upfront guard above never fires for `Wilson`), but kept total rather than
/// `unreachable!()` for that arm, per AGENTS.md's "boring, explicit" rule.
pub(crate) fn ci_method_label(ci_method: CiMethod) -> &'static str {
    match ci_method {
        CiMethod::Wilson => "wilson",
        CiMethod::Exact => "exact",
        CiMethod::Jeffreys => "jeffreys",
    }
}

/// A label for `IncompatibleFailurePolicy`'s error message; matches the CLI's `--failure-policy`
/// spelling. Only ever reached for a non-`ReportOnly` policy (the upfront guard in
/// `MetricConfig::new` never fires for `ReportOnly`), but kept total rather than `unreachable!()`
/// for that arm, same precedent as `ci_method_label`.
pub(crate) fn failure_policy_label(policy: FailurePolicy) -> &'static str {
    match policy {
        FailurePolicy::ReportOnly => "report-only",
        FailurePolicy::Exclude => "exclude",
        FailurePolicy::Loss => "loss",
    }
}

/// A label for `IncompatibleBootstrapMethod`'s error message; matches the CLI's
/// `--bootstrap-method` spelling. Only ever reached for `Bca` (the upfront guard in
/// `MetricConfig::new` never fires for `Percentile`/`Basic`), but kept total rather than
/// `unreachable!()` for those arms, same precedent as `ci_method_label`.
pub(crate) fn bootstrap_method_label(method: BootstrapMethod) -> &'static str {
    match method {
        BootstrapMethod::Percentile => "percentile",
        BootstrapMethod::Bca => "bca",
        BootstrapMethod::Basic => "basic",
    }
}

#[cfg(test)]
mod tests;
