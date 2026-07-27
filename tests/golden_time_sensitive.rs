//! Golden JSON report tests for `time-sensitive`: one fixture reaching a `pass` (hard-deadline
//! reward, `gro` policy, a short deterministic run of candidate wins), one staying `inconclusive`
//! at the reward schedule's horizon (exponential-decay reward, `edo` policy, a net-neutral
//! alternating win/loss sequence).
//!
//! Structural + float-tolerant, not byte-exact, same rationale and helper as
//! `tests/golden_verify_run.rs`.
//!
//! To regenerate a fixture after an intentional output change: `UPDATE_GOLDEN=1 cargo test --test
//! golden_time_sensitive`, then `git diff tests/fixtures/*.golden.json` and review the diff like
//! any other change.

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

/// Runs `veridict time-sensitive <args>`, compares its stdout JSON against the checked-in
/// fixture at `tests/fixtures/<name>.golden.json`, and supports regenerating that fixture via
/// `UPDATE_GOLDEN=1`.
fn check_golden(name: &str, args: &[&str], expected_code: i32) {
    let fixture_path = format!("tests/fixtures/{name}.golden.json");
    let output = veridict().args(args).output().unwrap();
    assert_eq!(
        output.status.code(),
        Some(expected_code),
        "unexpected exit code, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let actual: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("time-sensitive stdout must be valid JSON");

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
            "missing golden fixture {fixture_path}; run `UPDATE_GOLDEN=1 cargo test --test golden_time_sensitive` to create it, then review the diff before committing"
        )
    }))
    .unwrap();

    assert_json_matches_golden(&actual, &expected, 1e-9, "$");
}

#[test]
fn hard_deadline_gro_pass_matches_golden_fixture() {
    check_golden(
        "time_sensitive_hard_deadline_pass",
        &[
            "time-sensitive",
            "tests/fixtures/time_sensitive_hard_deadline_pass.jsonl",
            "--p0",
            "0.5",
            "--p1",
            "0.9",
            "--alpha",
            "0.2",
            "--policy",
            "gro",
            "--reward",
            "hard-deadline",
            "--deadline",
            "10",
        ],
        0,
    );
}

#[test]
fn exponential_edo_inconclusive_matches_golden_fixture() {
    check_golden(
        "time_sensitive_exponential_inconclusive",
        &[
            "time-sensitive",
            "tests/fixtures/time_sensitive_exponential_inconclusive.jsonl",
            "--p0",
            "0.5",
            "--p1",
            "0.55",
            "--alpha",
            "0.05",
            "--policy",
            "edo",
            "--reward",
            "exponential",
            "--time-scale",
            "200",
            "--horizon",
            "10",
        ],
        2,
    );
}
