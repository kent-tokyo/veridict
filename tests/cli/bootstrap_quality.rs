use super::*;

#[test]
fn bootstrap_method_bca_differs_from_percentile_on_skewed_data() {
    let diffs = [
        0.05, 0.08, 0.12, 0.02, 0.15, 0.01, 0.30, 0.04, 0.06, 0.50, 0.03, 0.09, 0.11, 0.07, 0.02,
        0.60, 0.04, 0.08, 0.10, 0.05,
    ];
    let stdin: String = diffs
        .iter()
        .enumerate()
        .map(|(i, d)| format!("{{\"baseline\":{i}.0,\"candidate\":{}}}\n", i as f64 + d))
        .collect();
    let percentile = veridict()
        .args([
            "compare",
            "-",
            "--metric",
            "mean-diff",
            "--bootstrap-method",
            "percentile",
        ])
        .write_stdin(stdin.clone())
        .output()
        .unwrap();
    let bca = veridict()
        .args([
            "compare",
            "-",
            "--metric",
            "mean-diff",
            "--bootstrap-method",
            "bca",
        ])
        .write_stdin(stdin)
        .output()
        .unwrap();
    assert_ne!(percentile.stdout, bca.stdout);
}

#[test]
fn bootstrap_method_basic_differs_from_percentile_on_skewed_data() {
    let diffs = [
        0.05, 0.08, 0.12, 0.02, 0.15, 0.01, 0.30, 0.04, 0.06, 0.50, 0.03, 0.09, 0.11, 0.07, 0.02,
        0.60, 0.04, 0.08, 0.10, 0.05,
    ];
    let stdin: String = diffs
        .iter()
        .enumerate()
        .map(|(i, d)| format!("{{\"baseline\":{i}.0,\"candidate\":{}}}\n", i as f64 + d))
        .collect();
    let percentile = veridict()
        .args([
            "compare",
            "-",
            "--metric",
            "mean-diff",
            "--bootstrap-method",
            "percentile",
        ])
        .write_stdin(stdin.clone())
        .output()
        .unwrap();
    let basic = veridict()
        .args([
            "compare",
            "-",
            "--metric",
            "mean-diff",
            "--bootstrap-method",
            "basic",
        ])
        .write_stdin(stdin)
        .output()
        .unwrap();
    assert_ne!(percentile.stdout, basic.stdout);
}

#[test]
fn tiny_sample_warning_appears_in_json_report() {
    let stdin = (0..10)
        .map(|_| "{\"result\":\"candidate_win\"}\n")
        .collect::<String>();
    veridict()
        .args([
            "compare",
            "-",
            "--metric",
            "winrate",
            "--min-effect",
            "0.01",
        ])
        .write_stdin(stdin)
        .assert()
        .stdout(predicate::str::contains("small sample"));
}

#[test]
fn clean_large_sample_has_empty_warnings_array() {
    let stdin = (0..40)
        .map(|_| "{\"result\":\"candidate_win\"}\n")
        .collect::<String>();
    veridict()
        .args([
            "compare",
            "-",
            "--metric",
            "winrate",
            "--min-effect",
            "0.01",
        ])
        .write_stdin(stdin)
        .assert()
        .stdout(predicate::str::contains("\"warnings\": []"));
}

#[test]
fn low_id_diversity_warning_appears_when_one_id_dominates_unpaired() {
    // id "dup" repeated 5 times plus 7 distinct ids - the "same test case
    // logged multiple times" pattern the check exists to catch.
    let mut stdin = "{\"id\":\"dup\",\"result\":\"candidate_win\"}\n".repeat(5);
    for i in 0..7 {
        stdin.push_str(&format!(
            "{{\"id\":\"r{i}\",\"result\":\"candidate_win\"}}\n"
        ));
    }
    veridict()
        .args([
            "compare",
            "-",
            "--metric",
            "winrate",
            "--min-effect",
            "0.01",
        ])
        .write_stdin(stdin)
        .assert()
        .stdout(predicate::str::contains("low id diversity"))
        .stdout(predicate::str::contains("\"low_id_diversity\": true"));
}

#[test]
fn low_id_diversity_is_silent_when_every_id_appears_exactly_twice() {
    // The common, innocent mistake: genuinely paired data run without
    // --paired-by-id. Must stay silent - firing here would be noise.
    let stdin = (0..6)
        .flat_map(|i| {
            [
                format!("{{\"id\":\"pair{i}\",\"result\":\"candidate_win\"}}\n"),
                format!("{{\"id\":\"pair{i}\",\"result\":\"candidate_win\"}}\n"),
            ]
        })
        .collect::<String>();
    veridict()
        .args([
            "compare",
            "-",
            "--metric",
            "winrate",
            "--min-effect",
            "0.01",
        ])
        .write_stdin(stdin)
        .assert()
        .stdout(predicate::str::contains("\"low_id_diversity\": false"))
        .stdout(predicate::str::contains("low id diversity").not());
}

// --- --correction ---
// 62 candidate wins / 38 baseline wins out of 100 is a real, verified marginal pass at the
