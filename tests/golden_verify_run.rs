//! Golden JSON report tests for `verify-run`: one fixture with no structural defects (validity
//! valid, empty violations/checks_skipped), one exercising every violation-producing check plus
//! several skipped checks - together covering every field's populated shape.
//!
//! Structural + float-tolerant, not byte-exact, same rationale and helper as
//! `tests/golden_compare.rs`.
//!
//! To regenerate a fixture after an intentional output change: `UPDATE_GOLDEN=1 cargo test --test
//! golden_verify_run`, then `git diff tests/fixtures/*.golden.json` and review the diff like any
//! other change.
//!
//! Violation order is asserted to be process-order-independent here, not just structurally
//! matched: `verify_run::run` sorts its output specifically so this golden comparison (which is
//! positional for arrays) doesn't flake between runs.

use assert_cmd::Command;

fn veridict() -> Command {
    Command::cargo_bin("veridict").unwrap()
}

fn assert_json_matches_golden(
    actual: &serde_json::Value,
    expected: &serde_json::Value,
    tol: f64,
    path: &str,
) {
    match (actual, expected) {
        (serde_json::Value::Number(a), serde_json::Value::Number(e)) => {
            let (a, e) = (a.as_f64().unwrap(), e.as_f64().unwrap());
            assert!((a - e).abs() < tol, "{path}: expected {e}, got {a}");
        }
        (serde_json::Value::Object(a), serde_json::Value::Object(e)) => {
            let (mut ak, mut ek): (Vec<_>, Vec<_>) = (a.keys().collect(), e.keys().collect());
            ak.sort();
            ek.sort();
            assert_eq!(ak, ek, "{path}: field set drifted");
            for k in a.keys() {
                assert_json_matches_golden(&a[k], &e[k], tol, &format!("{path}.{k}"));
            }
        }
        (serde_json::Value::Array(a), serde_json::Value::Array(e)) => {
            assert_eq!(a.len(), e.len(), "{path}: array length drifted");
            for (i, (av, ev)) in a.iter().zip(e).enumerate() {
                assert_json_matches_golden(av, ev, tol, &format!("{path}[{i}]"));
            }
        }
        (a, e) => assert_eq!(a, e, "{path}"),
    }
}

/// Runs `veridict verify-run <manifest> <games>`, compares its stdout JSON against the
/// checked-in fixture at `tests/fixtures/<name>.golden.json`, and supports regenerating that
/// fixture via `UPDATE_GOLDEN=1`.
fn check_golden(name: &str, manifest: &str, games: &str, expected_code: i32) {
    let fixture_path = format!("tests/fixtures/{name}.golden.json");
    let output = veridict()
        .args(["verify-run", manifest, games])
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(expected_code),
        "unexpected exit code, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let actual: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("verify-run stdout must be valid JSON");

    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::write(
            &fixture_path,
            serde_json::to_string_pretty(&actual).unwrap() + "\n",
        )
        .unwrap();
        return;
    }

    let expected: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&fixture_path).unwrap_or_else(|_| {
        panic!(
            "missing golden fixture {fixture_path}; run `UPDATE_GOLDEN=1 cargo test --test golden_verify_run` to create it, then review the diff before committing"
        )
    }))
    .unwrap();

    assert_json_matches_golden(&actual, &expected, 1e-9, "$");
}

#[test]
fn clean_run_matches_golden_fixture() {
    check_golden(
        "verify_run_clean",
        "tests/fixtures/verify_run_clean.toml",
        "tests/fixtures/verify_run_clean.jsonl",
        0,
    );
}

#[test]
fn run_with_violations_matches_golden_fixture() {
    check_golden(
        "verify_run_with_violations",
        "tests/fixtures/verify_run_with_violations.toml",
        "tests/fixtures/verify_run_with_violations.jsonl",
        1,
    );
}
