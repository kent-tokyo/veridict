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
fn plan_recommends_the_narrower_ci_candidate_last() {
    let dir = std::env::temp_dir();
    let well_sampled = dir.join("veridict_cli_test_plan_a.jsonl");
    let barely_sampled = dir.join("veridict_cli_test_plan_b.jsonl");
    write_winloss_file(&well_sampled, 400, 100);
    write_winloss_file(&barely_sampled, 4, 1);
    let output = veridict()
        .args([
            "plan",
            well_sampled.to_str().unwrap(),
            barely_sampled.to_str().unwrap(),
            "--min-elo",
            "10",
        ])
        .assert()
        .code(0)
        .get_output()
        .stdout
        .clone();
    std::fs::remove_file(&well_sampled).ok();
    std::fs::remove_file(&barely_sampled).ok();

    let json: serde_json::Value = serde_json::from_slice(&output).unwrap();
    let recs = json["recommendations"].as_array().unwrap();
    let pos = |col: &str| {
        recs.iter()
            .position(|r| r["row"] == "baseline" && r["col"] == col)
            .unwrap()
    };
    assert!(
        pos("veridict_cli_test_plan_b") < pos("veridict_cli_test_plan_a"),
        "the barely-sampled candidate should be recommended first: {json}"
    );
}

#[test]
fn plan_rejects_non_positive_min_elo() {
    let dir = std::env::temp_dir();
    let a = dir.join("veridict_cli_test_plan_bad_min_elo.jsonl");
    write_winloss_file(&a, 10, 10);
    veridict()
        .args(["plan", a.to_str().unwrap(), "--min-elo", "0"])
        .assert()
        .code(3)
        .stderr(predicate::str::contains("--min-elo"));
    std::fs::remove_file(&a).ok();
}

#[test]
fn plan_report_md_flag_writes_markdown_file() {
    let dir = std::env::temp_dir();
    let a = dir.join("veridict_cli_test_plan_md.jsonl");
    let md = dir.join("veridict_cli_test_plan_md_report.md");
    write_winloss_file(&a, 20, 5);
    veridict()
        .args([
            "plan",
            a.to_str().unwrap(),
            "--min-elo",
            "10",
            "--report-md",
            md.to_str().unwrap(),
        ])
        .assert()
        .code(0);
    let contents = std::fs::read_to_string(&md).unwrap();
    assert!(contents.contains("# Veridict Plan"));
    std::fs::remove_file(&a).ok();
    std::fs::remove_file(&md).ok();
}

#[test]
fn power_produces_valid_json_and_exits_zero() {
    veridict()
        .args([
            "power",
            "--metric",
            "elo",
            "--min-effect",
            "20",
            "--assume-effect",
            "35",
        ])
        .assert()
        .code(0)
        .stdout(predicate::str::contains("\"schema_version\": 1"))
        .stdout(predicate::str::contains("\"metric\": \"elo\""))
        .stdout(predicate::str::contains(
            "\"method\": \"exact_binomial_search\"",
        ));
}

#[test]
fn power_rejects_assume_effect_not_greater_than_min_effect() {
    veridict()
        .args([
            "power",
            "--metric",
            "elo",
            "--min-effect",
            "20",
            "--assume-effect",
            "15",
        ])
        .assert()
        .code(3)
        .stderr(predicate::str::contains("--assume-effect"));
}

#[test]
fn power_rejects_mean_diff_at_the_clap_parsing_level() {
    veridict()
        .args([
            "power",
            "--metric",
            "mean-diff",
            "--min-effect",
            "0.01",
            "--assume-effect",
            "0.05",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("mean-diff"));
}

#[test]
fn power_rejects_exact_ci_method_for_elo() {
    veridict()
        .args([
            "power",
            "--metric",
            "elo",
            "--ci-method",
            "exact",
            "--min-effect",
            "20",
            "--assume-effect",
            "35",
        ])
        .assert()
        .code(3)
        .stderr(predicate::str::contains("--ci-method exact"));
}

#[test]
fn power_rejects_out_of_range_target_power_with_the_right_flag_named() {
    veridict()
        .args([
            "power",
            "--metric",
            "elo",
            "--min-effect",
            "20",
            "--assume-effect",
            "35",
            "--target-power",
            "1.5",
        ])
        .assert()
        .code(3)
        .stderr(predicate::str::contains("--target-power"));
}

#[test]
fn power_report_md_flag_writes_markdown_file() {
    let dir = std::env::temp_dir();
    let md = dir.join("veridict_cli_test_power_md_report.md");
    veridict()
        .args([
            "power",
            "--metric",
            "winrate",
            "--min-effect",
            "0.02",
            "--assume-effect",
            "0.10",
            "--report-md",
            md.to_str().unwrap(),
        ])
        .assert()
        .code(0);
    let contents = std::fs::read_to_string(&md).unwrap();
    assert!(contents.contains("# Veridict Power"));
    std::fs::remove_file(&md).ok();
}

#[test]
fn power_paired_by_id_adds_a_note_without_changing_the_flag_free_json_shape() {
    let output = veridict()
        .args([
            "power",
            "--metric",
            "winrate",
            "--min-effect",
            "0.02",
            "--assume-effect",
            "0.10",
            "--paired-by-id",
        ])
        .assert()
        .code(0)
        .get_output()
        .stdout
        .clone();
    let json: serde_json::Value = serde_json::from_slice(&output).unwrap();
    let notes = json["notes"].as_array().unwrap();
    assert!(
        notes
            .iter()
            .any(|n| n.as_str().unwrap().contains("paired-by-id"))
    );
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
fn sprt_pentanomial_clear_h1_stream_passes() {
    let stdin: String = (0..200)
        .flat_map(|i| {
            [
                format!("{{\"id\":\"op{i}\",\"result\":\"candidate_win\"}}\n"),
                format!("{{\"id\":\"op{i}\",\"result\":\"candidate_win\"}}\n"),
            ]
        })
        .collect();
    veridict()
        .args([
            "sprt",
            "-",
            "--sprt-variant",
            "pentanomial",
            "--elo0",
            "0",
            "--elo1",
            "10",
            "--paired-by-id",
        ])
        .write_stdin(stdin)
        .assert()
        .code(0)
        .stdout(predicate::str::contains("\"verdict\": \"pass\""))
        .stdout(predicate::str::contains(
            "\"sprt_variant\": \"pentanomial\"",
        ))
        .stdout(predicate::str::contains("\"score_2_0\": 200"));
}

#[test]
fn sprt_pentanomial_requires_paired_by_id() {
    veridict()
        .args([
            "sprt",
            "-",
            "--sprt-variant",
            "pentanomial",
            "--elo0",
            "0",
            "--elo1",
            "10",
        ])
        .write_stdin("{\"id\":\"op1\",\"result\":\"draw\"}\n")
        .assert()
        .code(3)
        .stderr(predicate::str::contains("requires --paired-by-id"));
}

#[test]
fn sprt_pentanomial_rejects_belo_flags() {
    veridict()
        .args([
            "sprt",
            "-",
            "--sprt-variant",
            "pentanomial",
            "--belo0",
            "0",
            "--belo1",
            "10",
            "--paired-by-id",
        ])
        .write_stdin("{\"id\":\"op1\",\"result\":\"draw\"}\n")
        .assert()
        .code(3)
        .stderr(predicate::str::contains(
            "--belo0/--belo1 are only used with --sprt-variant trinomial",
        ));
}

#[test]
fn sprt_pentanomial_rejects_incomplete_pair() {
    let stdin = "{\"id\":\"op1\",\"result\":\"candidate_win\"}\n";
    veridict()
        .args([
            "sprt",
            "-",
            "--sprt-variant",
            "pentanomial",
            "--elo0",
            "0",
            "--elo1",
            "10",
            "--paired-by-id",
        ])
        .write_stdin(stdin)
        .assert()
        .code(3)
        .stderr(predicate::str::contains("pentanomial"));
}

#[test]
fn sprt_pentanomial_rejects_triple_id() {
    let stdin = "{\"id\":\"op1\",\"result\":\"candidate_win\"}\n{\"id\":\"op1\",\"result\":\"candidate_win\"}\n{\"id\":\"op1\",\"result\":\"candidate_win\"}\n";
    veridict()
        .args([
            "sprt",
            "-",
            "--sprt-variant",
            "pentanomial",
            "--elo0",
            "0",
            "--elo1",
            "10",
            "--paired-by-id",
        ])
        .write_stdin(stdin)
        .assert()
        .code(3)
        .stderr(predicate::str::contains("pentanomial"));
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
fn failure_policy_report_only_is_the_default_and_status_only_records_contribute_nothing() {
    let stdin = "{\"result\":\"candidate_win\"}\n{\"result\":\"baseline_win\"}\n\
                 {\"candidate_status\":\"crash\"}\n{\"baseline_status\":\"timeout\"}\n";
    veridict()
        .args(["compare", "-", "--metric", "winrate"])
        .write_stdin(stdin)
        .assert()
        .stdout(predicate::str::contains("\"paired_count\": 2"))
        .stdout(predicate::str::contains("\"crashes\": 1"))
        .stdout(predicate::str::contains("\"timeouts\": 1"));
}

#[test]
fn failure_policy_exclude_drops_a_result_next_to_a_failure_status() {
    // The behavioral divergence report-only/exclude actually have: a record carrying both a
    // failure status and a literal `result`. report-only still counts the result; exclude drops
    // it entirely, shrinking paired_count by one relative to report-only on the same input.
    let stdin = "{\"result\":\"candidate_win\"}\n{\"result\":\"baseline_win\"}\n\
                 {\"candidate_status\":\"crash\",\"result\":\"candidate_win\"}\n";
    let report_only = veridict()
        .args(["compare", "-", "--metric", "winrate"])
        .write_stdin(stdin)
        .output()
        .unwrap();
    let exclude = veridict()
        .args([
            "compare",
            "-",
            "--metric",
            "winrate",
            "--failure-policy",
            "exclude",
        ])
        .write_stdin(stdin)
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&report_only.stdout).contains("\"paired_count\": 3"));
    assert!(String::from_utf8_lossy(&exclude.stdout).contains("\"paired_count\": 2"));
}

#[test]
fn failure_policy_loss_synthesizes_a_baseline_win_on_candidate_failure() {
    let stdin = "{\"candidate_status\":\"crash\"}\n";
    veridict()
        .args([
            "compare",
            "-",
            "--metric",
            "winrate",
            "--failure-policy",
            "loss",
        ])
        .write_stdin(stdin)
        .assert()
        .stdout(predicate::str::contains("\"paired_count\": 1"))
        .stdout(predicate::str::contains("\"baseline_count\": 1"))
        .stdout(predicate::str::contains("\"candidate_count\": 0"));
}

#[test]
fn failure_policy_loss_both_sides_failing_nets_to_a_draw_excluded_from_winrate_n() {
    // winrate excludes draws from its decisive-trial denominator (same "decisive only"
    // convention as sprt wald) - a both-failed pair should net to a draw, not move
    // candidate_count/baseline_count at all.
    let stdin = "{\"baseline_status\":\"timeout\",\"candidate_status\":\"crash\"}\n";
    veridict()
        .args([
            "compare",
            "-",
            "--metric",
            "winrate",
            "--failure-policy",
            "loss",
        ])
        .write_stdin(stdin)
        .assert()
        .stdout(predicate::str::contains("\"paired_count\": 0"))
        .stdout(predicate::str::contains("\"candidate_count\": 0"))
        .stdout(predicate::str::contains("\"baseline_count\": 0"));
}

#[test]
fn failure_policy_loss_overrides_a_literal_result_on_the_same_record() {
    let stdin = "{\"candidate_status\":\"crash\",\"result\":\"candidate_win\"}\n";
    veridict()
        .args([
            "compare",
            "-",
            "--metric",
            "winrate",
            "--failure-policy",
            "loss",
        ])
        .write_stdin(stdin)
        .assert()
        .stdout(predicate::str::contains("\"baseline_count\": 1"))
        .stdout(predicate::str::contains("\"candidate_count\": 0"));
}

// The pairing interaction advisor flagged as the real hazard: a failure *inside* a
// --paired-by-id pair, not just a standalone failed record. Pair "op1": game 1 is a real
// candidate_win, game 2 is a candidate crash with no result. Under `loss`, game 2 synthesizes to
// baseline_win, so the pair nets candidate_win(1.0) + baseline_win(0.0) = 1.0 -> a net draw, the
// same "total points" convention any other paired outcome uses - `OutcomeCollector` never needs
// to know the outcome was synthesized rather than literal.
#[test]
fn failure_policy_loss_nets_correctly_inside_a_paired_by_id_pair() {
    let stdin = "{\"id\":\"op1\",\"result\":\"candidate_win\"}\n\
                 {\"id\":\"op1\",\"candidate_status\":\"crash\"}\n";
    veridict()
        .args([
            "compare",
            "-",
            "--metric",
            "winrate",
            "--paired-by-id",
            "--failure-policy",
            "loss",
        ])
        .write_stdin(stdin)
        .assert()
        .code(2)
        .stdout(predicate::str::contains("\"paired_count\": 0"));
}

#[test]
fn failure_policy_exclude_rejected_for_mean_diff() {
    let stdin = "{\"baseline\":1.0,\"candidate\":1.1}\n";
    veridict()
        .args([
            "compare",
            "-",
            "--metric",
            "mean-diff",
            "--failure-policy",
            "exclude",
        ])
        .write_stdin(stdin)
        .assert()
        .code(3)
        .stderr(predicate::str::contains("--failure-policy exclude"));
}

#[test]
fn failure_policy_loss_rejected_for_sign_test() {
    let stdin = "{\"baseline\":1.0,\"candidate\":1.1}\n";
    veridict()
        .args([
            "compare",
            "-",
            "--metric",
            "sign-test",
            "--failure-policy",
            "loss",
        ])
        .write_stdin(stdin)
        .assert()
        .code(3)
        .stderr(predicate::str::contains("--failure-policy loss"));
}

#[test]
fn failure_policy_loss_rejected_when_any_requested_metric_is_incompatible() {
    // Multiple --metric flags: the incompatible one must be caught regardless of position, not
    // just when it happens to be first.
    let stdin = "{\"baseline\":1.0,\"candidate\":1.1,\"result\":\"candidate_win\"}\n";
    veridict()
        .args([
            "compare",
            "-",
            "--metric",
            "winrate",
            "--metric",
            "mean-diff",
            "--failure-policy",
            "loss",
        ])
        .write_stdin(stdin)
        .assert()
        .code(3)
        .stderr(predicate::str::contains("--failure-policy loss"));
}

#[test]
fn failure_policy_sprt_loss_synthesizes_a_baseline_win_on_candidate_failure() {
    let stdin = (0..200)
        .map(|_| "{\"candidate_status\":\"crash\"}\n")
        .collect::<String>();
    veridict()
        .args([
            "sprt",
            "-",
            "--elo0",
            "0",
            "--elo1",
            "10",
            "--failure-policy",
            "loss",
        ])
        .write_stdin(stdin)
        .assert()
        .code(1)
        .stdout(predicate::str::contains("\"verdict\": \"fail\""))
        .stdout(predicate::str::contains("\"baseline_wins\": 200"));
}

// pentanomial x failure-policy: explicitly decided and tested, not discovered by accident. A
// crash on one side of a pair routes its synthesized outcome into `PentanomialCollector` exactly
// like a literal result would - candidate_win + (crash -> baseline_win) nets to pentanomial
// bucket score_1_0 (the pair's combined score is 1.0), same math as compare's --paired-by-id.
#[test]
fn failure_policy_sprt_pentanomial_loss_synthesizes_outcome_inside_a_pair() {
    let stdin = "{\"id\":\"op1\",\"result\":\"candidate_win\"}\n\
                 {\"id\":\"op1\",\"candidate_status\":\"crash\"}\n";
    veridict()
        .args([
            "sprt",
            "-",
            "--sprt-variant",
            "pentanomial",
            "--elo0",
            "0",
            "--elo1",
            "10",
            "--paired-by-id",
            "--failure-policy",
            "loss",
        ])
        .write_stdin(stdin)
        .assert()
        .stdout(predicate::str::contains("\"score_1_0\": 1"))
        .stdout(predicate::str::contains("\"paired_count\": 1"));
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

#[test]
fn low_id_diversity_warning_appears_when_one_id_dominates_unpaired() {
    // id "dup" repeated 5 times plus 7 distinct ids - the "same test case
    // logged multiple times" pattern the check exists to catch.
    let mut stdin = "{\"id\":\"dup\",\"result\":\"candidate_win\"}\n".repeat(5);
    for i in 0..7 {
        stdin.push_str(&format!(
            "{{\"id\":\"r{i}\",\"result\":\"candidate_win\"}}\n"
        ));
    }
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
        .stdout(predicate::str::contains("low id diversity"))
        .stdout(predicate::str::contains("\"low_id_diversity\": true"));
}

#[test]
fn low_id_diversity_is_silent_when_every_id_appears_exactly_twice() {
    // The common, innocent mistake: genuinely paired data run without
    // --paired-by-id. Must stay silent - firing here would be noise.
    let stdin = (0..6)
        .flat_map(|i| {
            [
                format!("{{\"id\":\"pair{i}\",\"result\":\"candidate_win\"}}\n"),
                format!("{{\"id\":\"pair{i}\",\"result\":\"candidate_win\"}}\n"),
            ]
        })
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
        .stdout(predicate::str::contains("\"low_id_diversity\": false"))
        .stdout(predicate::str::contains("low id diversity").not());
}
