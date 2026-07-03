//! End-to-end CLI tests: real binary, real exit codes, real stdout.

use assert_cmd::Command;
use predicates::prelude::*;

fn veridict() -> Command {
    Command::cargo_bin("veridict").unwrap()
}

#[test]
fn winrate_clear_pass_exits_zero() {
    let stdin = (0..20)
        .map(|_| "{\"result\":\"candidate_win\"}\n")
        .collect::<String>();
    veridict()
        .args(["compare", "-", "--metric", "winrate", "--min-effect", "0.1"])
        .write_stdin(stdin)
        .assert()
        .code(0)
        .stdout(predicate::str::contains("\"verdict\": \"pass\""));
}

#[test]
fn winrate_clear_fail_exits_one() {
    let stdin = (0..20)
        .map(|_| "{\"result\":\"baseline_win\"}\n")
        .collect::<String>();
    veridict()
        .args(["compare", "-", "--metric", "winrate", "--min-effect", "0.1"])
        .write_stdin(stdin)
        .assert()
        .code(1)
        .stdout(predicate::str::contains("\"verdict\": \"fail\""));
}

#[test]
fn mean_diff_clear_pass_exits_zero() {
    let stdin = (0..20)
        .map(|i| format!("{{\"baseline\":{i}.0,\"candidate\":{}.5}}\n", i))
        .collect::<String>();
    veridict()
        .args([
            "compare",
            "-",
            "--metric",
            "mean-diff",
            "--min-effect",
            "0.1",
        ])
        .write_stdin(stdin)
        .assert()
        .code(0)
        .stdout(predicate::str::contains("\"verdict\": \"pass\""));
}

#[test]
fn small_sample_is_inconclusive_exits_two() {
    let stdin = "{\"result\":\"candidate_win\"}\n{\"result\":\"baseline_win\"}\n";
    veridict()
        .args([
            "compare",
            "-",
            "--metric",
            "winrate",
            "--min-effect",
            "0.02",
        ])
        .write_stdin(stdin)
        .assert()
        .code(2)
        .stdout(predicate::str::contains("\"verdict\": \"inconclusive\""));
}

#[test]
fn malformed_json_exits_three() {
    veridict()
        .args(["compare", "-", "--metric", "winrate"])
        .write_stdin("not json\n")
        .assert()
        .code(3)
        .stderr(predicate::str::contains("error:"));
}

#[test]
fn empty_input_exits_three() {
    veridict()
        .args(["compare", "-", "--metric", "winrate"])
        .write_stdin("")
        .assert()
        .code(3)
        .stderr(predicate::str::contains("no records"));
}

#[test]
fn schema_mismatch_on_example_file_exits_three() {
    veridict()
        .args([
            "compare",
            "examples/status_failures.jsonl",
            "--metric",
            "winrate",
        ])
        .assert()
        .code(3)
        .stderr(predicate::str::contains("line 1"));
}

#[test]
fn example_paired_scores_runs_mean_diff() {
    veridict()
        .args([
            "compare",
            "examples/paired_scores.jsonl",
            "--metric",
            "mean-diff",
        ])
        .assert()
        .code(predicate::in_iter([0, 1, 2]))
        .stdout(predicate::str::contains("\"paired_count\": 5"));
}

#[test]
fn pass_above_requires_fail_below() {
    veridict()
        .args(["compare", "-", "--metric", "winrate", "--pass-above", "0.1"])
        .write_stdin("{\"result\":\"draw\"}\n")
        .assert()
        .failure();
}

#[test]
fn sign_test_clear_pass_exits_zero() {
    let stdin = (0..20)
        .map(|i| format!("{{\"baseline\":{i}.0,\"candidate\":{}.5}}\n", i))
        .collect::<String>();
    veridict()
        .args([
            "compare",
            "-",
            "--metric",
            "sign-test",
            "--min-effect",
            "0.1",
        ])
        .write_stdin(stdin)
        .assert()
        .code(0)
        .stdout(predicate::str::contains("\"metric\": \"sign-test\""));
}

#[test]
fn csv_input_detected_by_extension() {
    let path = std::env::temp_dir().join("veridict_cli_test_winrate.csv");
    let mut csv = "id,result\n".to_string();
    for i in 0..20 {
        csv.push_str(&format!("c{i},candidate_win\n"));
    }
    std::fs::write(&path, csv).unwrap();
    veridict()
        .args([
            "compare",
            path.to_str().unwrap(),
            "--metric",
            "winrate",
            "--min-effect",
            "0.1",
        ])
        .assert()
        .code(0)
        .stdout(predicate::str::contains("\"paired_count\": 20"));
    std::fs::remove_file(&path).ok();
}

#[test]
fn csv_input_via_explicit_format_flag_on_stdin() {
    // 3 samples is too small to clear the default zero threshold, so the
    // verdict is legitimately Inconclusive; this test is about format
    // detection parsing correctly, not about the statistical outcome.
    let stdin = "id,result\na,candidate_win\nb,candidate_win\nc,baseline_win\n";
    veridict()
        .args(["compare", "-", "--metric", "winrate", "--format", "csv"])
        .write_stdin(stdin)
        .assert()
        .code(predicate::in_iter([0, 1, 2]))
        .stdout(predicate::str::contains("\"paired_count\": 3"));
}

#[test]
fn multi_metric_wraps_reports_and_combines_verdict() {
    let stdin = (0..20)
        .map(|i| {
            format!(
                "{{\"baseline\":{i}.0,\"candidate\":{}.5,\"result\":\"candidate_win\"}}\n",
                i
            )
        })
        .collect::<String>();
    veridict()
        .args([
            "compare",
            "-",
            "--metric",
            "winrate",
            "--metric",
            "mean-diff",
            "--min-effect",
            "0.1",
        ])
        .write_stdin(stdin)
        .assert()
        .code(0)
        .stdout(predicate::str::contains("\"reports\""));
}

#[test]
fn different_seed_changes_mean_diff_ci() {
    // Diffs must vary in magnitude (not just a constant offset), otherwise
    // every bootstrap resample lands on the same mean regardless of seed.
    let diffs = [0.1, 2.0, -1.5, 3.2, 0.4, -2.1, 1.8, 0.05, -0.9, 2.7];
    let stdin: String = diffs
        .iter()
        .enumerate()
        .map(|(i, d)| format!("{{\"baseline\":{i}.0,\"candidate\":{}}}\n", i as f64 + d))
        .collect();
    let out_a = veridict()
        .args(["compare", "-", "--metric", "mean-diff", "--seed", "1"])
        .write_stdin(stdin.clone())
        .output()
        .unwrap();
    let out_b = veridict()
        .args(["compare", "-", "--metric", "mean-diff", "--seed", "2"])
        .write_stdin(stdin)
        .output()
        .unwrap();
    assert_ne!(out_a.stdout, out_b.stdout);
}

#[test]
fn report_md_flag_writes_markdown_file() {
    let path = std::env::temp_dir().join("veridict_cli_test_report.md");
    let stdin = "{\"result\":\"candidate_win\"}\n{\"result\":\"candidate_win\"}\n";
    veridict()
        .args([
            "compare",
            "-",
            "--metric",
            "winrate",
            "--report-md",
            path.to_str().unwrap(),
        ])
        .write_stdin(stdin)
        .assert()
        .code(predicate::in_iter([0, 1, 2]));
    let contents = std::fs::read_to_string(&path).unwrap();
    assert!(contents.contains("# Veridict Report"));
    std::fs::remove_file(&path).ok();
}
