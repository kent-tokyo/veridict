use super::*;

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
fn mean_diff_overflow_is_an_input_error_not_a_pass() {
    veridict()
        .args(["compare", "-", "--metric", "mean-diff", "--min-effect", "1"])
        .write_stdin("{\"id\":\"x\",\"baseline\":-1e308,\"candidate\":1e308}\n")
        .assert()
        .code(3)
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains("candidate_minus_baseline"));
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
fn one_mean_diff_observation_is_inconclusive_even_with_a_point_interval() {
    veridict()
        .args([
            "compare",
            "-",
            "--metric",
            "mean-diff",
            "--min-effect",
            "0.01",
        ])
        .write_stdin("{\"baseline\":1.0,\"candidate\":1.1}\n")
        .assert()
        .code(2)
        .stdout(predicate::str::contains("\"verdict\": \"inconclusive\""))
        .stdout(predicate::str::contains("at least 2 are required"));
}

#[test]
fn one_paired_elo_pair_is_inconclusive_even_with_a_point_interval() {
    veridict()
        .args([
            "compare",
            "-",
            "--metric",
            "paired-elo",
            "--paired-by-id",
            "--min-effect",
            "10",
        ])
        .write_stdin(
            "{\"id\":\"p\",\"result\":\"candidate_win\"}\n\
             {\"id\":\"p\",\"result\":\"candidate_win\"}\n",
        )
        .assert()
        .code(2)
        .stdout(predicate::str::contains("\"verdict\": \"inconclusive\""))
        .stdout(predicate::str::contains("independent pair"));
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
fn fail_below_accepts_a_negative_value() {
    // Regression test: --fail-below/--pass-above lacked allow_hyphen_values,
    // so a negative --fail-below (the documented "Regression gate" README
    // example, and the only sensible value for a below-zero fail threshold)
    // was misparsed by clap as an unknown flag rather than a numeric value.
    let stdin = (0..20)
        .map(|_| "{\"result\":\"candidate_win\"}\n")
        .collect::<String>();
    veridict()
        .args([
            "compare",
            "-",
            "--metric",
            "winrate",
            "--fail-below",
            "-0.01",
            "--pass-above",
            "0.02",
        ])
        .write_stdin(stdin)
        .assert()
        .code(predicate::in_iter([0, 1, 2]));
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

#[test]
fn elo_metric_clear_pass_exits_zero() {
    let stdin = (0..30)
        .map(|_| "{\"result\":\"candidate_win\"}\n")
        .chain((0..10).map(|_| "{\"result\":\"baseline_win\"}\n"))
        .collect::<String>();
    veridict()
        .args(["compare", "-", "--metric", "elo", "--min-effect", "10"])
        .write_stdin(stdin)
        .assert()
        .code(0)
        .stdout(predicate::str::contains("\"metric\": \"elo\""));
}

#[test]
fn paired_elo_preserves_one_point_five_and_two_point_buckets() {
    let fixture = format!(
        "{}/tests/fixtures/paired_elo_pair_scores.jsonl",
        env!("CARGO_MANIFEST_DIR")
    );
    let output = Command::cargo_bin("veridict")
        .unwrap()
        .args([
            "compare",
            &fixture,
            "--metric",
            "paired-elo",
            "--paired-by-id",
            "--seed",
            "42",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["metric"], "paired-elo");
    assert_eq!(report["paired_count"], 2);
    assert_eq!(report["candidate_count"], 3);
    assert_eq!(report["pair_score_counts"]["score_1_5"], 1);
    assert_eq!(report["pair_score_counts"]["score_2_0"], 1);
    assert_eq!(report["pair_score_counts"]["score_1_0"], 0);
    assert!((report["effect"].as_f64().unwrap() - 338.04).abs() < 0.1);

    // Legacy paired Elo intentionally nets both pairs to `candidate_win`; the opt-in metric must
    // stay observably different rather than silently changing that established behavior.
    let legacy = Command::cargo_bin("veridict")
        .unwrap()
        .args([
            "compare",
            &fixture,
            "--metric",
            "elo",
            "--paired-by-id",
            "--seed",
            "42",
        ])
        .output()
        .unwrap();
    let legacy_report: serde_json::Value = serde_json::from_slice(&legacy.stdout).unwrap();
    assert_eq!(legacy_report["paired_count"], 2);
    assert!(legacy_report.get("pair_score_counts").is_none());
    assert!(legacy_report["effect"].as_f64().unwrap() > 1_000.0);
}

#[test]
fn paired_elo_requires_paired_by_id() {
    Command::cargo_bin("veridict")
        .unwrap()
        .args(["compare", "-", "--metric", "paired-elo"])
        .write_stdin(
            "{\"id\":\"p1\",\"result\":\"candidate_win\"}\n\
             {\"id\":\"p1\",\"result\":\"draw\"}\n",
        )
        .assert()
        .code(3)
        .stderr(predicate::str::contains("requires --paired-by-id"));
}

#[test]
fn paired_elo_rejects_zero_bootstrap_resamples_without_panicking() {
    Command::cargo_bin("veridict")
        .unwrap()
        .args([
            "compare",
            "-",
            "--metric",
            "paired-elo",
            "--paired-by-id",
            "--resamples",
            "0",
        ])
        .write_stdin(
            "{\"id\":\"p1\",\"result\":\"candidate_win\"}\n\
             {\"id\":\"p1\",\"result\":\"draw\"}\n",
        )
        .assert()
        .code(3)
        .stderr(predicate::str::contains(
            "bootstrap resample count 0: must be at least 1",
        ));
}

#[test]
fn paired_elo_rejects_missing_singleton_and_triple_ids() {
    let cases = [
        (
            "{\"result\":\"candidate_win\"}\n{\"id\":\"p1\",\"result\":\"draw\"}\n",
            "requires a non-empty id",
        ),
        (
            "{\"id\":\"p1\",\"result\":\"candidate_win\"}\n",
            "appears once",
        ),
        (
            "{\"id\":\"p1\",\"result\":\"candidate_win\"}\n\
             {\"id\":\"p1\",\"result\":\"draw\"}\n\
             {\"id\":\"p1\",\"result\":\"baseline_win\"}\n",
            "appears more than twice",
        ),
    ];
    for (input, expected) in cases {
        Command::cargo_bin("veridict")
            .unwrap()
            .args(["compare", "-", "--metric", "paired-elo", "--paired-by-id"])
            .write_stdin(input)
            .assert()
            .code(3)
            .stderr(predicate::str::contains(expected));
    }
}

#[test]
fn paired_elo_rejects_a_pair_with_only_one_usable_outcome() {
    Command::cargo_bin("veridict")
        .unwrap()
        .args(["compare", "-", "--metric", "paired-elo", "--paired-by-id"])
        .write_stdin(
            "{\"id\":\"p1\",\"result\":\"candidate_win\"}\n\
             {\"id\":\"p1\",\"candidate_status\":\"timeout\"}\n",
        )
        .assert()
        .code(3)
        .stderr(predicate::str::contains("has 1 usable outcome"));
}
