use super::*;

pub(super) fn run_sprt(args: SprtArgs) -> Result<ExitCode, VeridictError> {
    let (elo0, elo1, variant) = resolve_sprt_hypotheses(&args)?;
    let config = SprtConfig::new(elo0, elo1, args.alpha, args.beta)?;
    let format = resolve_format(&args.input, args.format);
    let records = read_records(&args.input, format)?;
    let failure_policy: FailurePolicy = args.failure_policy.into();

    let mut report = veridict::sprt::run_with_options(
        records,
        &config,
        SprtRunOptions {
            variant,
            paired_by_id: args.paired_by_id,
            failure_policy,
            require_complete_pairs: args.require_complete_pairs,
            min_paired_ids: args.min_paired_ids,
            max_paired_ids: args.max_paired_ids,
        },
    )?;
    let caps = FailureCaps {
        max_timeouts: args.max_timeouts,
        max_crashes: args.max_crashes,
        max_invalid: args.max_invalid,
    };
    veridict::sprt::apply_failure_caps(&mut report, &caps);
    let json = report.to_json_pretty();
    let markdown = report.to_markdown();

    println!("{json}");
    write_reports(&json, &markdown, &args.report_json, &args.report_md)?;
    Ok(exit_code_for(report.verdict))
}

/// Picks the (elo0, elo1) pair matching `--sprt-variant`, and rejects the
/// *other* variant's flags being set too - per AGENTS.md's "never silently
/// ignore invalid data", a wald run given `--belo0` (or vice versa) is a
/// user mistake worth a clear error, not a silently-dropped flag.
fn resolve_sprt_hypotheses(args: &SprtArgs) -> Result<(f64, f64, SprtVariant), VeridictError> {
    if args.require_complete_pairs && !args.paired_by_id {
        return Err(VeridictError::InvalidThreshold(
            "--require-complete-pairs requires --paired-by-id".to_string(),
        ));
    }
    if (args.min_paired_ids.is_some() || args.max_paired_ids.is_some())
        && !matches!(args.sprt_variant, SprtVariantArg::Pentanomial)
    {
        return Err(VeridictError::InvalidThreshold(
            "--min-paired-ids/--max-paired-ids require --sprt-variant pentanomial (the only \
             variant with a paired_count)"
                .to_string(),
        ));
    }
    if let Some(min) = args.min_paired_ids
        && min < 1
    {
        return Err(VeridictError::InvalidThreshold(
            "--min-paired-ids must be >= 1".to_string(),
        ));
    }
    if let Some(max) = args.max_paired_ids
        && max < 1
    {
        return Err(VeridictError::InvalidThreshold(
            "--max-paired-ids must be >= 1".to_string(),
        ));
    }
    if let (Some(min), Some(max)) = (args.min_paired_ids, args.max_paired_ids)
        && min > max
    {
        return Err(VeridictError::InvalidThreshold(format!(
            "--min-paired-ids ({min}) must be <= --max-paired-ids ({max})"
        )));
    }

    let wald_flags_given = args.elo0.is_some() || args.elo1.is_some();
    let trinomial_flags_given = args.belo0.is_some() || args.belo1.is_some();
    match args.sprt_variant {
        SprtVariantArg::Wald => {
            if trinomial_flags_given {
                return Err(VeridictError::InvalidThreshold(
                    "--belo0/--belo1 are only used with --sprt-variant trinomial; pass --elo0/--elo1 for the default wald variant".to_string(),
                ));
            }
            match (args.elo0, args.elo1) {
                (Some(e0), Some(e1)) => Ok((e0, e1, SprtVariant::Wald)),
                _ => Err(VeridictError::InvalidThreshold(
                    "--elo0 and --elo1 are required for --sprt-variant wald (the default)"
                        .to_string(),
                )),
            }
        }
        SprtVariantArg::Trinomial => {
            if wald_flags_given {
                return Err(VeridictError::InvalidThreshold(
                    "--elo0/--elo1 are only used with --sprt-variant wald; pass --belo0/--belo1 for --sprt-variant trinomial".to_string(),
                ));
            }
            match (args.belo0, args.belo1) {
                (Some(b0), Some(b1)) => Ok((b0, b1, SprtVariant::Trinomial)),
                _ => Err(VeridictError::InvalidThreshold(
                    "--belo0 and --belo1 are required for --sprt-variant trinomial".to_string(),
                )),
            }
        }
        // Pentanomial shares wald's logistic-Elo scale (no drawelo-style nuisance parameter
        // exists in this model, see stats::pentanomial_sprt's doc), so it takes the same
        // --elo0/--elo1 branch shape as wald, not trinomial's --belo0/--belo1.
        SprtVariantArg::Pentanomial => {
            if trinomial_flags_given {
                return Err(VeridictError::InvalidThreshold(
                    "--belo0/--belo1 are only used with --sprt-variant trinomial; pass --elo0/--elo1 for --sprt-variant pentanomial".to_string(),
                ));
            }
            if !args.paired_by_id {
                return Err(VeridictError::InvalidThreshold(
                    "--sprt-variant pentanomial requires --paired-by-id".to_string(),
                ));
            }
            match (args.elo0, args.elo1) {
                (Some(e0), Some(e1)) => Ok((e0, e1, SprtVariant::Pentanomial)),
                _ => Err(VeridictError::InvalidThreshold(
                    "--elo0 and --elo1 are required for --sprt-variant pentanomial".to_string(),
                )),
            }
        }
    }
}
