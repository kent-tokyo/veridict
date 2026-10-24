//! Fail-closed validation for derived numeric observations and metric outputs.

use super::*;

/// Computes the numeric observation shared by `mean-diff`, `quantile-diff`, and `sign-test`.
/// Finite operands do not guarantee a finite subtraction (`f64::MAX - (-f64::MAX)` overflows),
/// so the derived value is checked before it reaches pairing, bootstrap, or verdict logic.
pub(crate) fn checked_difference(
    line: usize,
    baseline: f64,
    candidate: f64,
) -> Result<f64, VeridictError> {
    let diff = candidate - baseline;
    if !diff.is_finite() {
        return Err(VeridictError::InvalidValue {
            line,
            field: "candidate_minus_baseline",
            value: diff,
        });
    }
    Ok(diff)
}

fn require_finite(
    metric: MetricKind,
    field: &'static str,
    value: f64,
) -> Result<(), VeridictError> {
    if value.is_finite() {
        Ok(())
    } else {
        Err(VeridictError::NonFiniteStatistic {
            metric: metric_label(metric),
            field,
            value,
        })
    }
}

/// Last fail-closed boundary before a metric result can influence a verdict or be serialized.
/// This is central rather than bootstrap-specific so a future metric cannot accidentally turn
/// `NaN`/infinity into JSON `null` and then compare that non-finite value against thresholds.
pub(crate) fn validate_finite_output(
    metric: MetricKind,
    out: &MetricOutput,
) -> Result<(), VeridictError> {
    require_finite(metric, "effect", out.effect)?;
    require_finite(metric, "ci_low", out.ci_low)?;
    require_finite(metric, "ci_high", out.ci_high)?;
    if let Some(value) = out.effective_sample_size {
        require_finite(metric, "effective_sample_size", value)?;
    }
    if let Some(value) = out.design_effect {
        require_finite(metric, "design_effect", value)?;
    }
    if let Some(diag) = out.scale_diagnostics {
        require_finite(metric, "min_positive_baseline", diag.min_positive_baseline)?;
        require_finite(metric, "max_positive_baseline", diag.max_positive_baseline)?;
        require_finite(
            metric,
            "raw_orders_of_magnitude",
            diag.raw_orders_of_magnitude,
        )?;
        if let Some(value) = diag.robust_orders_of_magnitude {
            require_finite(metric, "robust_orders_of_magnitude", value)?;
        }
    }
    Ok(())
}
