use super::*;

#[test]
fn verify_run_clean_fixture_exits_zero() {
    veridict()
        .args([
            "verify-run",
            "tests/fixtures/verify_run_clean.toml",
            "tests/fixtures/verify_run_clean.jsonl",
        ])
        .assert()
        .code(0)
        .stdout(predicate::str::contains("\"validity\": \"valid\""))
        .stdout(predicate::str::contains("\"violations\": []"));
}

#[test]
fn verify_run_with_violations_fixture_exits_one() {
    veridict()
        .args([
            "verify-run",
            "tests/fixtures/verify_run_with_violations.toml",
            "tests/fixtures/verify_run_with_violations.jsonl",
        ])
        .assert()
        .code(1)
        .stdout(predicate::str::contains("\"validity\": \"invalid\""))
        .stdout(predicate::str::contains("\"check\": \"pair_completeness\""))
        .stdout(predicate::str::contains(
            "\"check\": \"experiment_contamination\"",
        ));
}

#[test]
fn verify_run_games_via_stdin() {
    let games = std::fs::read_to_string("tests/fixtures/verify_run_clean.jsonl").unwrap();
    veridict()
        .args(["verify-run", "tests/fixtures/verify_run_clean.toml", "-"])
        .write_stdin(games)
        .assert()
        .code(0)
        .stdout(predicate::str::contains("\"validity\": \"valid\""));
}

#[test]
fn verify_run_malformed_manifest_exits_three() {
    veridict()
        .args([
            "verify-run",
            "tests/fixtures/verify_run_malformed.toml",
            "tests/fixtures/verify_run_clean.jsonl",
        ])
        .assert()
        .code(3)
        .stderr(predicate::str::contains("invalid TOML"));
}

#[test]
fn verify_run_unsupported_manifest_schema_version_exits_three() {
    veridict()
        .args([
            "verify-run",
            "tests/fixtures/verify_run_unsupported_version.toml",
            "tests/fixtures/verify_run_clean.jsonl",
        ])
        .assert()
        .code(3)
        .stderr(predicate::str::contains("manifest_schema_version 999"));
}

#[test]
fn verify_run_degenerate_manifest_exits_three() {
    veridict()
        .args([
            "verify-run",
            "tests/fixtures/verify_run_degenerate.toml",
            "tests/fixtures/verify_run_clean.jsonl",
        ])
        .assert()
        .code(3)
        .stderr(predicate::str::contains("declares nothing"));
}

#[test]
fn verify_run_malformed_games_file_exits_three() {
    veridict()
        .args([
            "verify-run",
            "tests/fixtures/verify_run_clean.toml",
            "tests/fixtures/verify_run_malformed.jsonl",
        ])
        .assert()
        .code(3)
        .stderr(predicate::str::contains("invalid JSON"));
}

#[test]
fn verify_run_empty_games_input_exits_three() {
    veridict()
        .args(["verify-run", "tests/fixtures/verify_run_clean.toml", "-"])
        .write_stdin("")
        .assert()
        .code(3)
        .stderr(predicate::str::contains("no records"));
}
