//! Binary entry point. CLI definitions and handlers live in `cli` so this file only owns startup.

use std::process::ExitCode;

mod cli;

fn main() -> ExitCode {
    cli::entrypoint()
}
