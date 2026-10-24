use super::*;

// Default 95% confidence for --min-effect 0.02 (ci_low ~= 0.0221, just past the 0.02 pass bar) is
// weak enough that a family of two metrics downgrades it under Bonferroni, but not under Holm.
// The values were checked against a real run before these tests were written.
fn marginal_winrate_stdin() -> String {
    let mut stdin = String::new();
    for _ in 0..62 {
        stdin.push_str("{\"result\":\"candidate_win\"}\n");
    }
    for _ in 0..38 {
        stdin.push_str("{\"result\":\"baseline_win\"}\n");
    }
    stdin
}

#[test]
fn correction_none_is_the_default_and_produces_no_extra_json_keys() {
    // The concrete proof of "no behavior change when disabled": an exact key-set check, not just
    // "still passes" - correction_method/family_size/achieved_alpha/adjusted_alpha_threshold/
    // unadjusted_verdict/family_adjusted_verdict/family_adjusted_promotion must be entirely
    // absent, not present as null.
    let output = veridict()
        .args([
            "compare",
            "-",
            "--metric",
            "winrate",
            "--min-effect",
            "0.02",
        ])
        .write_stdin(marginal_winrate_stdin())
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
    for field in [
        "correction_method",
        "family_size",
        "achieved_alpha",
        "adjusted_alpha_threshold",
        "unadjusted_verdict",
        "family_adjusted_verdict",
        "family_adjusted_promotion",
    ] {
        assert!(!keys.contains(&field), "unexpected key {field} in {keys:?}");
    }
}

#[test]
fn correction_explicit_none_matches_the_default() {
    let stdin = marginal_winrate_stdin();
    let default_output = veridict()
        .args([
            "compare",
            "-",
            "--metric",
            "winrate",
            "--min-effect",
            "0.02",
        ])
        .write_stdin(stdin.clone())
        .assert()
        .get_output()
        .stdout
        .clone();
    let explicit_none_output = veridict()
        .args([
            "compare",
            "-",
            "--metric",
            "winrate",
            "--min-effect",
            "0.02",
            "--claim-correction",
            "none",
        ])
        .write_stdin(stdin)
        .assert()
        .get_output()
        .stdout
        .clone();
    assert_eq!(default_output, explicit_none_output);
}

#[test]
fn claim_correction_bonferroni_downgrades_a_marginal_claim_but_never_the_overall_verdict() {
    // The deployment-gate verdict/promotion are an intersection-union rule over every metric -
    // already at least as conservative as a single metric, so --claim-correction never touches
    // them: exit code 0/"pass" here, even though winrate's own family_adjusted_verdict is
    // downgraded.
    veridict()
        .args([
            "compare",
            "-",
            "--metric",
            "winrate",
            "--metric",
            "elo",
            "--min-effect",
            "0.02",
            "--claim-correction",
            "bonferroni",
        ])
        .write_stdin(marginal_winrate_stdin())
        .assert()
        .code(0)
        .stdout(predicate::str::contains("\"verdict\": \"pass\""))
        .stdout(predicate::str::contains(
            "\"correction_method\": \"bonferroni\"",
        ))
        .stdout(predicate::str::contains("\"family_size\": 2"))
        .stdout(predicate::str::contains("\"unadjusted_verdict\": \"pass\""))
        .stdout(predicate::str::contains(
            "\"family_adjusted_verdict\": \"inconclusive\"",
        ))
        .stdout(predicate::str::contains(
            "\"simultaneous_claims_promotion\": \"not_promoted\"",
        ));
}

#[test]
fn claim_correction_holm_is_uniformly_more_powerful_than_bonferroni_on_the_same_family() {
    // Same input, same family - Holm's family_adjusted_verdict stays "pass" for both metrics
    // where Bonferroni (above) downgrades winrate's, the concrete behavioral proof of Holm's
    // higher power. The overall verdict was already "pass" either way (see the test above).
    let output = veridict()
        .args([
            "compare",
            "-",
            "--metric",
            "winrate",
            "--metric",
            "elo",
            "--min-effect",
            "0.02",
            "--claim-correction",
            "holm",
        ])
        .write_stdin(marginal_winrate_stdin())
        .assert()
        .code(0)
        .stdout(predicate::str::contains("\"verdict\": \"pass\""))
        .stdout(predicate::str::contains("\"correction_method\": \"holm\""))
        .stdout(predicate::str::contains(
            "\"simultaneous_claims_promotion\": \"promoted\"",
        ))
        .get_output()
        .stdout
        .clone();
    let json: serde_json::Value = serde_json::from_slice(&output).unwrap();
    for report in json["reports"].as_array().unwrap() {
        assert_eq!(report["family_adjusted_verdict"], "pass");
    }
}

#[test]
fn claim_correction_on_a_single_metric_run_is_a_no_op() {
    // family_size=1 degenerates both corrections to the report's own existing pass condition.
    veridict()
        .args([
            "compare",
            "-",
            "--metric",
            "winrate",
            "--min-effect",
            "0.02",
            "--claim-correction",
            "holm",
        ])
        .write_stdin(marginal_winrate_stdin())
        .assert()
        .code(0)
        .stdout(predicate::str::contains("\"verdict\": \"pass\""))
        .stdout(predicate::str::contains("\"family_size\": 1"))
        .stdout(predicate::str::contains(
            "\"family_adjusted_verdict\": \"pass\"",
        ));
}

#[test]
fn claim_correction_never_changes_the_overall_verdict_or_promotion() {
    // Same marginal family as the bonferroni test above, checked field-by-field: the deployment
    // gate (verdict/promotion) is untouched even though the family-adjusted claim is downgraded.
    let output = veridict()
        .args([
            "compare",
            "-",
            "--metric",
            "winrate",
            "--metric",
            "elo",
            "--min-effect",
            "0.02",
            "--claim-correction",
            "bonferroni",
        ])
        .write_stdin(marginal_winrate_stdin())
        .assert()
        .code(0)
        .get_output()
        .stdout
        .clone();
    let json: serde_json::Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(json["verdict"], "pass");
    assert_eq!(json["promotion"], "promoted");
    assert_eq!(json["simultaneous_claims_promotion"], "not_promoted");
    let reports = json["reports"].as_array().unwrap();
    assert_eq!(reports[0]["metric"], "winrate");
    assert_eq!(reports[0]["verdict"], "pass");
    assert_eq!(reports[0]["family_adjusted_verdict"], "inconclusive");
    assert_eq!(reports[1]["metric"], "elo");
    assert_eq!(reports[1]["verdict"], "pass");
    assert_eq!(reports[1]["family_adjusted_verdict"], "pass");
}

#[test]
fn correction_rejects_an_unknown_method() {
    veridict()
        .args([
            "compare",
            "-",
            "--metric",
            "winrate",
            "--claim-correction",
            "sidak",
        ])
        .write_stdin(marginal_winrate_stdin())
        .assert()
        .code(3)
        .stderr(predicate::str::contains("invalid value"));
}

#[test]
fn claim_correction_rejects_a_mean_diff_family() {
    let stdin = "{\"id\":\"a\",\"baseline\":1.0,\"candidate\":1.1}\n{\"id\":\"b\",\"baseline\":1.0,\"candidate\":1.2}\n";
    veridict()
        .args([
            "compare",
            "-",
            "--metric",
            "mean-diff",
            "--claim-correction",
            "bonferroni",
        ])
        .write_stdin(stdin)
        .assert()
        .code(3)
        .stderr(predicate::str::contains("mean-diff"));
}

#[test]
fn claim_correction_rejects_a_quantile_diff_family() {
    let stdin: String = (0..30)
        .map(|i| format!("{{\"id\":\"a{i}\",\"baseline\":1.0,\"candidate\":1.1}}\n"))
        .collect();
    veridict()
        .args([
            "compare",
            "-",
            "--metric",
            "quantile-diff",
            "--quantile",
            "0.5",
            "--claim-correction",
            "holm",
        ])
        .write_stdin(stdin)
        .assert()
        .code(3)
        .stderr(predicate::str::contains("quantile-diff"));
}

#[test]
fn deprecated_correction_alias_behaves_identically_to_claim_correction() {
    let stdin = marginal_winrate_stdin();
    let via_claim_correction = veridict()
        .args([
            "compare",
            "-",
            "--metric",
            "winrate",
            "--metric",
            "elo",
            "--min-effect",
            "0.02",
            "--claim-correction",
            "bonferroni",
        ])
        .write_stdin(stdin.clone())
        .assert()
        .code(0)
        .get_output()
        .stdout
        .clone();
    let via_deprecated_alias = veridict()
        .args([
            "compare",
            "-",
            "--metric",
            "winrate",
            "--metric",
            "elo",
            "--min-effect",
            "0.02",
            "--correction",
            "bonferroni",
        ])
        .write_stdin(stdin)
        .assert()
        .code(0)
        .stderr(predicate::str::contains("--correction is deprecated"))
        .get_output()
        .stdout
        .clone();
    assert_eq!(via_claim_correction, via_deprecated_alias);
}

#[test]
fn claim_correction_and_deprecated_correction_together_is_a_config_error() {
    veridict()
        .args([
            "compare",
            "-",
            "--metric",
            "winrate",
            "--claim-correction",
            "holm",
            "--correction",
            "holm",
        ])
        .write_stdin(marginal_winrate_stdin())
        .assert()
        .code(3)
        .stderr(predicate::str::contains("cannot be used with"));
}

#[test]
fn failure_cap_violation_forces_not_promoted_on_both_raw_and_family_adjusted_promotion() {
    // A crash cap breach must invalidate the report before claim-correction ever looks at it - an
    // invalid report must never read as a legitimate simultaneous claim, no matter how strong its
    // raw numeric verdict looks. winrate's candidate side crashes 3 times (breaching
    // --max-crashes 0) while every other trial is a clean candidate win; elo has no crashes at
    // all and should pass its own claim cleanly.
    let stdin: String = (0..3)
        .map(|_| "{\"candidate_status\":\"crash\"}\n".to_string())
        .chain((0..20).map(|_| "{\"result\":\"candidate_win\"}\n".to_string()))
        .collect();
    let output = veridict()
        .args([
            "compare",
            "-",
            "--metric",
            "winrate",
            "--metric",
            "elo",
            "--min-effect",
            "0.1",
            "--max-crashes",
            "0",
            "--claim-correction",
            "holm",
        ])
        .write_stdin(stdin)
        .assert()
        .code(2)
        .get_output()
        .stdout
        .clone();
    let json: serde_json::Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(json["promotion"], "not_promoted");
    assert_eq!(json["simultaneous_claims_promotion"], "not_promoted");
    let winrate = &json["reports"][0];
    assert_eq!(winrate["validity"], "invalid");
    assert_eq!(winrate["promotion"], "not_promoted");
    assert_eq!(winrate["family_adjusted_promotion"], "not_promoted");
}

// --- --max-timeouts/--max-crashes/--max-invalid (validity/promotion) ---

#[test]
fn uncapped_compare_run_reports_valid_and_promoted() {
    let stdin = (0..20)
        .map(|_| "{\"result\":\"candidate_win\"}\n")
        .collect::<String>();
    veridict()
        .args(["compare", "-", "--metric", "winrate", "--min-effect", "0.1"])
        .write_stdin(stdin)
        .assert()
        .code(0)
        .stdout(predicate::str::contains("\"validity\": \"valid\""))
        .stdout(predicate::str::contains("\"promotion\": \"promoted\""));
}

#[test]
fn breached_crash_cap_forces_inconclusive_and_not_promoted_even_under_failure_policy_loss() {
    // Under --failure-policy loss a crash would normally just synthesize a baseline_win - here
    // every candidate trial crashes, which would numerically net to a clean winrate Fail. The
    // crash cap must catch this before that misleading Fail ever reaches the report.
    let stdin = (0..20)
        .map(|_| "{\"candidate_status\":\"crash\"}\n")
        .collect::<String>();
    veridict()
        .args([
            "compare",
            "-",
            "--metric",
            "winrate",
            "--min-effect",
            "0.1",
            "--failure-policy",
            "loss",
            "--max-crashes",
            "0",
        ])
        .write_stdin(stdin)
        .assert()
        .code(2)
        .stdout(predicate::str::contains("\"verdict\": \"inconclusive\""))
        .stdout(predicate::str::contains("\"validity\": \"invalid\""))
        .stdout(predicate::str::contains("\"promotion\": \"not_promoted\""))
        .stdout(predicate::str::contains("\"reason\": \"INVALID:"));
}

#[test]
fn max_timeouts_cap_is_respected_at_exactly_the_limit() {
    let mut stdin = (0..20)
        .map(|_| "{\"result\":\"candidate_win\"}\n".to_string())
        .collect::<String>();
    stdin.push_str("{\"candidate_status\":\"timeout\"}\n");
    veridict()
        .args([
            "compare",
            "-",
            "--metric",
            "winrate",
            "--min-effect",
            "0.1",
            "--max-timeouts",
            "1",
        ])
        .write_stdin(stdin)
        .assert()
        .code(0)
        .stdout(predicate::str::contains("\"validity\": \"valid\""))
        .stdout(predicate::str::contains("\"promotion\": \"promoted\""));
}

#[test]
fn max_invalid_cap_breached_in_a_multi_metric_run_invalidates_the_whole_report() {
    let mut stdin = (0..20)
        .map(|i| {
            format!(
                "{{\"result\":\"candidate_win\",\"baseline\":{i}.0,\"candidate\":{}.5}}\n",
                i
            )
        })
        .collect::<String>();
    stdin.push_str("{\"candidate_status\":\"invalid\"}\n");
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
            "--max-invalid",
            "0",
        ])
        .write_stdin(stdin)
        .assert()
        .code(2)
        .stdout(predicate::str::contains("\"verdict\": \"inconclusive\""))
        .stdout(predicate::str::contains("\"validity\": \"invalid\""))
        .stdout(predicate::str::contains("\"promotion\": \"not_promoted\""));
}

#[test]
fn sprt_max_crashes_cap_forces_inconclusive() {
    let mut stdin = (0..40)
        .map(|_| "{\"result\":\"candidate_win\"}\n".to_string())
        .collect::<String>();
    stdin.push_str("{\"candidate_status\":\"crash\"}\n");
    veridict()
        .args([
            "sprt",
            "-",
            "--elo0",
            "0",
            "--elo1",
            "10",
            "--max-crashes",
            "0",
        ])
        .write_stdin(stdin)
        .assert()
        .code(2)
        .stdout(predicate::str::contains("\"verdict\": \"inconclusive\""))
        .stdout(predicate::str::contains("\"validity\": \"invalid\""))
        .stdout(predicate::str::contains("\"promotion\": \"not_promoted\""));
}
