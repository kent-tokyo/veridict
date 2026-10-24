use super::*;

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
fn power_mean_diff_requires_assume_sd_or_pilot() {
    // mean-diff is a valid --metric value now (unlike when this was rejected at the clap
    // level) - but it still needs one of --assume-sd/--pilot at runtime, since there's no real
    // data pre-experiment to estimate a standard deviation from otherwise.
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
        .code(3)
        .stderr(predicate::str::contains("--assume-sd"))
        .stderr(predicate::str::contains("--pilot"));
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

// --- power --metric mean-diff (--assume-sd / --pilot) ---

#[test]
fn power_mean_diff_with_assume_sd_produces_valid_json() {
    veridict()
        .args([
            "power",
            "--metric",
            "mean-diff",
            "--min-effect",
            "0.02",
            "--assume-effect",
            "0.10",
            "--assume-sd",
            "0.15",
        ])
        .assert()
        .code(0)
        .stdout(predicate::str::contains("\"metric\": \"mean-diff\""))
        .stdout(predicate::str::contains("\"ci_method\": \"normal\""))
        .stdout(predicate::str::contains(
            "\"method\": \"normal_approximation_closed_form\"",
        ))
        .stdout(predicate::str::contains("\"assume_sd\": 0.15"))
        .stdout(predicate::str::contains("\"sd_source\": \"assume-sd\""));
}

#[test]
fn power_mean_diff_with_pilot_produces_valid_json() {
    veridict()
        .args([
            "power",
            "--metric",
            "mean-diff",
            "--min-effect",
            "0.02",
            "--assume-effect",
            "0.10",
            "--pilot",
            "tests/fixtures/pilot_scores.jsonl",
        ])
        .assert()
        .code(0)
        .stdout(predicate::str::contains("\"sd_source\": \"pilot\""))
        .stdout(predicate::str::contains(
            "below the conventional 30-observation threshold",
        ));
}

#[test]
fn power_mean_diff_requires_exactly_one_of_assume_sd_or_pilot() {
    veridict()
        .args([
            "power",
            "--metric",
            "mean-diff",
            "--min-effect",
            "0.02",
            "--assume-effect",
            "0.10",
            "--assume-sd",
            "0.1",
            "--pilot",
            "tests/fixtures/pilot_scores.jsonl",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("cannot be used with"));
}

#[test]
fn power_mean_diff_rejects_non_positive_assume_sd() {
    veridict()
        .args([
            "power",
            "--metric",
            "mean-diff",
            "--min-effect",
            "0.02",
            "--assume-effect",
            "0.10",
            "--assume-sd",
            "0",
        ])
        .assert()
        .code(3)
        .stderr(predicate::str::contains("--assume-sd"));
}

#[test]
fn power_assume_sd_rejects_non_mean_diff_metric() {
    veridict()
        .args([
            "power",
            "--metric",
            "winrate",
            "--min-effect",
            "0.02",
            "--assume-effect",
            "0.10",
            "--assume-sd",
            "0.15",
        ])
        .assert()
        .code(3)
        .stderr(predicate::str::contains(
            "only used with --metric mean-diff",
        ));
}

#[test]
fn power_pilot_rejects_a_file_with_too_few_usable_diffs() {
    let dir = std::env::temp_dir();
    let path = dir.join("veridict_cli_test_tiny_pilot.jsonl");
    std::fs::write(&path, "{\"baseline\":1.0,\"candidate\":1.5}\n").unwrap();
    veridict()
        .args([
            "power",
            "--metric",
            "mean-diff",
            "--min-effect",
            "0.02",
            "--assume-effect",
            "0.10",
            "--pilot",
            path.to_str().unwrap(),
        ])
        .assert()
        .code(3)
        .stderr(predicate::str::contains("at least 2 are needed"));
    std::fs::remove_file(&path).ok();
}

#[test]
fn power_pilot_rejects_zero_variance_data() {
    let dir = std::env::temp_dir();
    let path = dir.join("veridict_cli_test_zero_variance_pilot.jsonl");
    std::fs::write(
        &path,
        "{\"baseline\":1.0,\"candidate\":2.0}\n{\"baseline\":1.0,\"candidate\":2.0}\n",
    )
    .unwrap();
    veridict()
        .args([
            "power",
            "--metric",
            "mean-diff",
            "--min-effect",
            "0.02",
            "--assume-effect",
            "0.10",
            "--pilot",
            path.to_str().unwrap(),
        ])
        .assert()
        .code(3)
        .stderr(predicate::str::contains("zero variance"));
    std::fs::remove_file(&path).ok();
}

#[test]
fn power_winrate_json_has_no_mean_diff_only_keys() {
    // Exact key-set check, mirroring correction's own precedent: proves mean-diff's new fields
    // don't leak into the already-shipped metrics' JSON shape.
    let output = veridict()
        .args([
            "power",
            "--metric",
            "winrate",
            "--min-effect",
            "0.02",
            "--assume-effect",
            "0.10",
        ])
        .assert()
        .code(0)
        .get_output()
        .stdout
        .clone();
    let json: serde_json::Value = serde_json::from_slice(&output).unwrap();
    let keys: Vec<&str> = json
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert!(!keys.contains(&"assume_sd"), "unexpected key in {keys:?}");
    assert!(!keys.contains(&"sd_source"), "unexpected key in {keys:?}");
}

#[test]
fn power_mean_diff_paired_by_id_nets_pilot_pairs_before_estimating_sd() {
    let dir = std::env::temp_dir();
    let path = dir.join("veridict_cli_test_paired_pilot.jsonl");
    // id "p1" appears twice (diffs 0.4 and 0.2) - a data error in unpaired mode (rejected as
    // DuplicateId, same as `compare` without --paired-by-id), but the whole point of paired mode:
    // nets to a single averaged diff (0.3) instead. A second, singly-occurring id ("p2") keeps
    // paired mode's diff count at 2 (needed for InsufficientPilotData's n>=2 floor) without
    // affecting what's under test.
    std::fs::write(
        &path,
        "{\"id\":\"p1\",\"baseline\":1.0,\"candidate\":1.4}\n\
         {\"id\":\"p1\",\"baseline\":1.0,\"candidate\":1.2}\n\
         {\"id\":\"p2\",\"baseline\":1.0,\"candidate\":1.6}\n",
    )
    .unwrap();
    let unpaired = veridict()
        .args([
            "power",
            "--metric",
            "mean-diff",
            "--min-effect",
            "0.02",
            "--assume-effect",
            "0.10",
            "--pilot",
            path.to_str().unwrap(),
        ])
        .assert()
        .code(3); // repeated "p1" without --paired-by-id -> DuplicateId error
    unpaired.stderr(predicate::str::contains("p1"));

    let paired_output = veridict()
        .args([
            "power",
            "--metric",
            "mean-diff",
            "--min-effect",
            "0.02",
            "--assume-effect",
            "0.10",
            "--pilot",
            path.to_str().unwrap(),
            "--paired-by-id",
        ])
        .assert()
        .code(0)
        .get_output()
        .stdout
        .clone();
    let json: serde_json::Value = serde_json::from_slice(&paired_output).unwrap();
    assert!(json["notes"].as_array().unwrap().iter().any(|n| {
        n.as_str()
            .unwrap()
            .contains("netted into one diff before computing")
    }));
    std::fs::remove_file(&path).ok();
}

#[test]
fn power_sprt_produces_valid_json_and_exits_zero() {
    veridict()
        .args(["power", "--sprt", "--elo0", "0", "--elo1", "20"])
        .assert()
        .code(0)
        .stdout(predicate::str::contains("\"schema_version\": 1"))
        .stdout(predicate::str::contains(
            "\"method\": \"wald_asn_approximation\"",
        ))
        .stdout(predicate::str::contains("expected_trials_under_h0"))
        .stdout(predicate::str::contains("expected_trials_under_h1"));
}

#[test]
fn power_sprt_horizon_adds_the_probability_field() {
    veridict()
        .args([
            "power",
            "--sprt",
            "--elo0",
            "0",
            "--elo1",
            "20",
            "--horizon",
            "500",
        ])
        .assert()
        .code(0)
        .stdout(predicate::str::contains("\"horizon\": 500"))
        .stdout(predicate::str::contains(
            "\"probability_no_decision_by_horizon\":",
        ));
}

#[test]
fn power_sprt_without_horizon_omits_the_new_fields() {
    veridict()
        .args(["power", "--sprt", "--elo0", "0", "--elo1", "20"])
        .assert()
        .code(0)
        .stdout(predicate::str::contains("\"horizon\": null").not())
        .stdout(predicate::str::contains("probability_no_decision_by_horizon").not());
}

#[test]
fn power_horizon_requires_sprt() {
    veridict()
        .args([
            "power",
            "--metric",
            "winrate",
            "--min-effect",
            "0.02",
            "--assume-effect",
            "0.1",
            "--horizon",
            "500",
        ])
        .assert()
        .code(3)
        .stderr(predicate::str::contains("--sprt"));
}

#[test]
fn power_sprt_rejects_elo0_greater_than_elo1_via_reused_sprt_config_validation() {
    veridict()
        .args(["power", "--sprt", "--elo0", "20", "--elo1", "0"])
        .assert()
        .code(3)
        .stderr(predicate::str::contains("elo0"))
        .stderr(predicate::str::contains("elo1"));
}

#[test]
fn power_sprt_rejects_alpha_beta_sum_at_or_above_one() {
    veridict()
        .args([
            "power", "--sprt", "--elo0", "0", "--elo1", "20", "--alpha", "0.5", "--beta", "0.5",
        ])
        .assert()
        .code(3)
        .stderr(predicate::str::contains("alpha + beta must be less than 1"));
}

#[test]
fn power_sprt_conflicts_with_metric() {
    veridict()
        .args([
            "power", "--sprt", "--elo0", "0", "--elo1", "20", "--metric", "elo",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("cannot be used with"));
}

#[test]
fn power_sprt_requires_elo1_alongside_elo0() {
    veridict()
        .args(["power", "--sprt", "--elo0", "0"])
        .assert()
        .failure();
}

#[test]
fn power_sprt_report_md_flag_writes_markdown_file() {
    let dir = std::env::temp_dir();
    let md = dir.join("veridict_cli_test_power_sprt_md_report.md");
    veridict()
        .args([
            "power",
            "--sprt",
            "--elo0",
            "0",
            "--elo1",
            "20",
            "--report-md",
            md.to_str().unwrap(),
        ])
        .assert()
        .code(0);
    let contents = std::fs::read_to_string(&md).unwrap();
    assert!(contents.contains("# Veridict Power (SPRT)"));
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
