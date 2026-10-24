use super::*;

fn resolve_thresholds(
    metrics: &[MetricArg],
    pass_above: Option<f64>,
    fail_below: Option<f64>,
    min_effect: &[MinEffectEntry],
) -> Result<Vec<Thresholds>, VeridictError> {
    let mut default: Option<f64> = None;
    let mut overrides: Vec<(MetricArg, f64)> = Vec::new();
    for entry in min_effect {
        match *entry {
            MinEffectEntry::Default(v) => {
                if let Some(existing) = default {
                    return Err(VeridictError::InvalidThreshold(format!(
                        "--min-effect gives more than one default value ({existing} and {v}); \
                         only one bare number is allowed per run"
                    )));
                }
                default = Some(v);
            }
            MinEffectEntry::Metric(m, v) => {
                let label = metric_arg_label(m);
                if overrides
                    .iter()
                    .any(|&(om, _)| MetricKind::from(om) == MetricKind::from(m))
                {
                    return Err(VeridictError::InvalidThreshold(format!(
                        "--min-effect specifies metric '{label}' more than once"
                    )));
                }
                if !metrics
                    .iter()
                    .any(|&rm| MetricKind::from(rm) == MetricKind::from(m))
                {
                    return Err(VeridictError::InvalidThreshold(format!(
                        "--min-effect '{label}=...' names a metric that wasn't requested via \
                         --metric"
                    )));
                }
                overrides.push((m, v));
            }
        }
    }

    if let (Some(pass_above), Some(fail_below)) = (pass_above, fail_below) {
        return metrics
            .iter()
            .map(|_| Thresholds::new(pass_above, fail_below))
            .collect();
    }

    metrics
        .iter()
        .map(|&m| {
            let v = overrides
                .iter()
                .find(|&&(om, _)| MetricKind::from(om) == MetricKind::from(m))
                .map(|&(_, v)| v)
                .unwrap_or_else(|| default.unwrap_or(0.0));
            Thresholds::symmetric(v).map_err(|e| {
                VeridictError::InvalidThreshold(format!(
                    "--min-effect for metric '{}': {e}",
                    metric_arg_label(m)
                ))
            })
        })
        .collect()
}

/// Which scale a metric's raw effect number lives on - see `CompareArgs::min_effect`'s doc for
/// the same three-way split. Used only to warn when a bare `--min-effect` default is silently
/// shared across metrics whose numbers aren't comparable.
fn min_effect_unit_family(m: MetricArg) -> &'static str {
    match m {
        MetricArg::Winrate | MetricArg::SignTest => "win-rate margin off 0.5",
        MetricArg::RelativeDiff => "relative ratio",
        MetricArg::MeanDiff | MetricArg::QuantileDiff | MetricArg::Elo | MetricArg::PairedElo => {
            "raw units"
        }
    }
}

/// Warns on stderr (never blocks the run - this is advisory, like the `--correction` deprecation
/// warning it's modeled on) when a bare `--min-effect` default silently applies to two or more
/// requested metrics whose unit families differ (see `min_effect_unit_family`) - e.g. `--min-effect
/// 0.01` meaning "barely above a coin flip" for `sign-test` and "a real 1% score change" for
/// `relative-diff` at the same time. Silent when every metric has its own `metric=value` override
/// (nothing is actually shared), when only one metric is requested (nothing to compare against),
/// or when `--pass-above`/`--fail-below` were given (they bypass `--min-effect` entirely).
fn warn_on_mixed_min_effect_units(
    metrics: &[MetricArg],
    pass_above: Option<f64>,
    fail_below: Option<f64>,
    min_effect: &[MinEffectEntry],
) {
    if pass_above.is_some() || fail_below.is_some() {
        return;
    }
    let default = min_effect.iter().find_map(|e| match *e {
        MinEffectEntry::Default(v) => Some(v),
        MinEffectEntry::Metric(..) => None,
    });
    let Some(default) = default else {
        return;
    };
    let overridden: Vec<MetricArg> = min_effect
        .iter()
        .filter_map(|e| match *e {
            MinEffectEntry::Metric(m, _) => Some(m),
            MinEffectEntry::Default(_) => None,
        })
        .collect();
    let defaulted: Vec<MetricArg> = metrics
        .iter()
        .copied()
        .filter(|m| {
            !overridden
                .iter()
                .any(|&om| MetricKind::from(om) == MetricKind::from(*m))
        })
        .collect();

    let mut families: Vec<&'static str> = defaulted
        .iter()
        .map(|&m| min_effect_unit_family(m))
        .collect();
    families.sort_unstable();
    families.dedup();
    if families.len() < 2 {
        return;
    }

    let named: Vec<String> = defaulted
        .iter()
        .map(|&m| format!("{} ({})", metric_arg_label(m), min_effect_unit_family(m)))
        .collect();
    eprintln!(
        "warning: --min-effect {default} applies the same threshold to {} without a per-metric \
         override, but these are not the same scale. Consider e.g. --min-effect {}.",
        named.join(" and "),
        defaulted
            .iter()
            .map(|&m| format!("{}={default}", metric_arg_label(m)))
            .collect::<Vec<_>>()
            .join(",")
    );
}

pub(super) fn run_compare(args: CompareArgs) -> Result<ExitCode, VeridictError> {
    let thresholds = resolve_thresholds(
        &args.metrics,
        args.pass_above,
        args.fail_below,
        &args.min_effect,
    )?;
    warn_on_mixed_min_effect_units(
        &args.metrics,
        args.pass_above,
        args.fail_below,
        &args.min_effect,
    );

    let format = resolve_format(&args.input, args.format);
    let records = read_records(&args.input, format)?;
    let seed = args.seed.unwrap_or(DEFAULT_SEED);
    let ci_method: CiMethod = args.ci_method.into();
    let bootstrap_method: BootstrapMethod = args.bootstrap_method.into();
    let failure_policy: FailurePolicy = args.failure_policy.into();
    // `MetricConfig::new` itself treats `quantile` as silently unused by non-`QuantileDiff`
    // metrics (so it can be shared across e.g. `--metric mean-diff --metric quantile-diff` in one
    // run) - but a `--quantile` passed without *any* `--metric quantile-diff` requested at all is
    // a config error at this, the CLI boundary, not a silent no-op: the project's own "never
    // silently drop/fall back" rule (see `IncompatibleCiMethod`/`IncompatibleFailurePolicy`).
    if args.quantile.is_some()
        && !args
            .metrics
            .iter()
            .any(|m| matches!(m, MetricArg::QuantileDiff))
    {
        return Err(VeridictError::QuantileRequiresQuantileDiffMetric);
    }
    let metrics: Vec<MetricConfig> = args
        .metrics
        .into_iter()
        .map(|m| {
            MetricConfig::new(
                m.into(),
                ci_method,
                bootstrap_method,
                failure_policy,
                args.quantile,
            )
        })
        .collect::<Result<_, _>>()?;

    let correction: Correction = match (args.claim_correction, args.correction) {
        (Some(c), None) => c.into(),
        (None, Some(c)) => {
            eprintln!(
                "warning: --correction is deprecated; use --claim-correction instead \
                 (--correction will be removed in a future release)"
            );
            c.into()
        }
        (None, None) => Correction::None,
        (Some(_), Some(_)) => unreachable!("clap's conflicts_with rejects passing both"),
    };
    let caps = FailureCaps {
        max_timeouts: args.max_timeouts,
        max_crashes: args.max_crashes,
        max_invalid: args.max_invalid,
    };
    // Failure caps run first so an invalid report's verdict is already forced to Inconclusive
    // before claim correction inspects it - an invalid report must never count as a legitimate
    // statistical claim (see correction::apply_correction's doc).
    let (verdict, json, markdown) = if let [only] = metrics[..] {
        let mut report = veridict::compare_one(
            records,
            only,
            args.confidence,
            &thresholds[0],
            args.resamples,
            seed,
            args.paired_by_id,
            args.cluster_by_id,
        )?;
        verdict::apply_failure_caps(&mut report, &caps);
        veridict::correction::apply_correction(
            std::slice::from_mut(&mut report),
            &metrics,
            correction,
            args.confidence,
        )?;
        (
            report.verdict,
            report.to_json_pretty(),
            report.to_markdown(),
        )
    } else {
        let mut multi = veridict::compare_many(
            records,
            &metrics,
            args.confidence,
            &thresholds,
            args.resamples,
            seed,
            args.paired_by_id,
            args.cluster_by_id,
        )?;
        verdict::apply_failure_caps_to_multi(&mut multi, &caps);
        veridict::correction::apply_correction_to_multi(
            &mut multi,
            &metrics,
            correction,
            args.confidence,
        )?;
        (multi.verdict, multi.to_json_pretty(), multi.to_markdown())
    };

    println!("{json}");
    write_reports(&json, &markdown, &args.report_json, &args.report_md)?;
    Ok(exit_code_for(verdict))
}
