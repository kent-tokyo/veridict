use super::*;

#[test]
fn paired_by_id_nets_asymmetric_pairs_to_a_pass() {
    let stdin: String = (0..30)
        .flat_map(|i| {
            [
                format!("{{\"id\":\"op{i}\",\"result\":\"candidate_win\"}}\n"),
                format!("{{\"id\":\"op{i}\",\"result\":\"draw\"}}\n"),
            ]
        })
        .collect();
    veridict()
        .args([
            "compare",
            "-",
            "--metric",
            "winrate",
            "--paired-by-id",
            "--min-effect",
            "0.05",
        ])
        .write_stdin(stdin)
        .assert()
        .code(0)
        .stdout(predicate::str::contains("\"paired_count\": 30"));
}

#[test]
fn paired_by_id_allows_duplicate_ids_mean_diff_otherwise_rejects() {
    let stdin = "{\"id\":\"dup\",\"baseline\":1.0,\"candidate\":1.1}\n{\"id\":\"dup\",\"baseline\":2.0,\"candidate\":2.1}\n";
    veridict()
        .args(["compare", "-", "--metric", "mean-diff", "--paired-by-id"])
        .write_stdin(stdin)
        .assert()
        .code(predicate::in_iter([0, 1, 2]))
        .stdout(predicate::str::contains("\"paired_count\": 1"));
}

#[test]
fn paired_by_id_rejects_triple_id() {
    let stdin = "{\"id\":\"op1\",\"result\":\"candidate_win\"}\n{\"id\":\"op1\",\"result\":\"candidate_win\"}\n{\"id\":\"op1\",\"result\":\"candidate_win\"}\n";
    veridict()
        .args(["compare", "-", "--metric", "winrate", "--paired-by-id"])
        .write_stdin(stdin)
        .assert()
        .code(3)
        .stderr(predicate::str::contains("paired-by-id"));
}

// --- --cluster-by-id ---

#[test]
fn cluster_by_id_adds_the_new_report_fields() {
    // 20 openings, each replayed 5 times, candidate wins every game - a clean pass regardless of
    // whether the CI accounts for clustering, but cluster_count/max_cluster_size/
    // effective_sample_size/design_effect must appear and be self-consistent.
    let stdin: String = (0..20)
        .flat_map(|i| {
            (0..5).map(move |_| format!("{{\"id\":\"op{i}\",\"result\":\"candidate_win\"}}\n"))
        })
        .collect();
    veridict()
        .args([
            "compare",
            "-",
            "--metric",
            "winrate",
            "--cluster-by-id",
            "--min-effect",
            "0.1",
        ])
        .write_stdin(stdin)
        .assert()
        .code(0)
        .stdout(predicate::str::contains("\"paired_count\": 100"))
        .stdout(predicate::str::contains("\"cluster_count\": 20"))
        .stdout(predicate::str::contains("\"max_cluster_size\": 5"))
        .stdout(predicate::str::contains("\"effective_sample_size\":"))
        .stdout(predicate::str::contains("\"design_effect\":"));
}

#[test]
fn cluster_by_id_leaves_estimated_additional_trials_null_even_when_inconclusive() {
    // Same heterogeneous data as the CI-widening test below: unclustered reads as a confident
    // pass, clustered reads inconclusive. A naive port of estimate_additional_trials would
    // binary-search wilson/jeffreys/exact against a CI that's actually a cluster bootstrap -
    // wrong math, and paired_count is the wrong n to scale from (the cluster is the independent
    // unit). Must stay null rather than print a misleadingly precise number.
    let stdin: String =
        (0..20)
            .flat_map(|i| {
                let (cand, base) = if i % 2 == 0 { (5, 0) } else { (1, 4) };
                (0..cand)
                    .map(|_| format!("{{\"id\":\"op{i}\",\"result\":\"candidate_win\"}}\n"))
                    .chain((0..base).map(move |_| {
                        format!("{{\"id\":\"op{i}\",\"result\":\"baseline_win\"}}\n")
                    }))
                    .collect::<Vec<_>>()
            })
            .collect();
    let out = veridict()
        .args(["compare", "-", "--metric", "winrate", "--cluster-by-id"])
        .write_stdin(stdin)
        .output()
        .unwrap();
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["verdict"], "inconclusive");
    assert!(report["estimated_additional_trials"].is_null());
}

#[test]
fn cluster_by_id_widens_the_ci_versus_treating_clustered_games_as_independent() {
    // 20 openings with real between-cluster heterogeneity: half strongly favor the candidate
    // (5-0), half strongly favor the baseline (1-4) - same aggregate p=0.6 a naive Wilson CI on
    // 100 "independent" trials would read as fairly precise, but there are really only ~20
    // independent units of evidence here (which opening you drew dominates the outcome, not
    // game-to-game noise) - the cluster bootstrap must be visibly wider.
    let stdin: String =
        (0..20)
            .flat_map(|i| {
                let (cand, base) = if i % 2 == 0 { (5, 0) } else { (1, 4) };
                (0..cand)
                    .map(|_| format!("{{\"id\":\"op{i}\",\"result\":\"candidate_win\"}}\n"))
                    .chain((0..base).map(move |_| {
                        format!("{{\"id\":\"op{i}\",\"result\":\"baseline_win\"}}\n")
                    }))
                    .collect::<Vec<_>>()
            })
            .collect();
    let clustered = veridict()
        .args(["compare", "-", "--metric", "winrate", "--cluster-by-id"])
        .write_stdin(stdin.clone())
        .output()
        .unwrap();
    let unclustered = veridict()
        .args(["compare", "-", "--metric", "winrate"])
        .write_stdin(stdin)
        .output()
        .unwrap();
    let clustered: serde_json::Value = serde_json::from_slice(&clustered.stdout).unwrap();
    let unclustered: serde_json::Value = serde_json::from_slice(&unclustered.stdout).unwrap();
    let width =
        |r: &serde_json::Value| r["ci_high"].as_f64().unwrap() - r["ci_low"].as_f64().unwrap();
    assert!(
        width(&clustered) > width(&unclustered),
        "cluster CI width {} should exceed the naive i.i.d. CI width {}",
        width(&clustered),
        width(&unclustered)
    );
    assert!(clustered["design_effect"].as_f64().unwrap() > 1.0);
}

#[test]
fn cluster_by_id_conflicts_with_paired_by_id() {
    veridict()
        .args([
            "compare",
            "-",
            "--metric",
            "winrate",
            "--cluster-by-id",
            "--paired-by-id",
        ])
        .write_stdin("{\"result\":\"candidate_win\"}\n")
        .assert()
        .code(3)
        .stderr(predicate::str::contains("cannot be used with"));
}

#[test]
fn cluster_by_id_rejected_for_mean_diff() {
    let stdin = "{\"id\":\"a\",\"baseline\":1.0,\"candidate\":1.1}\n";
    veridict()
        .args(["compare", "-", "--metric", "mean-diff", "--cluster-by-id"])
        .write_stdin(stdin)
        .assert()
        .code(3)
        .stderr(predicate::str::contains("--cluster-by-id"));
}

#[test]
fn cluster_by_id_conflicts_with_claim_correction() {
    let stdin: String = (0..20)
        .flat_map(|i| {
            (0..5).map(move |_| format!("{{\"id\":\"op{i}\",\"result\":\"candidate_win\"}}\n"))
        })
        .collect();
    veridict()
        .args([
            "compare",
            "-",
            "--metric",
            "winrate",
            "--cluster-by-id",
            "--claim-correction",
            "bonferroni",
        ])
        .write_stdin(stdin)
        .assert()
        .code(3)
        .stderr(predicate::str::contains("--claim-correction"))
        .stderr(predicate::str::contains("--cluster-by-id"));
}

#[test]
fn cluster_by_id_conflicts_with_the_deprecated_correction_alias_too() {
    let stdin: String = (0..20)
        .flat_map(|i| {
            (0..5).map(move |_| format!("{{\"id\":\"op{i}\",\"result\":\"candidate_win\"}}\n"))
        })
        .collect();
    veridict()
        .args([
            "compare",
            "-",
            "--metric",
            "winrate",
            "--cluster-by-id",
            "--correction",
            "bonferroni",
        ])
        .write_stdin(stdin)
        .assert()
        .code(3)
        .stderr(predicate::str::contains("--cluster-by-id"));
}

#[test]
fn cluster_by_id_with_explicit_claim_correction_none_still_succeeds() {
    // --claim-correction none is a no-op (apply_correction returns immediately, same as if the
    // flag were never passed) - only a real correction method conflicts with --cluster-by-id.
    let stdin: String = (0..20)
        .flat_map(|i| {
            (0..5).map(move |_| format!("{{\"id\":\"op{i}\",\"result\":\"candidate_win\"}}\n"))
        })
        .collect();
    veridict()
        .args([
            "compare",
            "-",
            "--metric",
            "winrate",
            "--cluster-by-id",
            "--claim-correction",
            "none",
            "--min-effect",
            "0.1",
        ])
        .write_stdin(stdin)
        .assert()
        .code(0)
        .stdout(predicate::str::contains("\"cluster_count\": 20"));
}

#[test]
fn cluster_by_id_works_for_elo() {
    let stdin: String = (0..20)
        .flat_map(|i| {
            (0..5).map(move |_| format!("{{\"id\":\"op{i}\",\"result\":\"candidate_win\"}}\n"))
        })
        .collect();
    veridict()
        .args([
            "compare",
            "-",
            "--metric",
            "elo",
            "--cluster-by-id",
            "--min-effect",
            "10",
        ])
        .write_stdin(stdin)
        .assert()
        .code(0)
        .stdout(predicate::str::contains("\"cluster_count\": 20"));
}

#[test]
fn sprt_paired_by_id_nets_split_pairs_to_a_draw() {
    let stdin: String = (0..1000)
        .flat_map(|i| {
            [
                format!("{{\"id\":\"op{i}\",\"result\":\"candidate_win\"}}\n"),
                format!("{{\"id\":\"op{i}\",\"result\":\"baseline_win\"}}\n"),
            ]
        })
        .collect();
    veridict()
        .args(["sprt", "-", "--elo0", "0", "--elo1", "10", "--paired-by-id"])
        .write_stdin(stdin)
        .assert()
        .code(2)
        .stdout(predicate::str::contains("\"llr\": 0.0"));
}

#[test]
fn sprt_pentanomial_clear_h1_stream_passes() {
    let stdin: String = (0..200)
        .flat_map(|i| {
            [
                format!("{{\"id\":\"op{i}\",\"result\":\"candidate_win\"}}\n"),
                format!("{{\"id\":\"op{i}\",\"result\":\"candidate_win\"}}\n"),
            ]
        })
        .collect();
    veridict()
        .args([
            "sprt",
            "-",
            "--sprt-variant",
            "pentanomial",
            "--elo0",
            "0",
            "--elo1",
            "10",
            "--paired-by-id",
        ])
        .write_stdin(stdin)
        .assert()
        .code(0)
        .stdout(predicate::str::contains("\"verdict\": \"pass\""))
        .stdout(predicate::str::contains(
            "\"sprt_variant\": \"pentanomial\"",
        ))
        // The sequential walk stops as soon as it crosses the upper bound (pair 104 for this
        // unanimous, elo0=0/elo1=10 stream) - it never analyzes the remaining 96 of the 200
        // input pairs. See sprt.rs's module doc.
        .stdout(predicate::str::contains("\"score_2_0\": 104"))
        .stdout(predicate::str::contains("\"available_paired_count\": 200"));
}

#[test]
fn sprt_pentanomial_requires_paired_by_id() {
    veridict()
        .args([
            "sprt",
            "-",
            "--sprt-variant",
            "pentanomial",
            "--elo0",
            "0",
            "--elo1",
            "10",
        ])
        .write_stdin("{\"id\":\"op1\",\"result\":\"draw\"}\n")
        .assert()
        .code(3)
        .stderr(predicate::str::contains("requires --paired-by-id"));
}

#[test]
fn sprt_pentanomial_rejects_belo_flags() {
    veridict()
        .args([
            "sprt",
            "-",
            "--sprt-variant",
            "pentanomial",
            "--belo0",
            "0",
            "--belo1",
            "10",
            "--paired-by-id",
        ])
        .write_stdin("{\"id\":\"op1\",\"result\":\"draw\"}\n")
        .assert()
        .code(3)
        .stderr(predicate::str::contains(
            "--belo0/--belo1 are only used with --sprt-variant trinomial",
        ));
}

#[test]
fn sprt_pentanomial_rejects_incomplete_pair() {
    let stdin = "{\"id\":\"op1\",\"result\":\"candidate_win\"}\n";
    veridict()
        .args([
            "sprt",
            "-",
            "--sprt-variant",
            "pentanomial",
            "--elo0",
            "0",
            "--elo1",
            "10",
            "--paired-by-id",
        ])
        .write_stdin(stdin)
        .assert()
        .code(3)
        .stderr(predicate::str::contains("pentanomial"));
}

#[test]
fn sprt_pentanomial_rejects_triple_id() {
    let stdin = "{\"id\":\"op1\",\"result\":\"candidate_win\"}\n{\"id\":\"op1\",\"result\":\"candidate_win\"}\n{\"id\":\"op1\",\"result\":\"candidate_win\"}\n";
    veridict()
        .args([
            "sprt",
            "-",
            "--sprt-variant",
            "pentanomial",
            "--elo0",
            "0",
            "--elo1",
            "10",
            "--paired-by-id",
        ])
        .write_stdin(stdin)
        .assert()
        .code(3)
        .stderr(predicate::str::contains("pentanomial"));
}

#[test]
fn sprt_min_paired_ids_forces_inconclusive_below_minimum() {
    let stdin: String = (0..200)
        .flat_map(|i| {
            [
                format!("{{\"id\":\"op{i}\",\"result\":\"candidate_win\"}}\n"),
                format!("{{\"id\":\"op{i}\",\"result\":\"candidate_win\"}}\n"),
            ]
        })
        .collect();
    veridict()
        .args([
            "sprt",
            "-",
            "--sprt-variant",
            "pentanomial",
            "--elo0",
            "0",
            "--elo1",
            "10",
            "--paired-by-id",
            "--min-paired-ids",
            "300",
        ])
        .write_stdin(stdin)
        .assert()
        .code(2)
        .stdout(predicate::str::contains("\"verdict\": \"inconclusive\""))
        .stdout(predicate::str::contains("\"min_paired_ids\": 300"))
        .stdout(predicate::str::contains("\"validity\": \"valid\""));
}

#[test]
fn sprt_min_paired_ids_requires_pentanomial_variant() {
    veridict()
        .args([
            "sprt",
            "-",
            "--elo0",
            "0",
            "--elo1",
            "10",
            "--min-paired-ids",
            "300",
        ])
        .write_stdin("{\"result\":\"candidate_win\"}\n")
        .assert()
        .code(3)
        .stderr(predicate::str::contains(
            "require --sprt-variant pentanomial",
        ));
}

#[test]
fn sprt_max_paired_ids_zero_is_rejected() {
    veridict()
        .args([
            "sprt",
            "-",
            "--sprt-variant",
            "pentanomial",
            "--elo0",
            "0",
            "--elo1",
            "10",
            "--paired-by-id",
            "--max-paired-ids",
            "0",
        ])
        .write_stdin("{\"id\":\"op1\",\"result\":\"candidate_win\"}\n{\"id\":\"op1\",\"result\":\"candidate_win\"}\n")
        .assert()
        .code(3)
        .stderr(predicate::str::contains("--max-paired-ids must be >= 1"));
}

#[test]
fn sprt_min_paired_ids_must_not_exceed_max_paired_ids() {
    veridict()
        .args([
            "sprt",
            "-",
            "--sprt-variant",
            "pentanomial",
            "--elo0",
            "0",
            "--elo1",
            "10",
            "--paired-by-id",
            "--min-paired-ids",
            "500",
            "--max-paired-ids",
            "100",
        ])
        .write_stdin("{\"id\":\"op1\",\"result\":\"candidate_win\"}\n{\"id\":\"op1\",\"result\":\"candidate_win\"}\n")
        .assert()
        .code(3)
        .stderr(predicate::str::contains(
            "--min-paired-ids (500) must be <= --max-paired-ids (100)",
        ));
}

#[test]
fn sprt_require_complete_pairs_rejects_lone_wald_id() {
    let stdin = "{\"id\":\"op1\",\"result\":\"candidate_win\"}\n{\"id\":\"op1\",\"result\":\"baseline_win\"}\n{\"id\":\"op2\",\"result\":\"candidate_win\"}\n";
    veridict()
        .args([
            "sprt",
            "-",
            "--elo0",
            "0",
            "--elo1",
            "10",
            "--paired-by-id",
            "--require-complete-pairs",
        ])
        .write_stdin(stdin)
        .assert()
        .code(3)
        .stderr(predicate::str::contains(
            "requires exactly 2 records per id",
        ));
}

#[test]
fn sprt_require_complete_pairs_requires_paired_by_id() {
    veridict()
        .args([
            "sprt",
            "-",
            "--elo0",
            "0",
            "--elo1",
            "10",
            "--require-complete-pairs",
        ])
        .write_stdin("{\"result\":\"candidate_win\"}\n")
        .assert()
        .code(3)
        .stderr(predicate::str::contains("requires --paired-by-id"));
}

#[test]
fn ci_method_exact_widens_the_interval_versus_wilson() {
    let stdin = "{\"result\":\"candidate_win\"}\n{\"result\":\"candidate_win\"}\n{\"result\":\"candidate_win\"}\n{\"result\":\"candidate_win\"}\n{\"result\":\"baseline_win\"}\n";
    let wilson = veridict()
        .args([
            "compare",
            "-",
            "--metric",
            "winrate",
            "--ci-method",
            "wilson",
        ])
        .write_stdin(stdin)
        .output()
        .unwrap();
    let exact = veridict()
        .args([
            "compare",
            "-",
            "--metric",
            "winrate",
            "--ci-method",
            "exact",
        ])
        .write_stdin(stdin)
        .output()
        .unwrap();
    assert_ne!(wilson.stdout, exact.stdout);
}

#[test]
fn ci_method_exact_rejects_elo() {
    let stdin = "{\"result\":\"candidate_win\"}\n{\"result\":\"baseline_win\"}\n";
    veridict()
        .args(["compare", "-", "--metric", "elo", "--ci-method", "exact"])
        .write_stdin(stdin)
        .assert()
        .code(3)
        .stderr(predicate::str::contains("--ci-method exact"));
}

#[test]
fn ci_method_exact_rejects_mean_diff() {
    let stdin = "{\"baseline\":1.0,\"candidate\":1.1}\n";
    veridict()
        .args([
            "compare",
            "-",
            "--metric",
            "mean-diff",
            "--ci-method",
            "exact",
        ])
        .write_stdin(stdin)
        .assert()
        .code(3)
        .stderr(predicate::str::contains("--ci-method exact"));
}

#[test]
fn failure_policy_report_only_is_the_default_and_status_only_records_contribute_nothing() {
    let stdin = "{\"result\":\"candidate_win\"}\n{\"result\":\"baseline_win\"}\n\
                 {\"candidate_status\":\"crash\"}\n{\"baseline_status\":\"timeout\"}\n";
    veridict()
        .args(["compare", "-", "--metric", "winrate"])
        .write_stdin(stdin)
        .assert()
        .stdout(predicate::str::contains("\"paired_count\": 2"))
        .stdout(predicate::str::contains("\"crashes\": 1"))
        .stdout(predicate::str::contains("\"timeouts\": 1"));
}

#[test]
fn failure_policy_exclude_drops_a_result_next_to_a_failure_status() {
    // The behavioral divergence report-only/exclude actually have: a record carrying both a
    // failure status and a literal `result`. report-only still counts the result; exclude drops
    // it entirely, shrinking paired_count by one relative to report-only on the same input.
    let stdin = "{\"result\":\"candidate_win\"}\n{\"result\":\"baseline_win\"}\n\
                 {\"candidate_status\":\"crash\",\"result\":\"candidate_win\"}\n";
    let report_only = veridict()
        .args(["compare", "-", "--metric", "winrate"])
        .write_stdin(stdin)
        .output()
        .unwrap();
    let exclude = veridict()
        .args([
            "compare",
            "-",
            "--metric",
            "winrate",
            "--failure-policy",
            "exclude",
        ])
        .write_stdin(stdin)
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&report_only.stdout).contains("\"paired_count\": 3"));
    assert!(String::from_utf8_lossy(&exclude.stdout).contains("\"paired_count\": 2"));
}

#[test]
fn failure_policy_loss_synthesizes_a_baseline_win_on_candidate_failure() {
    let stdin = "{\"candidate_status\":\"crash\"}\n";
    veridict()
        .args([
            "compare",
            "-",
            "--metric",
            "winrate",
            "--failure-policy",
            "loss",
        ])
        .write_stdin(stdin)
        .assert()
        .stdout(predicate::str::contains("\"paired_count\": 1"))
        .stdout(predicate::str::contains("\"baseline_count\": 1"))
        .stdout(predicate::str::contains("\"candidate_count\": 0"));
}

#[test]
fn failure_policy_loss_both_sides_failing_nets_to_a_draw_excluded_from_winrate_n() {
    // winrate excludes draws from its decisive-trial denominator (same "decisive only"
    // convention as sprt wald) - a both-failed pair should net to a draw, not move
    // candidate_count/baseline_count at all.
    let stdin = "{\"baseline_status\":\"timeout\",\"candidate_status\":\"crash\"}\n";
    veridict()
        .args([
            "compare",
            "-",
            "--metric",
            "winrate",
            "--failure-policy",
            "loss",
        ])
        .write_stdin(stdin)
        .assert()
        .stdout(predicate::str::contains("\"paired_count\": 0"))
        .stdout(predicate::str::contains("\"candidate_count\": 0"))
        .stdout(predicate::str::contains("\"baseline_count\": 0"));
}

#[test]
fn failure_policy_loss_overrides_a_literal_result_on_the_same_record() {
    let stdin = "{\"candidate_status\":\"crash\",\"result\":\"candidate_win\"}\n";
    veridict()
        .args([
            "compare",
            "-",
            "--metric",
            "winrate",
            "--failure-policy",
            "loss",
        ])
        .write_stdin(stdin)
        .assert()
        .stdout(predicate::str::contains("\"baseline_count\": 1"))
        .stdout(predicate::str::contains("\"candidate_count\": 0"));
}

// The pairing interaction advisor flagged as the real hazard: a failure *inside* a
// --paired-by-id pair, not just a standalone failed record. Pair "op1": game 1 is a real
// candidate_win, game 2 is a candidate crash with no result. Under `loss`, game 2 synthesizes to
// baseline_win, so the pair nets candidate_win(1.0) + baseline_win(0.0) = 1.0 -> a net draw, the
// same "total points" convention any other paired outcome uses - `OutcomeCollector` never needs
// to know the outcome was synthesized rather than literal.
#[test]
fn failure_policy_loss_nets_correctly_inside_a_paired_by_id_pair() {
    let stdin = "{\"id\":\"op1\",\"result\":\"candidate_win\"}\n\
                 {\"id\":\"op1\",\"candidate_status\":\"crash\"}\n";
    veridict()
        .args([
            "compare",
            "-",
            "--metric",
            "winrate",
            "--paired-by-id",
            "--failure-policy",
            "loss",
        ])
        .write_stdin(stdin)
        .assert()
        .code(2)
        .stdout(predicate::str::contains("\"paired_count\": 0"));
}

#[test]
fn failure_policy_exclude_rejected_for_mean_diff() {
    let stdin = "{\"baseline\":1.0,\"candidate\":1.1}\n";
    veridict()
        .args([
            "compare",
            "-",
            "--metric",
            "mean-diff",
            "--failure-policy",
            "exclude",
        ])
        .write_stdin(stdin)
        .assert()
        .code(3)
        .stderr(predicate::str::contains("--failure-policy exclude"));
}

#[test]
fn failure_policy_loss_rejected_for_sign_test() {
    let stdin = "{\"baseline\":1.0,\"candidate\":1.1}\n";
    veridict()
        .args([
            "compare",
            "-",
            "--metric",
            "sign-test",
            "--failure-policy",
            "loss",
        ])
        .write_stdin(stdin)
        .assert()
        .code(3)
        .stderr(predicate::str::contains("--failure-policy loss"));
}

#[test]
fn failure_policy_loss_rejected_when_any_requested_metric_is_incompatible() {
    // Multiple --metric flags: the incompatible one must be caught regardless of position, not
    // just when it happens to be first.
    let stdin = "{\"baseline\":1.0,\"candidate\":1.1,\"result\":\"candidate_win\"}\n";
    veridict()
        .args([
            "compare",
            "-",
            "--metric",
            "winrate",
            "--metric",
            "mean-diff",
            "--failure-policy",
            "loss",
        ])
        .write_stdin(stdin)
        .assert()
        .code(3)
        .stderr(predicate::str::contains("--failure-policy loss"));
}

#[test]
fn failure_policy_sprt_loss_synthesizes_a_baseline_win_on_candidate_failure() {
    let stdin = (0..200)
        .map(|_| "{\"candidate_status\":\"crash\"}\n")
        .collect::<String>();
    veridict()
        .args([
            "sprt",
            "-",
            "--elo0",
            "0",
            "--elo1",
            "10",
            "--failure-policy",
            "loss",
        ])
        .write_stdin(stdin)
        .assert()
        .code(1)
        .stdout(predicate::str::contains("\"verdict\": \"fail\""))
        .stdout(predicate::str::contains("\"baseline_wins\": 200"));
}

// pentanomial x failure-policy: explicitly decided and tested, not discovered by accident. A
// crash on one side of a pair routes its synthesized outcome into `PentanomialCollector` exactly
// like a literal result would - candidate_win + (crash -> baseline_win) nets to pentanomial
// bucket score_1_0 (the pair's combined score is 1.0), same math as compare's --paired-by-id.
#[test]
fn failure_policy_sprt_pentanomial_loss_synthesizes_outcome_inside_a_pair() {
    let stdin = "{\"id\":\"op1\",\"result\":\"candidate_win\"}\n\
                 {\"id\":\"op1\",\"candidate_status\":\"crash\"}\n";
    veridict()
        .args([
            "sprt",
            "-",
            "--sprt-variant",
            "pentanomial",
            "--elo0",
            "0",
            "--elo1",
            "10",
            "--paired-by-id",
            "--failure-policy",
            "loss",
        ])
        .write_stdin(stdin)
        .assert()
        .stdout(predicate::str::contains("\"score_1_0\": 1"))
        .stdout(predicate::str::contains("\"paired_count\": 1"));
}

#[test]
fn ci_method_jeffreys_differs_from_wilson() {
    let stdin = "{\"result\":\"candidate_win\"}\n{\"result\":\"candidate_win\"}\n{\"result\":\"candidate_win\"}\n{\"result\":\"candidate_win\"}\n{\"result\":\"baseline_win\"}\n";
    let wilson = veridict()
        .args([
            "compare",
            "-",
            "--metric",
            "winrate",
            "--ci-method",
            "wilson",
        ])
        .write_stdin(stdin)
        .output()
        .unwrap();
    let jeffreys = veridict()
        .args([
            "compare",
            "-",
            "--metric",
            "winrate",
            "--ci-method",
            "jeffreys",
        ])
        .write_stdin(stdin)
        .output()
        .unwrap();
    assert_ne!(wilson.stdout, jeffreys.stdout);
}

#[test]
fn ci_method_jeffreys_rejects_elo() {
    let stdin = "{\"result\":\"candidate_win\"}\n{\"result\":\"baseline_win\"}\n";
    veridict()
        .args(["compare", "-", "--metric", "elo", "--ci-method", "jeffreys"])
        .write_stdin(stdin)
        .assert()
        .code(3)
        .stderr(predicate::str::contains("--ci-method jeffreys"));
}
