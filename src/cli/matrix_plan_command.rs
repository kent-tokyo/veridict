use super::*;

pub(super) fn run_matrix(args: MatrixArgs) -> Result<ExitCode, VeridictError> {
    let format = args.format;
    let mut seen_names = std::collections::HashSet::new();
    // Lazy: each `(name, records-iterator)` pair is only produced (and each
    // file only opened) as `matrix::run` pulls it, one candidate at a time -
    // see `matrix::run`'s own doc comment for why this bounds peak memory by
    // the largest single file rather than the sum of all of them. The
    // duplicate-name check still runs per-file, in the same interleaved
    // order as before, not hoisted into a separate upfront pass.
    let named_records = args.files.iter().map(move |path| {
        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("candidate")
            .to_string();
        if !seen_names.insert(name.clone()) {
            return Err(VeridictError::InvalidThreshold(format!(
                "duplicate candidate name '{name}' from input file stems; rename one of the files"
            )));
        }
        let fmt = resolve_format(path, format);
        let records = read_records(path, fmt)?;
        Ok((name, records))
    });
    let match_records = args
        .matches
        .iter()
        .map(move |path| read_match_records(path, resolve_format(path, format)));

    let matrix = matrix::run(
        named_records,
        match_records,
        args.confidence,
        args.paired_by_id,
        args.resamples,
        args.seed.unwrap_or(DEFAULT_SEED),
        args.bootstrap_method.into(),
    )?;
    let json = matrix.to_json_pretty();
    let markdown = matrix.to_markdown();

    println!("{json}");
    write_reports(&json, &markdown, &args.report_json, &args.report_md)?;
    Ok(ExitCode::from(0))
}

pub(super) fn run_plan(args: PlanArgs) -> Result<ExitCode, VeridictError> {
    let format = args.format;
    let mut seen_names = std::collections::HashSet::new();
    let named_records = args.files.iter().map(move |path| {
        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("candidate")
            .to_string();
        if !seen_names.insert(name.clone()) {
            return Err(VeridictError::InvalidThreshold(format!(
                "duplicate candidate name '{name}' from input file stems; rename one of the files"
            )));
        }
        let fmt = resolve_format(path, format);
        let records = read_records(path, fmt)?;
        Ok((name, records))
    });
    let match_records = args
        .matches
        .iter()
        .map(move |path| read_match_records(path, resolve_format(path, format)));

    let plan = veridict::plan::run(
        named_records,
        match_records,
        args.confidence,
        args.paired_by_id,
        args.resamples,
        args.seed.unwrap_or(DEFAULT_SEED),
        args.bootstrap_method.into(),
        args.min_elo,
    )?;
    let json = plan.to_json_pretty();
    let markdown = plan.to_markdown();

    println!("{json}");
    write_reports(&json, &markdown, &args.report_json, &args.report_md)?;
    Ok(ExitCode::from(0))
}
