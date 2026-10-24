use super::*;

/// Resolves the mean-difference variability source after clap has parsed mutually exclusive
/// options. Returns `(sd, source, extra_notes)`; the notes retain pilot-specific caveats that the
/// pure sample-size calculation cannot infer because it never reads the pilot file.
fn resolve_assume_sd(args: &PowerArgs) -> Result<(f64, &'static str, Vec<String>), VeridictError> {
    match (args.assume_sd, &args.pilot) {
        (Some(sd), None) => {
            if !sd.is_finite() || sd <= 0.0 {
                return Err(VeridictError::InvalidAssumeSd(sd));
            }
            Ok((sd, "assume-sd", Vec::new()))
        }
        (None, Some(pilot_path)) => {
            let format = resolve_format(pilot_path, args.pilot_format);
            let records = read_records(pilot_path, format)?;
            let diffs = power::pilot_diffs(records, args.paired_by_id)?;
            if diffs.len() < 2 {
                return Err(VeridictError::InsufficientPilotData {
                    path: pilot_path.display().to_string(),
                    count: diffs.len(),
                });
            }
            let sd = sample_sd(&diffs);
            if sd == 0.0 {
                return Err(VeridictError::ZeroVariancePilotData {
                    path: pilot_path.display().to_string(),
                    count: diffs.len(),
                });
            }
            let mut notes = Vec::new();
            if diffs.len() < 30 {
                notes.push(format!(
                    "--pilot '{}' has only {} usable paired diff(s) - below the conventional \
                     30-observation threshold for the sample standard deviation itself to be a \
                     reliable estimate; treat assume_sd as a rougher guess than usual. Also, \
                     normal quantiles (used here) slightly underestimate the required n relative \
                     to a small-sample t-distribution correction, which isn't applied.",
                    pilot_path.display(),
                    diffs.len(),
                ));
            }
            Ok((sd, "pilot", notes))
        }
        (None, None) => Err(VeridictError::MeanDiffPowerRequiresSd),
        (Some(_), Some(_)) => {
            unreachable!("clap's conflicts_with_all already rejects --assume-sd with --pilot")
        }
    }
}

pub(super) fn run_power(args: PowerArgs) -> Result<ExitCode, VeridictError> {
    let (json, markdown) = if args.sprt {
        // clap's `requires_all = ["elo0", "elo1"]` on --sprt guarantees both are `Some` here.
        let report = power::estimate_sprt_expected_trials(
            args.elo0.expect("clap requires elo0 with --sprt"),
            args.elo1.expect("clap requires elo1 with --sprt"),
            args.alpha,
            args.beta,
            args.horizon,
        )?;
        (report.to_json_pretty(), report.to_markdown())
    } else {
        // clap's `required_unless_present = "sprt"` on these three guarantees `Some` here.
        let metric_arg = args.metric.expect("clap requires --metric without --sprt");
        if metric_arg != PowerMetricArg::MeanDiff
            && (args.assume_sd.is_some() || args.pilot.is_some())
        {
            return Err(VeridictError::AssumeSdOnlyForMeanDiff {
                metric: power_metric_arg_label(metric_arg),
            });
        }

        let (metric, extra_notes) = if metric_arg == PowerMetricArg::MeanDiff {
            let (assume_sd, sd_source, extra_notes) = resolve_assume_sd(&args)?;
            (
                power::PowerMetric::MeanDiff {
                    assume_sd,
                    sd_source,
                },
                extra_notes,
            )
        } else {
            let ci_method: CiMethod = args.ci_method.into();
            (
                power::PowerMetric::new(metric_arg.into(), ci_method)?,
                Vec::new(),
            )
        };

        let mut report = power::estimate_trials(
            metric,
            args.min_effect
                .expect("clap requires --min-effect without --sprt"),
            args.assume_effect
                .expect("clap requires --assume-effect without --sprt"),
            args.confidence,
            args.target_power,
            args.paired_by_id,
        )?;
        report.notes.extend(extra_notes);
        (report.to_json_pretty(), report.to_markdown())
    };

    println!("{json}");
    write_reports(&json, &markdown, &args.report_json, &args.report_md)?;
    Ok(ExitCode::from(0))
}
