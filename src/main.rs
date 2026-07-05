//! Thin CLI wrapper around the `veridict` library. Owns all stdout/stderr
//! output and exit codes; the library itself never prints.

use std::io::{self, BufRead};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand, ValueEnum};

use veridict::sprt::{SprtConfig, SprtVariant};
use veridict::stats::bootstrap::DEFAULT_SEED;
use veridict::verdict::Thresholds;
use veridict::{BootstrapMethod, CiMethod, MetricKind, Verdict, VeridictError, input, matrix};

#[derive(Parser)]
#[command(
    name = "veridict",
    version,
    about = "Statistical decision gate: is the candidate actually better than the baseline?"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Compare candidate vs baseline results and emit a pass/fail/inconclusive verdict.
    Compare(CompareArgs),
    /// Sequential probability ratio test: accumulate evidence until the candidate is clearly
    /// at least elo1 stronger (pass), clearly at most elo0 stronger (fail), or keep testing.
    Sprt(SprtArgs),
    /// Compare more than two candidates, each measured against the same shared baseline, and
    /// tabulate pairwise Elo differences. Report-only: always exits 0 on success (no single
    /// pass/fail verdict applies to a whole matrix).
    Matrix(MatrixArgs),
}

#[derive(clap::Args)]
struct CompareArgs {
    /// Input file. Use "-" to read from stdin.
    input: PathBuf,

    /// Repeat to run several metrics against the same input and combine
    /// them into one verdict (fail dominates, then inconclusive, then pass).
    #[arg(long = "metric", value_enum, required = true)]
    metrics: Vec<MetricArg>,

    /// Input format. Defaults to sniffing the file extension (.csv vs
    /// everything else); pass explicitly when reading CSV from stdin.
    #[arg(long, value_enum)]
    format: Option<FormatArg>,

    #[arg(long, default_value_t = 0.95)]
    confidence: f64,

    /// Symmetric effect-size threshold: pass if the CI lower bound is >= this, fail if the CI upper bound is <= -this. Ignored if --pass-above/--fail-below are given.
    #[arg(long)]
    min_effect: Option<f64>,

    /// Explicit pass threshold. Requires --fail-below.
    #[arg(long, requires = "fail_below", allow_hyphen_values = true)]
    pass_above: Option<f64>,

    /// Explicit fail threshold. Requires --pass-above.
    #[arg(long, requires = "pass_above", allow_hyphen_values = true)]
    fail_below: Option<f64>,

    /// Bootstrap resample count, used only by --metric mean-diff.
    #[arg(long, default_value_t = 10_000)]
    resamples: usize,

    /// Bootstrap RNG seed, used only by --metric mean-diff. Defaults to a
    /// fixed seed, so the same input reproduces bit-identical output in CI.
    #[arg(long)]
    seed: Option<u64>,

    /// Confidence interval method for --metric winrate/sign-test. `exact`
    /// (Clopper-Pearson) and `jeffreys` are only valid for those two metrics;
    /// combining either with --metric elo/mean-diff is a config error, not a
    /// silent fallback.
    #[arg(long, value_enum, default_value = "wilson")]
    ci_method: CiMethodArg,

    /// Bootstrap variant for --metric mean-diff. `bca` corrects for bias and
    /// skewness; `basic` reflects the percentile interval around the point
    /// estimate (simpler, no bias-correction of its own); `percentile` stays
    /// the default so existing CI numbers don't silently shift.
    #[arg(long, value_enum, default_value = "percentile")]
    bootstrap_method: BootstrapMethodArg,

    /// Treat two records sharing an id as one testcase played twice (e.g.
    /// roles swapped to cancel the testcase's own bias) and combine them
    /// into a single net observation instead of two independent ones. An id
    /// used only once is an ordinary unpaired sample; 3+ uses of the same
    /// id is rejected as a data error.
    #[arg(long)]
    paired_by_id: bool,

    /// Also write the JSON report to this file.
    #[arg(long)]
    report_json: Option<PathBuf>,

    /// Also write a human-readable Markdown report to this file.
    #[arg(long)]
    report_md: Option<PathBuf>,
}

#[derive(clap::Args)]
struct SprtArgs {
    /// Input file. Use "-" to read from stdin.
    input: PathBuf,

    /// Input format. Defaults to sniffing the file extension (.csv vs
    /// everything else); pass explicitly when reading CSV from stdin.
    #[arg(long, value_enum)]
    format: Option<FormatArg>,

    /// SPRT variant. `wald` (default): classic two-outcome test, draws
    /// excluded, via --elo0/--elo1 (logistic Elo). `trinomial`: draw rate
    /// estimated as a nuisance parameter, converges faster on draw-heavy
    /// data, via --belo0/--belo1 (BayesElo - a different scale from
    /// logistic Elo whenever the estimated draw rate is nonzero, see
    /// stats::trinomial_sprt's doc).
    #[arg(long, value_enum, default_value = "wald")]
    sprt_variant: SprtVariantArg,

    /// H0 for --sprt-variant wald: the candidate is at most this many
    /// logistic-Elo points stronger.
    #[arg(long, allow_hyphen_values = true)]
    elo0: Option<f64>,

    /// H1 for --sprt-variant wald: the candidate is at least this many
    /// logistic-Elo points stronger.
    #[arg(long, allow_hyphen_values = true)]
    elo1: Option<f64>,

    /// H0 for --sprt-variant trinomial: the candidate is at most this many
    /// BayesElo points stronger.
    #[arg(long, allow_hyphen_values = true)]
    belo0: Option<f64>,

    /// H1 for --sprt-variant trinomial: the candidate is at least this many
    /// BayesElo points stronger.
    #[arg(long, allow_hyphen_values = true)]
    belo1: Option<f64>,

    /// False-positive rate: probability of accepting H1 when H0 is true.
    #[arg(long, default_value_t = 0.05)]
    alpha: f64,

    /// False-negative rate: probability of accepting H0 when H1 is true.
    #[arg(long, default_value_t = 0.05)]
    beta: f64,

    /// Treat two records sharing an id as one testcase played twice and
    /// combine them into a single net observation. See `compare
    /// --paired-by-id` for the exact semantics.
    #[arg(long)]
    paired_by_id: bool,

    /// Also write the JSON report to this file.
    #[arg(long)]
    report_json: Option<PathBuf>,

    /// Also write a human-readable Markdown report to this file.
    #[arg(long)]
    report_md: Option<PathBuf>,
}

#[derive(clap::Args)]
struct MatrixArgs {
    /// Legacy: one file per candidate, each measured against the same shared baseline (id/
    /// baseline/candidate/result schema). Candidate names come from each file's stem (e.g.
    /// "prompt_a.jsonl" -> "prompt_a"). At least one of `files`/`--matches` is required.
    #[arg(num_args = 1.., required_unless_present = "matches")]
    files: Vec<PathBuf>,

    /// Head-to-head match data between named competitors (id/a/b/result schema, result is
    /// a_win|b_win|draw). Competitor names come from each record's a/b fields, not the file
    /// name. Repeatable. Use the literal name "baseline" in a/b to connect this data to the
    /// baseline node implied by `files`.
    #[arg(long = "matches", num_args = 1.., required_unless_present = "files")]
    matches: Vec<PathBuf>,

    /// Input format, applied to every file (both legacy files and --matches). Defaults to
    /// sniffing each file's extension.
    #[arg(long, value_enum)]
    format: Option<FormatArg>,

    #[arg(long, default_value_t = 0.95)]
    confidence: f64,

    /// Treat two records sharing an id as one testcase played twice and
    /// combine them into a single net observation. See `compare
    /// --paired-by-id` for the exact semantics; applied independently to
    /// each file.
    #[arg(long)]
    paired_by_id: bool,

    /// Bootstrap resample count for general-graph confidence intervals. Has
    /// no effect when every edge touches "baseline" (star mode uses the
    /// closed-form Wilson interval instead).
    #[arg(long, default_value_t = 2_000)]
    resamples: usize,

    /// Bootstrap RNG seed for general-graph confidence intervals. Defaults
    /// to a fixed seed, so the same input reproduces bit-identical output.
    #[arg(long)]
    seed: Option<u64>,

    /// Bootstrap variant for general-graph confidence intervals. Same
    /// meaning as `compare`'s `--bootstrap-method`; has no effect in star
    /// mode (closed-form Wilson interval, no bootstrap involved).
    #[arg(long, value_enum, default_value = "percentile")]
    bootstrap_method: BootstrapMethodArg,

    /// Also write the JSON report to this file.
    #[arg(long)]
    report_json: Option<PathBuf>,

    /// Also write a human-readable Markdown report to this file.
    #[arg(long)]
    report_md: Option<PathBuf>,
}

#[derive(Clone, Copy, ValueEnum)]
enum MetricArg {
    Winrate,
    MeanDiff,
    SignTest,
    Elo,
}

impl From<MetricArg> for MetricKind {
    fn from(m: MetricArg) -> Self {
        match m {
            MetricArg::Winrate => MetricKind::WinRate,
            MetricArg::MeanDiff => MetricKind::MeanDiff,
            MetricArg::SignTest => MetricKind::SignTest,
            MetricArg::Elo => MetricKind::Elo,
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum FormatArg {
    Jsonl,
    Csv,
}

#[derive(Clone, Copy, ValueEnum)]
enum CiMethodArg {
    Wilson,
    Exact,
    Jeffreys,
}

impl From<CiMethodArg> for CiMethod {
    fn from(m: CiMethodArg) -> Self {
        match m {
            CiMethodArg::Wilson => CiMethod::Wilson,
            CiMethodArg::Exact => CiMethod::Exact,
            CiMethodArg::Jeffreys => CiMethod::Jeffreys,
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum BootstrapMethodArg {
    Percentile,
    Bca,
    Basic,
}

impl From<BootstrapMethodArg> for BootstrapMethod {
    fn from(m: BootstrapMethodArg) -> Self {
        match m {
            BootstrapMethodArg::Percentile => BootstrapMethod::Percentile,
            BootstrapMethodArg::Bca => BootstrapMethod::Bca,
            BootstrapMethodArg::Basic => BootstrapMethod::Basic,
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum SprtVariantArg {
    Wald,
    Trinomial,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli.command) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(3)
        }
    }
}

fn run(command: Command) -> Result<ExitCode, VeridictError> {
    match command {
        Command::Compare(args) => run_compare(args),
        Command::Sprt(args) => run_sprt(args),
        Command::Matrix(args) => run_matrix(args),
    }
}

fn run_compare(args: CompareArgs) -> Result<ExitCode, VeridictError> {
    let thresholds = match (args.pass_above, args.fail_below) {
        (Some(pass_above), Some(fail_below)) => Thresholds::new(pass_above, fail_below)?,
        _ => Thresholds::symmetric(args.min_effect.unwrap_or(0.0))?,
    };

    let format = resolve_format(&args.input, args.format);
    let records = read_records(&args.input, format)?;
    let seed = args.seed.unwrap_or(DEFAULT_SEED);
    let metrics: Vec<MetricKind> = args.metrics.into_iter().map(Into::into).collect();
    let ci_method: CiMethod = args.ci_method.into();
    let bootstrap_method: BootstrapMethod = args.bootstrap_method.into();

    let (verdict, json, markdown) = if let [only] = metrics[..] {
        let report = veridict::compare_one(
            records,
            only,
            args.confidence,
            &thresholds,
            args.resamples,
            seed,
            args.paired_by_id,
            ci_method,
            bootstrap_method,
        )?;
        (
            report.verdict,
            report.to_json_pretty(),
            report.to_markdown(),
        )
    } else {
        let multi = veridict::compare_many(
            records,
            &metrics,
            args.confidence,
            &thresholds,
            args.resamples,
            seed,
            args.paired_by_id,
            ci_method,
            bootstrap_method,
        )?;
        (multi.verdict, multi.to_json_pretty(), multi.to_markdown())
    };

    println!("{json}");
    write_reports(&json, &markdown, &args.report_json, &args.report_md)?;
    Ok(exit_code_for(verdict))
}

fn run_sprt(args: SprtArgs) -> Result<ExitCode, VeridictError> {
    let (elo0, elo1, variant) = resolve_sprt_hypotheses(&args)?;
    let config = SprtConfig::new(elo0, elo1, args.alpha, args.beta)?;
    let format = resolve_format(&args.input, args.format);
    let records = read_records(&args.input, format)?;

    let report = veridict::sprt::run(records, &config, variant, args.paired_by_id)?;
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
    }
}

fn run_matrix(args: MatrixArgs) -> Result<ExitCode, VeridictError> {
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

fn write_reports(
    json: &str,
    markdown: &str,
    report_json: &Option<PathBuf>,
    report_md: &Option<PathBuf>,
) -> Result<(), VeridictError> {
    if let Some(path) = report_json {
        std::fs::write(path, json).map_err(|source| VeridictError::Io {
            path: path.display().to_string(),
            source,
        })?;
    }
    if let Some(path) = report_md {
        std::fs::write(path, markdown).map_err(|source| VeridictError::Io {
            path: path.display().to_string(),
            source,
        })?;
    }
    Ok(())
}

fn exit_code_for(verdict: Verdict) -> ExitCode {
    match verdict {
        Verdict::Pass => ExitCode::from(0),
        Verdict::Fail => ExitCode::from(1),
        Verdict::Inconclusive => ExitCode::from(2),
    }
}

fn resolve_format(path: &Path, explicit: Option<FormatArg>) -> FormatArg {
    explicit.unwrap_or_else(|| match path.extension().and_then(|e| e.to_str()) {
        Some(ext) if ext.eq_ignore_ascii_case("csv") => FormatArg::Csv,
        _ => FormatArg::Jsonl,
    })
}

type RecordIter = Box<dyn Iterator<Item = Result<(usize, input::Record), VeridictError>>>;

/// Streams records lazily from the file/stdin - callers that only need
/// `winrate`/`elo`/`sign-test`/`sprt` (never `mean-diff`) get bounded memory
/// regardless of input size, since nothing here materializes a `Vec` up
/// front. `input::parse_jsonl`/`parse_csv` are already lazy iterators; this
/// just boxes whichever one applies so both format branches share one
/// return type.
fn read_records(path: &PathBuf, format: FormatArg) -> Result<RecordIter, VeridictError> {
    let reader = open_input(path)?;
    Ok(match format {
        FormatArg::Jsonl => Box::new(input::parse_jsonl(reader)),
        FormatArg::Csv => Box::new(input::parse_csv(reader)),
    })
}

type MatchRecordIter = Box<dyn Iterator<Item = Result<(usize, input::MatchRecord), VeridictError>>>;

/// Same lazy-streaming shape as `read_records`, for `matrix --matches`.
fn read_match_records(path: &PathBuf, format: FormatArg) -> Result<MatchRecordIter, VeridictError> {
    let reader = open_input(path)?;
    Ok(match format {
        FormatArg::Jsonl => Box::new(input::parse_jsonl(reader)),
        FormatArg::Csv => Box::new(input::parse_csv(reader)),
    })
}

fn open_input(path: &PathBuf) -> Result<Box<dyn BufRead>, VeridictError> {
    if path.as_os_str() == "-" {
        // Streamed directly, not slurped into a Vec first: input::parse_jsonl/
        // parse_csv are lazy, so buffering all of stdin up front here would be
        // the one place that silently defeats streaming for piped input.
        Ok(Box::new(io::BufReader::new(io::stdin())))
    } else {
        let file = std::fs::File::open(path).map_err(|source| VeridictError::Io {
            path: path.display().to_string(),
            source,
        })?;
        Ok(Box::new(io::BufReader::new(file)))
    }
}
