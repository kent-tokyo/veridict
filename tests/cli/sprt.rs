use super::*;

#[test]
fn sprt_clear_h1_stream_passes() {
    let stdin = (0..200)
        .map(|_| "{\"result\":\"candidate_win\"}\n")
        .collect::<String>();
    veridict()
        .args(["sprt", "-", "--elo0", "0", "--elo1", "10"])
        .write_stdin(stdin)
        .assert()
        .code(0)
        .stdout(predicate::str::contains("\"verdict\": \"pass\""));
}

#[test]
fn sprt_clear_h0_stream_fails() {
    let stdin = (0..200)
        .map(|_| "{\"result\":\"baseline_win\"}\n")
        .collect::<String>();
    veridict()
        .args(["sprt", "-", "--elo0", "0", "--elo1", "10"])
        .write_stdin(stdin)
        .assert()
        .code(1)
        .stdout(predicate::str::contains("\"verdict\": \"fail\""));
}

#[test]
fn sprt_small_sample_stays_inconclusive() {
    let stdin = "{\"result\":\"candidate_win\"}\n{\"result\":\"baseline_win\"}\n";
    veridict()
        .args(["sprt", "-", "--elo0", "0", "--elo1", "10"])
        .write_stdin(stdin)
        .assert()
        .code(2)
        .stdout(predicate::str::contains("\"verdict\": \"inconclusive\""));
}

#[test]
fn sprt_wald_keeps_the_first_crossing_and_reports_the_ignored_suffix() {
    let output = veridict()
        .args([
            "sprt",
            "tests/fixtures/sprt_cross_then_revert.jsonl",
            "--elo0",
            "0",
            "--elo1",
            "100",
            "--alpha",
            "0.4",
            "--beta",
            "0.4",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["verdict"], "pass");
    assert_eq!(report["candidate_wins"], 2);
    assert_eq!(report["baseline_wins"], 2);
    assert_eq!(report["decision_candidate_wins"], 2);
    assert_eq!(report["decision_baseline_wins"], 0);
    assert_eq!(report["available_observation_count"], 4);
    assert_eq!(report["analyzed_observation_count"], 2);
    assert_eq!(report["stopping_observation_count"], 2);
    assert_eq!(report["stopping_observation_reason"], "upper_bound_crossed");
    assert_eq!(report["ignored_observations_after_stop"], 2);
    assert_eq!(report["ignored_records_after_stop"], 2);
}

#[test]
fn sprt_wald_and_trinomial_use_input_order_as_the_schedule() {
    let wald_reversed = veridict()
        .args([
            "sprt",
            "tests/fixtures/sprt_same_totals_reverse_order.jsonl",
            "--elo0",
            "0",
            "--elo1",
            "100",
            "--alpha",
            "0.4",
            "--beta",
            "0.4",
        ])
        .output()
        .unwrap();
    assert_eq!(wald_reversed.status.code(), Some(1));
    let wald_report: serde_json::Value = serde_json::from_slice(&wald_reversed.stdout).unwrap();
    assert_eq!(wald_report["verdict"], "fail");
    assert_eq!(wald_report["stopping_observation_count"], 2);

    for (fixture, expected_code, expected_verdict) in [
        ("tests/fixtures/sprt_cross_then_revert.jsonl", 0, "pass"),
        (
            "tests/fixtures/sprt_same_totals_reverse_order.jsonl",
            1,
            "fail",
        ),
    ] {
        let output = veridict()
            .args([
                "sprt",
                fixture,
                "--sprt-variant",
                "trinomial",
                "--belo0",
                "0",
                "--belo1",
                "100",
                "--alpha",
                "0.4",
                "--beta",
                "0.4",
            ])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(expected_code));
        let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(report["verdict"], expected_verdict);
        assert_eq!(report["stopping_observation_count"], 2);
    }
}

#[test]
fn sprt_rejects_elo0_not_less_than_elo1() {
    veridict()
        .args(["sprt", "-", "--elo0", "10", "--elo1", "0"])
        .write_stdin("{\"result\":\"draw\"}\n")
        .assert()
        .code(3)
        .stderr(predicate::str::contains("error:"));
}

#[test]
fn sprt_rejects_alpha_beta_sum_at_or_above_one() {
    veridict()
        .args([
            "sprt", "-", "--elo0", "0", "--elo1", "10", "--alpha", "0.9", "--beta", "0.9",
        ])
        .write_stdin("{\"result\":\"candidate_win\"}\n")
        .assert()
        .code(3)
        .stderr(predicate::str::contains("alpha + beta must be less than 1"));
}

#[test]
fn sprt_rejects_numeric_settings_that_would_serialize_as_null() {
    veridict()
        .args(["sprt", "-", "--elo0", "0", "--elo1", "1e308"])
        .write_stdin("{\"result\":\"baseline_win\"}\n")
        .assert()
        .code(3)
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains("representable probability"));

    veridict()
        .args([
            "sprt", "-", "--elo0", "0", "--elo1", "10", "--alpha", "5e-324",
        ])
        .write_stdin("{\"result\":\"candidate_win\"}\n")
        .assert()
        .code(3)
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains("finite SPRT bounds"));
}

#[test]
fn sprt_trinomial_variant_reports_drawelo() {
    let stdin = "{\"result\":\"candidate_win\"}\n{\"result\":\"draw\"}\n{\"result\":\"draw\"}\n{\"result\":\"baseline_win\"}\n";
    veridict()
        .args([
            "sprt",
            "-",
            "--sprt-variant",
            "trinomial",
            "--belo0",
            "0",
            "--belo1",
            "10",
        ])
        .write_stdin(stdin)
        .assert()
        .stdout(predicate::str::contains("\"drawelo\":"));
}

#[test]
fn sprt_trinomial_requires_belo_flags() {
    veridict()
        .args(["sprt", "-", "--sprt-variant", "trinomial"])
        .write_stdin("{\"result\":\"draw\"}\n")
        .assert()
        .code(3)
        .stderr(predicate::str::contains("--belo0 and --belo1 are required"));
}

#[test]
fn sprt_wald_rejects_belo_flags() {
    veridict()
        .args(["sprt", "-", "--belo0", "0", "--belo1", "10"])
        .write_stdin("{\"result\":\"draw\"}\n")
        .assert()
        .code(3)
        .stderr(predicate::str::contains(
            "--belo0/--belo1 are only used with --sprt-variant trinomial",
        ));
}

#[test]
fn sprt_report_md_flag_writes_markdown_file() {
    let path = std::env::temp_dir().join("veridict_cli_test_sprt_report.md");
    let stdin = (0..200)
        .map(|_| "{\"result\":\"candidate_win\"}\n")
        .collect::<String>();
    veridict()
        .args([
            "sprt",
            "-",
            "--elo0",
            "0",
            "--elo1",
            "10",
            "--report-md",
            path.to_str().unwrap(),
        ])
        .write_stdin(stdin)
        .assert()
        .code(0);
    let contents = std::fs::read_to_string(&path).unwrap();
    assert!(contents.contains("# Veridict SPRT Report"));
    std::fs::remove_file(&path).ok();
}

#[test]
fn sprt_low_id_diversity_is_reported_in_json_and_markdown() {
    let path = std::env::temp_dir().join("veridict_cli_test_sprt_id_warning.md");
    let mut stdin = "{\"id\":\"dup\",\"result\":\"candidate_win\"}\n".repeat(5);
    for i in 0..7 {
        stdin.push_str(&format!(
            "{{\"id\":\"unique-{i}\",\"result\":\"candidate_win\"}}\n"
        ));
    }

    veridict()
        .args([
            "sprt",
            "-",
            "--elo0",
            "0",
            "--elo1",
            "10",
            "--report-md",
            path.to_str().unwrap(),
        ])
        .write_stdin(stdin)
        .assert()
        .stdout(predicate::str::contains("\"low_id_diversity\": true"))
        .stdout(predicate::str::contains("low id diversity"));

    let markdown = std::fs::read_to_string(&path).unwrap();
    assert!(markdown.contains("Warnings:\n"));
    assert!(markdown.contains("low id diversity"));
    std::fs::remove_file(&path).ok();
}
