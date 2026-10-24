use super::*;

/// `verify_run::run` needs the whole record set because pairing, ordering, and deduplication are
/// cross-record checks. Other commands keep their input iterators lazy.
pub(super) fn run_verify_run(args: VerifyRunArgs) -> Result<ExitCode, VeridictError> {
    let manifest_text =
        std::fs::read_to_string(&args.manifest).map_err(|source| VeridictError::Io {
            path: args.manifest.display().to_string(),
            source,
        })?;
    let manifest =
        veridict::verify_run::parse_manifest(&args.manifest.display().to_string(), &manifest_text)?;

    let format = resolve_format(&args.games, args.format);
    let records: Vec<_> =
        read_verify_run_records(&args.games, format)?.collect::<Result<_, _>>()?;

    let report = veridict::verify_run::run(&manifest, records)?;
    let json = report.to_json_pretty();
    let markdown = report.to_markdown();

    println!("{json}");
    write_reports(&json, &markdown, &args.report_json, &args.report_md)?;
    Ok(match report.validity {
        veridict::Validity::Valid => ExitCode::from(0),
        veridict::Validity::Invalid => ExitCode::from(1),
    })
}
