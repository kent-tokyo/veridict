//! Golden JSON report tests for `plan`. Structural + float-tolerant, not byte-exact - same
//! rationale and helper as `tests/golden_matrix.rs` (see its doc comment); duplicated rather than
//! shared, since `tests/*.rs` files compile as independent binaries with no shared-helper module
//! in this project.
//!
//! To regenerate a fixture after an intentional output change:
//! `UPDATE_GOLDEN=1 cargo test --test golden_plan`, then `git diff tests/fixtures/*.golden.json`
//! and review the diff like any other change.

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

fn check_golden(name: &str, args: &[&str]) {
    let fixture_path = format!("tests/fixtures/{name}.golden.json");
    let output = veridict().args(args).output().unwrap();
    assert!(
        output.status.success() || output.status.code() == Some(0),
        "veridict plan exited non-zero: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let actual: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("plan stdout must be valid JSON");

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
            "missing golden fixture {fixture_path}; run `UPDATE_GOLDEN=1 cargo test --test golden_plan` to create it, then review the diff before committing"
        )
    }))
    .unwrap();

    assert_json_matches_golden(&actual, &expected, 1e-9, "$");
}

#[test]
fn star_graph_plan_matches_golden_fixture() {
    // Reuses matrix's own star-graph fixtures with a generous --min-elo (400): both
    // baseline-vs-candidate cells are already narrower than that (exact Wilson branch,
    // estimated_additional_trials: 0), and the inferred candidate-vs-candidate cell's wider CI
    // still rounds down to 0 more trials needed (CLT branch, ratio just above 1.0) - exercises
    // both trial-estimate branches in one fixture without a manufactured edge case.
    check_golden(
        "plan_star_graph",
        &[
            "plan",
            "tests/fixtures/matrix_legacy_a.jsonl",
            "tests/fixtures/matrix_legacy_b.jsonl",
            "--min-elo",
            "400",
        ],
    );
}

#[test]
fn mixed_graph_plan_matches_golden_fixture() {
    // Reuses matrix's own mixed-graph fixture (which includes an isolated pair with no bridge
    // to baseline): exercises genuinely disconnected pairs (no estimate, no CI), fragile-bridge
    // direct/inferred cells (elo_diff exists but no reliable CI - same "no estimate yet" note as
    // disconnected, different underlying reason), and real finite-CI baseline-vs-candidate
    // estimates, all through one --min-elo scan.
    check_golden(
        "plan_mixed_graph",
        &[
            "plan",
            "tests/fixtures/matrix_legacy_a.jsonl",
            "tests/fixtures/matrix_legacy_b.jsonl",
            "--matches",
            "tests/fixtures/matrix_matches.jsonl",
            "--min-elo",
            "20",
        ],
    );
}
