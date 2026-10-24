//! End-to-end CLI tests: real binary, real exit codes, and real stdout.

use assert_cmd::Command;
use predicates::prelude::*;

fn veridict() -> Command {
    Command::cargo_bin("veridict").unwrap()
}

fn write_winloss_file(path: &std::path::Path, candidate_wins: usize, baseline_wins: usize) {
    let mut text = String::new();
    for _ in 0..candidate_wins {
        text.push_str("{\"result\":\"candidate_win\"}\n");
    }
    for _ in 0..baseline_wins {
        text.push_str("{\"result\":\"baseline_win\"}\n");
    }
    std::fs::write(path, text).unwrap();
}

#[path = "cli/bootstrap_quality.rs"]
mod bootstrap_quality;
#[path = "cli/compare.rs"]
mod compare;
#[path = "cli/correction_failure.rs"]
mod correction_failure;
#[path = "cli/matrix_plan.rs"]
mod matrix_plan;
#[path = "cli/pairing_failure.rs"]
mod pairing_failure;
#[path = "cli/power.rs"]
mod power;
#[path = "cli/relative_min_effect.rs"]
mod relative_min_effect;
#[path = "cli/sprt.rs"]
mod sprt;
#[path = "cli/time_sensitive.rs"]
mod time_sensitive;
#[path = "cli/verify_run.rs"]
mod verify_run;
