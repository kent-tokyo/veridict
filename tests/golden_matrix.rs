//! Golden JSON report tests for `matrix`, the schema actually changing this
//! sprint (general-graph Bradley-Terry). Structural + float-tolerant, not
//! byte-exact: `log10` (behind Elo) isn't guaranteed correctly-rounded
//! across libm implementations, so a fixture generated on one platform
//! could differ from CI's `ubuntu-latest` in the last ULP even with
//! identical logic. What this DOES catch, exactly: an added, removed, or
//! renamed field (object key sets must match exactly at every level) and
//! any array-length change - the actual schema-drift risk this refactor
//! introduces.
//!
//! To regenerate a fixture after an intentional output change:
//! `UPDATE_GOLDEN=1 cargo test --test golden_matrix`, then `git diff
//! tests/fixtures/*.golden.json` and review the diff like any other change.

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

/// Runs `veridict matrix <args>`, compares its stdout JSON against the
/// checked-in fixture at `tests/fixtures/<name>.golden.json` (structural +
/// float-tolerant per `assert_json_matches_golden`), and supports
/// regenerating that fixture via `UPDATE_GOLDEN=1`.
fn check_golden(name: &str, args: &[&str]) {
    let fixture_path = format!("tests/fixtures/{name}.golden.json");
    let output = veridict().args(args).output().unwrap();
    assert!(
        output.status.success() || output.status.code() == Some(0),
        "veridict matrix exited non-zero: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let actual: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("matrix stdout must be valid JSON");

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
            "missing golden fixture {fixture_path}; run `UPDATE_GOLDEN=1 cargo test --test golden_matrix` to create it, then review the diff before committing"
        )
    }))
    .unwrap();

    assert_json_matches_golden(&actual, &expected, 1e-9, "$");
}

#[test]
fn legacy_only_matrix_matches_golden_fixture() {
    // Proves the star-graph path's output shape/values are unaffected by
    // this sprint's general-graph work - no `--matches` involved at all.
    check_golden(
        "matrix_legacy_only",
        &[
            "matrix",
            "tests/fixtures/matrix_legacy_a.jsonl",
            "tests/fixtures/matrix_legacy_b.jsonl",
        ],
    );
}

#[test]
fn mixed_graph_matrix_matches_golden_fixture() {
    // Exercises the new solver, a real `direct` candidate-vs-candidate
    // edge, an `inferred` cell, and a genuinely `disconnected` pair, all in
    // one fixture.
    check_golden(
        "matrix_mixed_graph",
        &[
            "matrix",
            "tests/fixtures/matrix_legacy_a.jsonl",
            "tests/fixtures/matrix_legacy_b.jsonl",
            "--matches",
            "tests/fixtures/matrix_matches.jsonl",
        ],
    );
}
