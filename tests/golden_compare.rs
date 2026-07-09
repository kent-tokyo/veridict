//! Golden JSON report tests for `compare --correction`: one fixture proving `--correction
//! none`/omitted stays byte-shape-identical to before this feature existed, one proving
//! `--correction holm`'s new fields/verdict shape.
//!
//! Structural + float-tolerant, not byte-exact, same rationale and helper as
//! `tests/golden_matrix.rs`: an added/removed/renamed field (object key sets must match exactly
//! at every level) and any array-length change are the actual schema-drift risks this catches.
//!
//! To regenerate a fixture after an intentional output change: `UPDATE_GOLDEN=1 cargo test --test
//! golden_compare`, then `git diff tests/fixtures/*.golden.json` and review the diff like any
//! other change.

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

/// Runs `veridict compare <args>`, compares its stdout JSON against the checked-in fixture at
/// `tests/fixtures/<name>.golden.json` (structural + float-tolerant per
/// `assert_json_matches_golden`), and supports regenerating that fixture via `UPDATE_GOLDEN=1`.
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
        serde_json::from_slice(&output.stdout).expect("compare stdout must be valid JSON");

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
            "missing golden fixture {fixture_path}; run `UPDATE_GOLDEN=1 cargo test --test golden_compare` to create it, then review the diff before committing"
        )
    }))
    .unwrap();

    assert_json_matches_golden(&actual, &expected, 1e-9, "$");
}

#[test]
fn correction_none_matches_the_pre_existing_multi_report_shape() {
    // No --correction flag at all: proves the multi-report's JSON shape is unaffected by this
    // feature existing, byte-shape-identical to before it shipped.
    check_golden(
        "compare_correction_none",
        &[
            "compare",
            "tests/fixtures/compare_multi_metric.jsonl",
            "--metric",
            "winrate",
            "--metric",
            "elo",
            "--min-effect",
            "0.02",
        ],
        0,
    );
}

#[test]
fn correction_holm_matches_the_corrected_multi_report_shape() {
    check_golden(
        "compare_correction_holm",
        &[
            "compare",
            "tests/fixtures/compare_multi_metric.jsonl",
            "--metric",
            "winrate",
            "--metric",
            "elo",
            "--min-effect",
            "0.02",
            "--correction",
            "holm",
        ],
        0,
    );
}
