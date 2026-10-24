//! Validated metric configuration.

use super::*;

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
    /// Opt-in pair-score-aware fixed-sample Elo. Every input record must have an id and every id
    /// must contribute exactly two usable outcomes; the CLI therefore requires
    /// `--paired-by-id`. Kept separate so `MetricConfig::Elo` remains source- and behavior-
    /// compatible.
    PairedElo {
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
            Self::PairedElo { .. } => MetricKind::PairedElo,
            Self::QuantileDiff { .. } => MetricKind::QuantileDiff,
            Self::RelativeDiff { .. } => MetricKind::RelativeDiff,
        }
    }

    /// Builds a validated config from flat, CLI-flag-shaped inputs - the
    /// same compatibility checks `compute_many` used to run on every call
    /// (`ci_method` other than `Wilson` is only valid for `WinRate`/
    /// `SignTest`; `failure_policy` other than `ReportOnly` is only valid
    /// for `WinRate`/`Elo`/`PairedElo`; `Bca` isn't a valid `bootstrap_method` for
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
            | MetricKind::PairedElo
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
            MetricKind::PairedElo => Ok(Self::PairedElo { failure_policy }),
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
    pub(super) fn ci_method(&self) -> CiMethod {
        match self {
            Self::WinRate { ci_method, .. } | Self::SignTest { ci_method } => *ci_method,
            Self::MeanDiff { .. }
            | Self::Elo { .. }
            | Self::PairedElo { .. }
            | Self::QuantileDiff { .. }
            | Self::RelativeDiff { .. } => CiMethod::Wilson,
        }
    }
}
