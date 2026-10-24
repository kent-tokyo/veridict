use super::*;

#[test]
fn relative_diff_reports_a_ratio_effect_and_passes_on_a_five_percent_min_effect() {
    let stdin = "{\"baseline\":100.0,\"candidate\":110.0}\n{\"baseline\":200.0,\"candidate\":220.0}\n{\"baseline\":50.0,\"candidate\":55.0}\n";
    let output = veridict()
        .args([
            "compare",
            "-",
            "--metric",
            "relative-diff",
            "--min-effect",
            "0.05",
        ])
        .write_stdin(stdin)
        .assert()
        .code(0)
        .get_output()
        .stdout
        .clone();
    let json: serde_json::Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(json["metric"], "relative-diff");
    assert!((json["effect"].as_f64().unwrap() - 0.10).abs() < 1e-9);
}

#[test]
fn relative_diff_inconclusive_kind_is_directional_in_the_dead_zone() {
    // Every pair has the identical +0.3% relative diff: the bootstrap CI collapses to a single
    // point at 0.003, which excludes zero but sits inside a +-0.5% dead zone.
    let stdin: String = (0..20)
        .map(|_| "{\"baseline\":100.0,\"candidate\":100.3}\n")
        .collect();
    let output = veridict()
        .args([
            "compare",
            "-",
            "--metric",
            "relative-diff",
            "--min-effect",
            "0.005",
        ])
        .write_stdin(stdin)
        .assert()
        .code(2)
        .get_output()
        .stdout
        .clone();
    let json: serde_json::Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(json["verdict"], "inconclusive");
    assert_eq!(json["inconclusive_kind"], "directional");
    assert!(json["estimated_additional_trials"].is_null());
}

#[test]
fn relative_diff_inconclusive_kind_is_noise_when_ci_straddles_zero() {
    let stdin: String = (0..20)
        .map(|i| {
            let pct = if i % 2 == 0 { 101.0 } else { 99.0 };
            format!("{{\"baseline\":100.0,\"candidate\":{pct}}}\n")
        })
        .collect();
    let output = veridict()
        .args([
            "compare",
            "-",
            "--metric",
            "relative-diff",
            "--min-effect",
            "0.5",
        ])
        .write_stdin(stdin)
        .assert()
        .code(2)
        .get_output()
        .stdout
        .clone();
    let json: serde_json::Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(json["verdict"], "inconclusive");
    assert_eq!(json["inconclusive_kind"], "noise");
}

#[test]
fn relative_diff_tied_count_and_diluted_by_ties_appear_in_json() {
    let mut stdin = String::new();
    stdin.push_str("{\"baseline\":100.0,\"candidate\":110.0}\n");
    stdin.push_str("{\"baseline\":100.0,\"candidate\":110.0}\n");
    for _ in 0..8 {
        stdin.push_str("{\"baseline\":100.0,\"candidate\":100.0}\n");
    }
    let output = veridict()
        .args(["compare", "-", "--metric", "relative-diff"])
        .write_stdin(stdin)
        .output()
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["tied_count"], 8);
    assert_eq!(json["data_quality"]["diluted_by_ties"], true);
}

#[test]
fn relative_diff_markdown_uses_percent_not_winrates_percentage_points() {
    let stdin =
        "{\"baseline\":100.0,\"candidate\":110.0}\n{\"baseline\":200.0,\"candidate\":220.0}\n";
    let md_path = std::env::temp_dir().join("veridict_cli_test_relative_diff.md");
    veridict()
        .args([
            "compare",
            "-",
            "--metric",
            "relative-diff",
            "--min-effect",
            "0.05",
            "--report-md",
            md_path.to_str().unwrap(),
        ])
        .write_stdin(stdin)
        .assert()
        .code(0);
    let md = std::fs::read_to_string(&md_path).unwrap();
    std::fs::remove_file(&md_path).ok();
    assert!(
        md.contains("+10.0%"),
        "expected a percent-formatted effect, got:\n{md}"
    );
    assert!(
        !md.contains("pp"),
        "relative-diff must not use winrate's pp suffix:\n{md}"
    );
}

#[test]
fn relative_diff_markdown_verdict_line_shows_inconclusive_kind() {
    let stdin: String = (0..20)
        .map(|_| "{\"baseline\":100.0,\"candidate\":100.3}\n")
        .collect();
    let md_path = std::env::temp_dir().join("veridict_cli_test_inconclusive_kind.md");
    veridict()
        .args([
            "compare",
            "-",
            "--metric",
            "relative-diff",
            "--min-effect",
            "0.005",
            "--report-md",
            md_path.to_str().unwrap(),
        ])
        .write_stdin(stdin)
        .assert()
        .code(2);
    let md = std::fs::read_to_string(&md_path).unwrap();
    std::fs::remove_file(&md_path).ok();
    assert!(
        md.contains("Verdict: inconclusive (directional)"),
        "expected the verdict line to carry the inconclusive_kind suffix, got:\n{md}"
    );
}

#[test]
fn relative_diff_rejects_zero_baseline_exits_three() {
    veridict()
        .args(["compare", "-", "--metric", "relative-diff"])
        .write_stdin("{\"baseline\":0.0,\"candidate\":1.0}\n")
        .assert()
        .code(3)
        .stderr(predicate::str::contains("baseline > 0"));
}

#[test]
fn relative_diff_rejects_negative_baseline_exits_three() {
    veridict()
        .args(["compare", "-", "--metric", "relative-diff"])
        .write_stdin("{\"baseline\":-5.0,\"candidate\":1.0}\n")
        .assert()
        .code(3)
        .stderr(predicate::str::contains("baseline > 0"));
}

#[test]
fn relative_diff_paired_by_id_averages_ratios_not_raw_values() {
    // Each id: +10% then -2% -> paired relative observation +4%, not the ratio of averaged
    // baseline/candidate. Use two independent pairs so the fixed-sample safety floor is met.
    let stdin = "{\"id\":\"op1\",\"baseline\":100.0,\"candidate\":110.0}\n\
                 {\"id\":\"op1\",\"baseline\":1000.0,\"candidate\":980.0}\n\
                 {\"id\":\"op2\",\"baseline\":100.0,\"candidate\":110.0}\n\
                 {\"id\":\"op2\",\"baseline\":1000.0,\"candidate\":980.0}\n";
    let output = veridict()
        .args([
            "compare",
            "-",
            "--metric",
            "relative-diff",
            "--paired-by-id",
        ])
        .write_stdin(stdin)
        .assert()
        .code(0)
        .get_output()
        .stdout
        .clone();
    let json: serde_json::Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(json["paired_count"], 2);
    assert!((json["effect"].as_f64().unwrap() - 0.04).abs() < 1e-9);
}

#[test]
fn relative_diff_all_bootstrap_methods_produce_valid_json() {
    let stdin: String = (0..20)
        .map(|i| {
            let b = 10.0 + i as f64;
            format!("{{\"baseline\":{b},\"candidate\":{}}}\n", b * 1.05)
        })
        .collect();
    for method in ["percentile", "basic", "bca"] {
        let output = veridict()
            .args([
                "compare",
                "-",
                "--metric",
                "relative-diff",
                "--bootstrap-method",
                method,
            ])
            .write_stdin(stdin.clone())
            .assert()
            .code(0)
            .get_output()
            .stdout
            .clone();
        let json: serde_json::Value = serde_json::from_slice(&output).unwrap();
        assert!(json["ci_low"].as_f64().unwrap().is_finite());
        assert!(json["ci_high"].as_f64().unwrap().is_finite());
    }
}

#[test]
fn relative_diff_same_seed_and_input_is_bit_identical() {
    let stdin: String = (0..20)
        .map(|i| {
            let b = 10.0 + i as f64;
            format!("{{\"baseline\":{b},\"candidate\":{}}}\n", b * 1.05 + 0.3)
        })
        .collect();
    let args = ["compare", "-", "--metric", "relative-diff", "--seed", "42"];
    let out_a = veridict()
        .args(args)
        .write_stdin(stdin.clone())
        .output()
        .unwrap();
    let out_b = veridict().args(args).write_stdin(stdin).output().unwrap();
    assert_eq!(out_a.stdout, out_b.stdout);
}

#[test]
fn relative_diff_multi_metric_with_mean_diff_runs_in_a_single_pass_with_correct_units() {
    let stdin =
        "{\"baseline\":100.0,\"candidate\":110.0}\n{\"baseline\":200.0,\"candidate\":220.0}\n";
    let output = veridict()
        .args([
            "compare",
            "-",
            "--metric",
            "mean-diff",
            "--metric",
            "relative-diff",
        ])
        .write_stdin(stdin)
        .assert()
        .code(0)
        .get_output()
        .stdout
        .clone();
    let json: serde_json::Value = serde_json::from_slice(&output).unwrap();
    let reports = json["reports"].as_array().unwrap();
    assert_eq!(reports.len(), 2);
    assert_eq!(reports[0]["metric"], "mean-diff");
    assert_eq!(reports[1]["metric"], "relative-diff");
    // mean-diff's effect is in raw units (mean of +10, +20 = 15); relative-diff's is a ratio (0.10).
    assert!((reports[0]["effect"].as_f64().unwrap() - 15.0).abs() < 1e-9);
    assert!((reports[1]["effect"].as_f64().unwrap() - 0.10).abs() < 1e-9);
}

/// Consistent +5% per pair: sign-test sees an unambiguous positive sign every time, relative-diff
/// sees a stable 0.05 ratio - both metrics agree loudly regardless of threshold, so these tests
/// isolate whether the *right number* reached each metric's own `pass_above`/`fail_below`, not
/// whether the run happens to pass or fail.
fn min_effect_multi_metric_stdin() -> String {
    (0..20)
        .map(|i| {
            let b = 10.0 + i as f64;
            format!("{{\"baseline\":{b},\"candidate\":{}}}\n", b * 1.05)
        })
        .collect()
}

#[test]
fn min_effect_per_metric_overrides_apply_independently() {
    let output = veridict()
        .args([
            "compare",
            "-",
            "--metric",
            "sign-test",
            "--metric",
            "relative-diff",
            "--min-effect",
            "sign-test=0.01,relative-diff=0.005",
        ])
        .write_stdin(min_effect_multi_metric_stdin())
        .output()
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let reports = json["reports"].as_array().unwrap();
    assert_eq!(reports[0]["metric"], "sign-test");
    assert_eq!(reports[0]["pass_above"], 0.01);
    assert_eq!(reports[0]["fail_below"], -0.01);
    assert_eq!(reports[1]["metric"], "relative-diff");
    assert_eq!(reports[1]["pass_above"], 0.005);
    assert_eq!(reports[1]["fail_below"], -0.005);
}

#[test]
fn min_effect_mixes_bare_default_and_metric_override() {
    let output = veridict()
        .args([
            "compare",
            "-",
            "--metric",
            "sign-test",
            "--metric",
            "relative-diff",
            "--min-effect",
            "0.01,relative-diff=0.005",
        ])
        .write_stdin(min_effect_multi_metric_stdin())
        .output()
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let reports = json["reports"].as_array().unwrap();
    assert_eq!(reports[0]["metric"], "sign-test");
    assert_eq!(
        reports[0]["pass_above"], 0.01,
        "sign-test falls back to the bare default"
    );
    assert_eq!(reports[1]["metric"], "relative-diff");
    assert_eq!(
        reports[1]["pass_above"], 0.005,
        "relative-diff keeps its own override"
    );
}

#[test]
fn min_effect_mixed_units_warns_on_bare_default() {
    veridict()
        .args([
            "compare",
            "-",
            "--metric",
            "sign-test",
            "--metric",
            "relative-diff",
            "--min-effect",
            "0.01",
        ])
        .write_stdin(min_effect_multi_metric_stdin())
        .assert()
        .stderr(
            predicate::str::contains("warning:")
                .and(predicate::str::contains("sign-test"))
                .and(predicate::str::contains("relative-diff"))
                .and(predicate::str::contains("not the same scale")),
        );
}

#[test]
fn min_effect_mixed_units_silent_with_full_per_metric_overrides() {
    veridict()
        .args([
            "compare",
            "-",
            "--metric",
            "sign-test",
            "--metric",
            "relative-diff",
            "--min-effect",
            "sign-test=0.01,relative-diff=0.01",
        ])
        .write_stdin(min_effect_multi_metric_stdin())
        .assert()
        .stderr(predicate::str::contains("warning:").not());
}

#[test]
fn min_effect_mixed_units_silent_for_a_single_metric() {
    veridict()
        .args([
            "compare",
            "-",
            "--metric",
            "relative-diff",
            "--min-effect",
            "0.01",
        ])
        .write_stdin(min_effect_multi_metric_stdin())
        .assert()
        .stderr(predicate::str::contains("warning:").not());
}

#[test]
fn min_effect_mixed_units_silent_when_same_unit_family() {
    // sign-test and winrate share a unit family (win-rate margin off 0.5) - a shared bare default
    // between them is genuinely comparable, so this must stay silent.
    let stdin = (0..20)
        .map(|_| "{\"result\":\"candidate_win\"}\n")
        .collect::<String>();
    veridict()
        .args([
            "compare",
            "-",
            "--metric",
            "sign-test",
            "--metric",
            "winrate",
            "--min-effect",
            "0.01",
        ])
        .write_stdin(stdin)
        .assert()
        .stderr(predicate::str::contains("warning:").not());
}

#[test]
fn min_effect_override_for_unrequested_metric_exits_three() {
    veridict()
        .args([
            "compare",
            "-",
            "--metric",
            "sign-test",
            "--min-effect",
            "relative-diff=0.1",
        ])
        .write_stdin(min_effect_multi_metric_stdin())
        .assert()
        .code(3)
        .stderr(predicate::str::contains("--min-effect"));
}

#[test]
fn min_effect_duplicate_bare_default_exits_three() {
    veridict()
        .args([
            "compare",
            "-",
            "--metric",
            "sign-test",
            "--min-effect",
            "0.01,0.02",
        ])
        .write_stdin(min_effect_multi_metric_stdin())
        .assert()
        .code(3)
        .stderr(predicate::str::contains("--min-effect"));
}

#[test]
fn min_effect_duplicate_metric_override_exits_three() {
    veridict()
        .args([
            "compare",
            "-",
            "--metric",
            "sign-test",
            "--metric",
            "relative-diff",
            "--min-effect",
            "sign-test=0.01,sign-test=0.02",
        ])
        .write_stdin(min_effect_multi_metric_stdin())
        .assert()
        .code(3)
        .stderr(predicate::str::contains("more than once"));
}

#[test]
fn min_effect_unknown_metric_name_exits_three() {
    veridict()
        .args([
            "compare",
            "-",
            "--metric",
            "sign-test",
            "--min-effect",
            "foo=0.1",
        ])
        .write_stdin(min_effect_multi_metric_stdin())
        .assert()
        .code(3)
        .stderr(predicate::str::contains("invalid value"));
}

#[test]
fn clap_usage_errors_do_not_collide_with_inconclusive_exit_code() {
    veridict()
        .args(["compare", "-"])
        .assert()
        .code(3)
        .stderr(predicate::str::contains("--metric"));
}

#[test]
fn clap_help_and_version_still_exit_successfully() {
    veridict()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("Usage:"));

    veridict()
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::contains(env!("CARGO_PKG_VERSION")));
}

#[test]
fn min_effect_override_violating_threshold_order_exits_three_and_names_metric() {
    veridict()
        .args([
            "compare",
            "-",
            "--metric",
            "sign-test",
            "--min-effect",
            "sign-test=-0.5",
        ])
        .write_stdin(min_effect_multi_metric_stdin())
        .assert()
        .code(3)
        .stderr(predicate::str::contains("sign-test"));
}

#[test]
fn claim_correction_rejects_a_relative_diff_family() {
    let stdin = "{\"id\":\"a\",\"baseline\":100.0,\"candidate\":110.0}\n{\"id\":\"b\",\"baseline\":200.0,\"candidate\":220.0}\n";
    veridict()
        .args([
            "compare",
            "-",
            "--metric",
            "relative-diff",
            "--claim-correction",
            "bonferroni",
        ])
        .write_stdin(stdin)
        .assert()
        .code(3)
        .stderr(predicate::str::contains("relative-diff"));
}

#[test]
fn power_relative_diff_is_a_clear_unsupported_error_not_a_clap_usage_error() {
    veridict()
        .args([
            "power",
            "--metric",
            "relative-diff",
            "--min-effect",
            "0.02",
            "--assume-effect",
            "0.10",
        ])
        .assert()
        .code(3)
        .stderr(predicate::str::contains("relative-diff"));
}

#[test]
fn cluster_by_id_rejected_for_relative_diff() {
    let stdin = "{\"id\":\"a\",\"baseline\":10.0,\"candidate\":11.0}\n";
    veridict()
        .args([
            "compare",
            "-",
            "--metric",
            "relative-diff",
            "--cluster-by-id",
        ])
        .write_stdin(stdin)
        .assert()
        .code(3)
        .stderr(predicate::str::contains("relative-diff"));
}

// --- wide_baseline_scale scale-mismatch diagnostic ---

#[test]
fn wide_baseline_scale_warning_fires_above_ten_x_spread_and_mentions_relative_diff() {
    let stdin = "{\"baseline\":10.0,\"candidate\":10.5}\n\
                 {\"baseline\":40.0,\"candidate\":42.0}\n\
                 {\"baseline\":70.0,\"candidate\":73.0}\n\
                 {\"baseline\":100.0,\"candidate\":105.0}\n\
                 {\"baseline\":150.0,\"candidate\":157.0}\n";
    let output = veridict()
        .args(["compare", "-", "--metric", "mean-diff"])
        .write_stdin(stdin)
        .assert()
        .code(0)
        .get_output()
        .stdout
        .clone();
    let json: serde_json::Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(json["data_quality"]["wide_baseline_scale"], true);
    assert!(json["scale_diagnostics"].is_object());
    let warnings = json["warnings"].as_array().unwrap();
    assert!(
        warnings
            .iter()
            .any(|w| w.as_str().unwrap().contains("--metric relative-diff"))
    );
}

#[test]
fn wide_baseline_scale_silent_below_ten_x_spread() {
    let stdin = "{\"baseline\":10.0,\"candidate\":10.5}\n\
                 {\"baseline\":20.0,\"candidate\":21.0}\n\
                 {\"baseline\":30.0,\"candidate\":31.5}\n\
                 {\"baseline\":50.0,\"candidate\":52.0}\n";
    let output = veridict()
        .args(["compare", "-", "--metric", "mean-diff"])
        .write_stdin(stdin)
        .assert()
        .code(0)
        .get_output()
        .stdout
        .clone();
    let json: serde_json::Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(json["data_quality"]["wide_baseline_scale"], false);
}
