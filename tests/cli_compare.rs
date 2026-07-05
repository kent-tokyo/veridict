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
fn sprt_rejects_elo0_not_less_than_elo1() {
    veridict()
        .args(["sprt", "-", "--elo0", "10", "--elo1", "0"])
        .write_stdin("{\"result\":\"draw\"}\n")
        .assert()
        .code(3)
        .stderr(predicate::str::contains("error:"));
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

fn write_winloss_file(path: &std::path::Path, candidate_wins: usize, baseline_wins: usize) {
    let mut content = String::new();
    for _ in 0..candidate_wins {
        content.push_str("{\"result\":\"candidate_win\"}\n");
    }
    for _ in 0..baseline_wins {
        content.push_str("{\"result\":\"baseline_win\"}\n");
    }
    std::fs::write(path, content).unwrap();
}

#[test]
fn matrix_tabulates_direct_and_extrapolated_cells() {
    let dir = std::env::temp_dir();
    let a = dir.join("veridict_cli_test_matrix_a.jsonl");
    let b = dir.join("veridict_cli_test_matrix_b.jsonl");
    write_winloss_file(&a, 30, 10);
    write_winloss_file(&b, 10, 30);
    veridict()
        .args(["matrix", a.to_str().unwrap(), b.to_str().unwrap()])
        .assert()
        .code(0)
        .stdout(predicate::str::contains(
            "\"row\": \"veridict_cli_test_matrix_a\"",
        ))
        .stdout(predicate::str::contains("\"direct\": false"));
    std::fs::remove_file(&a).ok();
    std::fs::remove_file(&b).ok();
}

#[test]
fn matrix_rejects_duplicate_candidate_names() {
    let dir = std::env::temp_dir().join("veridict_cli_test_matrix_dup");
    std::fs::create_dir_all(&dir).unwrap();
    let a1 = dir.join("same.jsonl");
    let sub = dir.join("nested");
    std::fs::create_dir_all(&sub).unwrap();
    let a2 = sub.join("same.jsonl");
    write_winloss_file(&a1, 5, 5);
    write_winloss_file(&a2, 5, 5);
    veridict()
        .args(["matrix", a1.to_str().unwrap(), a2.to_str().unwrap()])
        .assert()
        .code(3)
        .stderr(predicate::str::contains("duplicate candidate name"));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn matrix_general_graph_reports_real_bootstrap_cis() {
    // A robust 3-node cycle (a beats b, b beats c, c beats a, each 15-5):
    // large enough per-edge margins that resampling essentially never flips
    // an edge's bidirectionality, so every cell should clear the connected-
    // fraction threshold and get a real CI - proving `--matches`'s
    // general-graph path no longer reports `ci_low`/`ci_high: null`
    // unconditionally.
    let dir = std::env::temp_dir();
    let matches_path = dir.join("veridict_cli_test_matrix_bootstrap_ci.jsonl");
    let mut content = String::new();
    for (id_prefix, a, b) in [
        ("ab", "prompt_a", "prompt_b"),
        ("bc", "prompt_b", "prompt_c"),
        ("ca", "prompt_c", "prompt_a"),
    ] {
        for i in 0..15 {
            content.push_str(&format!(
                "{{\"id\":\"{id_prefix}{i}\",\"a\":\"{a}\",\"b\":\"{b}\",\"result\":\"a_win\"}}\n"
            ));
        }
        for i in 0..5 {
            content.push_str(&format!(
                "{{\"id\":\"{id_prefix}_r{i}\",\"a\":\"{a}\",\"b\":\"{b}\",\"result\":\"b_win\"}}\n"
            ));
        }
    }
    std::fs::write(&matches_path, content).unwrap();

    let output = veridict()
        .args([
            "matrix",
            "--matches",
            matches_path.to_str().unwrap(),
            "--resamples",
            "300",
            "--seed",
            "1",
        ])
        .assert()
        .code(0)
        .get_output()
        .stdout
        .clone();
    std::fs::remove_file(&matches_path).ok();

    // `CandidateSummary.ci_low`/`ci_high` stay `None` by design in
    // general-graph mode (an individual rating is only meaningful relative
    // to an arbitrary per-component pin) - it's specifically the `matrix`
    // array's per-pair `elo_diff` CIs that this feature populates.
    let json: serde_json::Value = serde_json::from_slice(&output).unwrap();
    let matrix = json["matrix"].as_array().unwrap();
    assert_eq!(matrix.len(), 3, "expected all 3 pairwise cells");
    for cell in matrix {
        assert_eq!(cell["status"], "direct");
        assert!(
            cell["ci_low"].is_number() && cell["ci_high"].is_number(),
            "expected a real bootstrap CI for {cell}"
        );
    }
}

#[test]
fn paired_by_id_nets_asymmetric_pairs_to_a_pass() {
    let stdin: String = (0..30)
        .flat_map(|i| {
            [
                format!("{{\"id\":\"op{i}\",\"result\":\"candidate_win\"}}\n"),
                format!("{{\"id\":\"op{i}\",\"result\":\"draw\"}}\n"),
            ]
        })
        .collect();
    veridict()
        .args([
            "compare",
            "-",
            "--metric",
            "winrate",
            "--paired-by-id",
            "--min-effect",
            "0.05",
        ])
        .write_stdin(stdin)
        .assert()
        .code(0)
        .stdout(predicate::str::contains("\"paired_count\": 30"));
}

#[test]
fn paired_by_id_allows_duplicate_ids_mean_diff_otherwise_rejects() {
    let stdin = "{\"id\":\"dup\",\"baseline\":1.0,\"candidate\":1.1}\n{\"id\":\"dup\",\"baseline\":2.0,\"candidate\":2.1}\n";
    veridict()
        .args(["compare", "-", "--metric", "mean-diff", "--paired-by-id"])
        .write_stdin(stdin)
        .assert()
        .code(predicate::in_iter([0, 1, 2]))
        .stdout(predicate::str::contains("\"paired_count\": 1"));
}

#[test]
fn paired_by_id_rejects_triple_id() {
    let stdin = "{\"id\":\"op1\",\"result\":\"candidate_win\"}\n{\"id\":\"op1\",\"result\":\"candidate_win\"}\n{\"id\":\"op1\",\"result\":\"candidate_win\"}\n";
    veridict()
        .args(["compare", "-", "--metric", "winrate", "--paired-by-id"])
        .write_stdin(stdin)
        .assert()
        .code(3)
        .stderr(predicate::str::contains("paired-by-id"));
}

#[test]
fn sprt_paired_by_id_nets_split_pairs_to_a_draw() {
    let stdin: String = (0..1000)
        .flat_map(|i| {
            [
                format!("{{\"id\":\"op{i}\",\"result\":\"candidate_win\"}}\n"),
                format!("{{\"id\":\"op{i}\",\"result\":\"baseline_win\"}}\n"),
            ]
        })
        .collect();
    veridict()
        .args(["sprt", "-", "--elo0", "0", "--elo1", "10", "--paired-by-id"])
        .write_stdin(stdin)
        .assert()
        .code(2)
        .stdout(predicate::str::contains("\"llr\": 0.0"));
}

#[test]
fn ci_method_exact_widens_the_interval_versus_wilson() {
    let stdin = "{\"result\":\"candidate_win\"}\n{\"result\":\"candidate_win\"}\n{\"result\":\"candidate_win\"}\n{\"result\":\"candidate_win\"}\n{\"result\":\"baseline_win\"}\n";
    let wilson = veridict()
        .args([
            "compare",
            "-",
            "--metric",
            "winrate",
            "--ci-method",
            "wilson",
        ])
        .write_stdin(stdin)
        .output()
        .unwrap();
    let exact = veridict()
        .args([
            "compare",
            "-",
            "--metric",
            "winrate",
            "--ci-method",
            "exact",
        ])
        .write_stdin(stdin)
        .output()
        .unwrap();
    assert_ne!(wilson.stdout, exact.stdout);
}

#[test]
fn ci_method_exact_rejects_elo() {
    let stdin = "{\"result\":\"candidate_win\"}\n{\"result\":\"baseline_win\"}\n";
    veridict()
        .args(["compare", "-", "--metric", "elo", "--ci-method", "exact"])
        .write_stdin(stdin)
        .assert()
        .code(3)
        .stderr(predicate::str::contains("--ci-method exact"));
}

#[test]
fn ci_method_exact_rejects_mean_diff() {
    let stdin = "{\"baseline\":1.0,\"candidate\":1.1}\n";
    veridict()
        .args([
            "compare",
            "-",
            "--metric",
            "mean-diff",
            "--ci-method",
            "exact",
        ])
        .write_stdin(stdin)
        .assert()
        .code(3)
        .stderr(predicate::str::contains("--ci-method exact"));
}

#[test]
fn ci_method_jeffreys_differs_from_wilson() {
    let stdin = "{\"result\":\"candidate_win\"}\n{\"result\":\"candidate_win\"}\n{\"result\":\"candidate_win\"}\n{\"result\":\"candidate_win\"}\n{\"result\":\"baseline_win\"}\n";
    let wilson = veridict()
        .args([
            "compare",
            "-",
            "--metric",
            "winrate",
            "--ci-method",
            "wilson",
        ])
        .write_stdin(stdin)
        .output()
        .unwrap();
    let jeffreys = veridict()
        .args([
            "compare",
            "-",
            "--metric",
            "winrate",
            "--ci-method",
            "jeffreys",
        ])
        .write_stdin(stdin)
        .output()
        .unwrap();
    assert_ne!(wilson.stdout, jeffreys.stdout);
}

#[test]
fn ci_method_jeffreys_rejects_elo() {
    let stdin = "{\"result\":\"candidate_win\"}\n{\"result\":\"baseline_win\"}\n";
    veridict()
        .args(["compare", "-", "--metric", "elo", "--ci-method", "jeffreys"])
        .write_stdin(stdin)
        .assert()
        .code(3)
        .stderr(predicate::str::contains("--ci-method jeffreys"));
}

#[test]
fn schema_version_appears_in_compare_sprt_and_matrix_reports() {
    let stdin = "{\"result\":\"candidate_win\"}\n{\"result\":\"baseline_win\"}\n";
    veridict()
        .args(["compare", "-", "--metric", "winrate"])
        .write_stdin(stdin)
        .assert()
        .stdout(predicate::str::contains("\"schema_version\": 1"));
    veridict()
        .args(["sprt", "-", "--elo0", "0", "--elo1", "10"])
        .write_stdin(stdin)
        .assert()
        .stdout(predicate::str::contains("\"schema_version\": 1"));
    veridict()
        .args(["matrix", "examples/winloss.jsonl"])
        .assert()
        .stdout(predicate::str::contains("\"schema_version\": 1"));
}

#[test]
fn bootstrap_method_bca_differs_from_percentile_on_skewed_data() {
    let diffs = [
        0.05, 0.08, 0.12, 0.02, 0.15, 0.01, 0.30, 0.04, 0.06, 0.50, 0.03, 0.09, 0.11, 0.07, 0.02,
        0.60, 0.04, 0.08, 0.10, 0.05,
    ];
    let stdin: String = diffs
        .iter()
        .enumerate()
        .map(|(i, d)| format!("{{\"baseline\":{i}.0,\"candidate\":{}}}\n", i as f64 + d))
        .collect();
    let percentile = veridict()
        .args([
            "compare",
            "-",
            "--metric",
            "mean-diff",
            "--bootstrap-method",
            "percentile",
        ])
        .write_stdin(stdin.clone())
        .output()
        .unwrap();
    let bca = veridict()
        .args([
            "compare",
            "-",
            "--metric",
            "mean-diff",
            "--bootstrap-method",
            "bca",
        ])
        .write_stdin(stdin)
        .output()
        .unwrap();
    assert_ne!(percentile.stdout, bca.stdout);
}

#[test]
fn bootstrap_method_basic_differs_from_percentile_on_skewed_data() {
    let diffs = [
        0.05, 0.08, 0.12, 0.02, 0.15, 0.01, 0.30, 0.04, 0.06, 0.50, 0.03, 0.09, 0.11, 0.07, 0.02,
        0.60, 0.04, 0.08, 0.10, 0.05,
    ];
    let stdin: String = diffs
        .iter()
        .enumerate()
        .map(|(i, d)| format!("{{\"baseline\":{i}.0,\"candidate\":{}}}\n", i as f64 + d))
        .collect();
    let percentile = veridict()
        .args([
            "compare",
            "-",
            "--metric",
            "mean-diff",
            "--bootstrap-method",
            "percentile",
        ])
        .write_stdin(stdin.clone())
        .output()
        .unwrap();
    let basic = veridict()
        .args([
            "compare",
            "-",
            "--metric",
            "mean-diff",
            "--bootstrap-method",
            "basic",
        ])
        .write_stdin(stdin)
        .output()
        .unwrap();
    assert_ne!(percentile.stdout, basic.stdout);
}

#[test]
fn tiny_sample_warning_appears_in_json_report() {
    let stdin = (0..10)
        .map(|_| "{\"result\":\"candidate_win\"}\n")
        .collect::<String>();
    veridict()
        .args([
            "compare",
            "-",
            "--metric",
            "winrate",
            "--min-effect",
            "0.01",
        ])
        .write_stdin(stdin)
        .assert()
        .stdout(predicate::str::contains("small sample"));
}

#[test]
fn clean_large_sample_has_empty_warnings_array() {
    let stdin = (0..40)
        .map(|_| "{\"result\":\"candidate_win\"}\n")
        .collect::<String>();
    veridict()
        .args([
            "compare",
            "-",
            "--metric",
            "winrate",
            "--min-effect",
            "0.01",
        ])
        .write_stdin(stdin)
        .assert()
        .stdout(predicate::str::contains("\"warnings\": []"));
}
