//! Thin CLI wrapper around the `veridict` library. Owns all stdout/stderr
//! output and exit codes; the library itself never prints.

use std::io::{self, BufRead, Read};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand, ValueEnum};

use veridict::sprt::SprtConfig;
use veridict::stats::bootstrap::DEFAULT_SEED;
use veridict::verdict::Thresholds;
use veridict::{MetricKind, Verdict, VeridictError, input};

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
    #[arg(long, requires = "fail_below")]
    pass_above: Option<f64>,

    /// Explicit fail threshold. Requires --pass-above.
    #[arg(long, requires = "pass_above")]
    fail_below: Option<f64>,

    /// Bootstrap resample count, used only by --metric mean-diff.
    #[arg(long, default_value_t = 10_000)]
    resamples: usize,

    /// Bootstrap RNG seed, used only by --metric mean-diff. Defaults to a
    /// fixed seed, so the same input reproduces bit-identical output in CI.
    #[arg(long)]
    seed: Option<u64>,

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

    /// H0: the candidate is at most this many Elo points stronger.
    #[arg(long, allow_hyphen_values = true)]
    elo0: f64,

    /// H1: the candidate is at least this many Elo points stronger.
    #[arg(long, allow_hyphen_values = true)]
    elo1: f64,

    /// False-positive rate: probability of accepting H1 when H0 is true.
    #[arg(long, default_value_t = 0.05)]
    alpha: f64,

    /// False-negative rate: probability of accepting H0 when H1 is true.
    #[arg(long, default_value_t = 0.05)]
    beta: f64,

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

    let (verdict, json, markdown) = if let [only] = metrics[..] {
        let report = veridict::compare_one(
            &records,
            only,
            args.confidence,
            &thresholds,
            args.resamples,
            seed,
        )?;
        (
            report.verdict,
            report.to_json_pretty(),
            report.to_markdown(),
        )
    } else {
        let multi = veridict::compare_many(
            &records,
            &metrics,
            args.confidence,
            &thresholds,
            args.resamples,
            seed,
        )?;
        (multi.verdict, multi.to_json_pretty(), multi.to_markdown())
    };

    println!("{json}");
    write_reports(&json, &markdown, &args.report_json, &args.report_md)?;
    Ok(exit_code_for(verdict))
}

fn run_sprt(args: SprtArgs) -> Result<ExitCode, VeridictError> {
    let config = SprtConfig::new(args.elo0, args.elo1, args.alpha, args.beta)?;
    let format = resolve_format(&args.input, args.format);
    let records = read_records(&args.input, format)?;

    let report = veridict::sprt::run(&records, &config)?;
    let json = report.to_json_pretty();
    let markdown = report.to_markdown();

    println!("{json}");
    write_reports(&json, &markdown, &args.report_json, &args.report_md)?;
    Ok(exit_code_for(report.verdict))
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

fn read_records(
    path: &PathBuf,
    format: FormatArg,
) -> Result<Vec<(usize, input::Record)>, VeridictError> {
    let reader = open_input(path)?;
    match format {
        FormatArg::Jsonl => input::parse_jsonl(reader).collect(),
        FormatArg::Csv => input::parse_csv(reader).collect(),
    }
}

fn open_input(path: &PathBuf) -> Result<Box<dyn BufRead>, VeridictError> {
    if path.as_os_str() == "-" {
        let mut buf = Vec::new();
        io::stdin()
            .read_to_end(&mut buf)
            .map_err(|source| VeridictError::Io {
                path: "<stdin>".to_string(),
                source,
            })?;
        Ok(Box::new(io::Cursor::new(buf)))
    } else {
        let file = std::fs::File::open(path).map_err(|source| VeridictError::Io {
            path: path.display().to_string(),
            source,
        })?;
        Ok(Box::new(io::BufReader::new(file)))
    }
}
