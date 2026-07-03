//! Thin CLI wrapper around the `veridict` library. Owns all stdout/stderr
//! output and exit codes; the library itself never prints.

use std::io::{self, BufRead, Read};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand, ValueEnum};

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

#[derive(Clone, Copy, ValueEnum)]
enum MetricArg {
    Winrate,
    MeanDiff,
    SignTest,
}

impl From<MetricArg> for MetricKind {
    fn from(m: MetricArg) -> Self {
        match m {
            MetricArg::Winrate => MetricKind::WinRate,
            MetricArg::MeanDiff => MetricKind::MeanDiff,
            MetricArg::SignTest => MetricKind::SignTest,
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
    let Command::Compare(args) = command;

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
    if let Some(path) = &args.report_json {
        std::fs::write(path, &json).map_err(|source| VeridictError::Io {
            path: path.display().to_string(),
            source,
        })?;
    }
    if let Some(path) = &args.report_md {
        std::fs::write(path, &markdown).map_err(|source| VeridictError::Io {
            path: path.display().to_string(),
            source,
        })?;
    }

    Ok(match verdict {
        Verdict::Pass => ExitCode::from(0),
        Verdict::Fail => ExitCode::from(1),
        Verdict::Inconclusive => ExitCode::from(2),
    })
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
