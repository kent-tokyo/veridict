//! Thin CLI wrapper around the `veridict` library. Owns all stdout/stderr
//! output and exit codes; the library itself never prints.

use std::io::{self, BufRead, Read};
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand, ValueEnum};

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
    /// JSONL input file. Use "-" to read from stdin.
    input: PathBuf,

    #[arg(long, value_enum)]
    metric: MetricArg,

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

    /// Also write the JSON report to this file.
    #[arg(long)]
    report_json: Option<PathBuf>,
}

#[derive(Clone, ValueEnum)]
enum MetricArg {
    Winrate,
    MeanDiff,
}

impl From<MetricArg> for MetricKind {
    fn from(m: MetricArg) -> Self {
        match m {
            MetricArg::Winrate => MetricKind::WinRate,
            MetricArg::MeanDiff => MetricKind::MeanDiff,
        }
    }
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

    let reader = open_input(&args.input)?;
    let records: Vec<(usize, input::Record)> =
        input::parse_jsonl(reader).collect::<Result<_, _>>()?;

    let report = veridict::compare(
        &records,
        args.metric.into(),
        args.confidence,
        &thresholds,
        args.resamples,
    )?;

    let json = report.to_json_pretty();
    println!("{json}");
    if let Some(path) = &args.report_json {
        std::fs::write(path, &json).map_err(|source| VeridictError::Io {
            path: path.display().to_string(),
            source,
        })?;
    }

    Ok(match report.verdict {
        Verdict::Pass => ExitCode::from(0),
        Verdict::Fail => ExitCode::from(1),
        Verdict::Inconclusive => ExitCode::from(2),
    })
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
