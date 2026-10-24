use super::*;

pub(super) fn run_time_sensitive(args: TimeSensitiveArgs) -> Result<ExitCode, VeridictError> {
    let reward = resolve_time_sensitive_reward(&args)?;
    let config = time_sensitive::TimeSensitiveConfig {
        hypotheses: time_sensitive::BernoulliHypotheses {
            p0: args.p0,
            p1: args.p1,
        },
        alpha: args.alpha,
        reward,
        policy: args.policy.into(),
        action_grid_size: args.action_grid_size,
        wealth_grid_size: args.wealth_grid_size,
    };
    let format = resolve_format(&args.input, args.format);
    let records = read_records(&args.input, format)?;
    let failure_policy: FailurePolicy = args.failure_policy.into();

    let mut report = time_sensitive::run(records, config, failure_policy)?;
    let caps = FailureCaps {
        max_timeouts: args.max_timeouts,
        max_crashes: args.max_crashes,
        max_invalid: args.max_invalid,
    };
    time_sensitive::apply_failure_caps(&mut report, &caps);
    let json = report.to_json_pretty();
    let markdown = report.to_markdown();

    println!("{json}");
    write_reports(&json, &markdown, &args.report_json, &args.report_md)?;
    Ok(exit_code_for(report.verdict))
}

/// Picks the reward schedule from exactly one of `--reward`(`--deadline`/`--time-scale`/
/// `--horizon`)/`--reward-schedule`, and rejects a flag that doesn't belong to the chosen
/// `--reward` variant (e.g. `--time-scale` with `--reward hard-deadline`) - clap's declarative
/// `requires`/`conflicts_with` can express "at least one of --reward/--reward-schedule" and "not
/// both `--reward` and `--reward-schedule`" but not "exactly this flag combination for exactly
/// this enum value," the same gap `resolve_sprt_hypotheses` fills for `--sprt-variant`.
fn resolve_time_sensitive_reward(
    args: &TimeSensitiveArgs,
) -> Result<time_sensitive::RewardSchedule, VeridictError> {
    if let Some(path) = &args.reward_schedule {
        let text = std::fs::read_to_string(path).map_err(|source| VeridictError::Io {
            path: path.display().to_string(),
            source,
        })?;
        let file: time_sensitive::reward::RewardScheduleFile = serde_json::from_str(&text)
            .map_err(|source| VeridictError::InvalidJsonFile {
                path: path.display().to_string(),
                source,
            })?;
        return time_sensitive::reward::expand_schedule_file(&file);
    }
    match args.reward {
        Some(RewardKindArg::HardDeadline) => {
            if args.time_scale.is_some() || args.horizon.is_some() {
                return Err(VeridictError::InvalidThreshold(
                    "--time-scale/--horizon are only used with --reward exponential; pass \
                     --deadline for --reward hard-deadline"
                        .to_string(),
                ));
            }
            let deadline = args.deadline.ok_or_else(|| {
                VeridictError::InvalidThreshold(
                    "--reward hard-deadline requires --deadline".to_string(),
                )
            })?;
            Ok(time_sensitive::RewardSchedule::HardDeadline { deadline })
        }
        Some(RewardKindArg::Exponential) => {
            if args.deadline.is_some() {
                return Err(VeridictError::InvalidThreshold(
                    "--deadline is only used with --reward hard-deadline; pass --time-scale/\
                     --horizon for --reward exponential"
                        .to_string(),
                ));
            }
            let time_scale = args.time_scale.ok_or_else(|| {
                VeridictError::InvalidThreshold(
                    "--reward exponential requires --time-scale".to_string(),
                )
            })?;
            let horizon = args.horizon.ok_or_else(|| {
                VeridictError::InvalidThreshold(
                    "--reward exponential requires --horizon".to_string(),
                )
            })?;
            Ok(time_sensitive::RewardSchedule::ExponentialDecay {
                time_scale,
                horizon,
            })
        }
        None => Err(VeridictError::InvalidThreshold(
            "exactly one of --reward or --reward-schedule is required".to_string(),
        )),
    }
}
