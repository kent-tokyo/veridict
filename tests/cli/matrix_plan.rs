use super::*;

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
fn matrix_general_graph_rejects_invalid_bootstrap_configuration() {
    let dir = std::env::temp_dir();
    let matches_path = dir.join("veridict_cli_test_matrix_invalid_bootstrap.jsonl");
    std::fs::write(
        &matches_path,
        concat!(
            "{\"id\":\"ab\",\"a\":\"a\",\"b\":\"b\",\"result\":\"draw\"}\n",
            "{\"id\":\"bc\",\"a\":\"b\",\"b\":\"c\",\"result\":\"draw\"}\n",
            "{\"id\":\"ca\",\"a\":\"c\",\"b\":\"a\",\"result\":\"draw\"}\n",
        ),
    )
    .unwrap();

    veridict()
        .args([
            "matrix",
            "--matches",
            matches_path.to_str().unwrap(),
            "--resamples",
            "0",
        ])
        .assert()
        .code(3)
        .stderr(predicate::str::contains("resample count"));
    veridict()
        .args([
            "matrix",
            "--matches",
            matches_path.to_str().unwrap(),
            "--confidence",
            "NaN",
        ])
        .assert()
        .code(3)
        .stderr(predicate::str::contains("confidence"));

    std::fs::remove_file(&matches_path).ok();
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
