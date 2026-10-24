//! Thin CLI wrapper around the `veridict` library. Owns all stdout/stderr
//! output and exit codes; the library itself never prints.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand, ValueEnum, error::ErrorKind};

use veridict::correction::Correction;
use veridict::sprt::{SprtConfig, SprtRunOptions, SprtVariant};
use veridict::stats::bootstrap::{DEFAULT_SEED, sample_sd};
use veridict::verdict::{self, Thresholds};
use veridict::{
    BootstrapMethod, CiMethod, FailureCaps, FailurePolicy, MetricConfig, MetricKind, Verdict,
    VeridictError, matrix, power, time_sensitive,
};

mod args;
mod compare;
mod io;
mod matrix_plan_command;
mod power_command;
mod sprt_command;
mod time_sensitive_command;
mod verify_run_command;

use self::args::*;
use self::compare::run_compare;
use self::io::{
    read_match_records, read_records, read_verify_run_records, resolve_format, write_reports,
};
use self::matrix_plan_command::{run_matrix, run_plan};
use self::power_command::run_power;
use self::sprt_command::run_sprt;
use self::time_sensitive_command::run_time_sensitive;
use self::verify_run_command::run_verify_run;

pub(crate) fn entrypoint() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => {
            let exit_code = match error.kind() {
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion => 0,
                _ => 3,
            };
            if let Err(print_error) = error.print() {
                eprintln!("error: could not print command-line diagnostic: {print_error}");
            }
            return ExitCode::from(exit_code);
        }
    };
    match run(cli.command) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(3)
        }
    }
}

/// Dispatches a parsed command without adding command-specific behavior at the binary root.
fn run(command: Command) -> Result<ExitCode, VeridictError> {
    match command {
        Command::Compare(args) => run_compare(args),
        Command::Sprt(args) => run_sprt(args),
        Command::Matrix(args) => run_matrix(args),
        Command::Plan(args) => run_plan(args),
        Command::Power(args) => run_power(args),
        Command::VerifyRun(args) => run_verify_run(args),
        Command::TimeSensitive(args) => run_time_sensitive(args),
    }
}

fn exit_code_for(verdict: Verdict) -> ExitCode {
    match verdict {
        Verdict::Pass => ExitCode::from(0),
        Verdict::Fail => ExitCode::from(1),
        Verdict::Inconclusive => ExitCode::from(2),
    }
}
