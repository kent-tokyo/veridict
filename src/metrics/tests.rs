use super::*;
use crate::BootstrapMethod;

const SEED: u64 = crate::stats::bootstrap::DEFAULT_SEED;

fn rec(
    id: &str,
    baseline: Option<f64>,
    candidate: Option<f64>,
    result: Option<&str>,
    bs: Option<&str>,
    cs: Option<&str>,
) -> Record {
    Record {
        id: Some(id.to_string()),
        baseline,
        candidate,
        result: result.map(str::to_string),
        baseline_status: bs.map(str::to_string),
        candidate_status: cs.map(str::to_string),
    }
}

#[test]
fn effective_outcome_report_only_ignores_a_status_only_failure() {
    let out = effective_outcome(
        FailurePolicy::ReportOnly,
        Some(TrialStatus::Crash),
        None,
        None,
        1,
    )
    .unwrap();
    assert_eq!(out, None);
}

#[test]
fn effective_outcome_report_only_still_honors_a_result_next_to_a_failure() {
    // Mixed status+result: `ReportOnly` keeps today's pre-`FailurePolicy` behavior - the
    // literal `result` still counts, the failure status is only tallied, not consulted here.
    let out = effective_outcome(
        FailurePolicy::ReportOnly,
        None,
        Some(TrialStatus::Crash),
        Some("candidate_win"),
        1,
    )
    .unwrap();
    assert_eq!(out, Some(Outcome::CandidateWin));
}

#[test]
fn effective_outcome_exclude_drops_a_result_next_to_a_failure() {
    let out = effective_outcome(
        FailurePolicy::Exclude,
        None,
        Some(TrialStatus::Crash),
        Some("candidate_win"),
        1,
    )
    .unwrap();
    assert_eq!(out, None);
}

#[test]
fn effective_outcome_exclude_matches_report_only_without_a_failure() {
    let out =
        effective_outcome(FailurePolicy::Exclude, None, None, Some("candidate_win"), 1).unwrap();
    assert_eq!(out, Some(Outcome::CandidateWin));
}

#[test]
fn effective_outcome_loss_synthesizes_baseline_win_on_a_candidate_failure() {
    let out = effective_outcome(
        FailurePolicy::Loss,
        None,
        Some(TrialStatus::Timeout),
        None,
        1,
    )
    .unwrap();
    assert_eq!(out, Some(Outcome::BaselineWin));
}

#[test]
fn effective_outcome_loss_synthesizes_candidate_win_on_a_baseline_failure() {
    let out =
        effective_outcome(FailurePolicy::Loss, Some(TrialStatus::Crash), None, None, 1).unwrap();
    assert_eq!(out, Some(Outcome::CandidateWin));
}

#[test]
fn effective_outcome_loss_both_sides_failing_nets_to_a_draw() {
    let out = effective_outcome(
        FailurePolicy::Loss,
        Some(TrialStatus::Invalid),
        Some(TrialStatus::Crash),
        None,
        1,
    )
    .unwrap();
    assert_eq!(out, Some(Outcome::Draw));
}

#[test]
fn effective_outcome_loss_overrides_a_literal_result() {
    // The crash wins even though `result` says `candidate_win` - AGENTS.md's "a false pass
    // is worse than an inconclusive result" means the execution-level failure signal is
    // trusted over a same-record `result` field, not the other way around.
    let out = effective_outcome(
        FailurePolicy::Loss,
        None,
        Some(TrialStatus::Crash),
        Some("candidate_win"),
        1,
    )
    .unwrap();
    assert_eq!(out, Some(Outcome::BaselineWin));
}

#[test]
fn effective_outcome_loss_falls_through_to_result_without_a_failure() {
    let out = effective_outcome(FailurePolicy::Loss, None, None, Some("candidate_win"), 1).unwrap();
    assert_eq!(out, Some(Outcome::CandidateWin));
}

#[test]
fn effective_outcome_malformed_result_next_to_a_failure_is_not_an_error_under_exclude() {
    // Once a policy has decided the trial's outcome is excluded, an unparseable `result`
    // alongside it is moot, not silently-ignored invalid data - see `effective_outcome`'s doc.
    let out = effective_outcome(
        FailurePolicy::Exclude,
        None,
        Some(TrialStatus::Crash),
        Some("not-a-real-outcome"),
        1,
    )
    .unwrap();
    assert_eq!(out, None);
}

#[test]
fn effective_outcome_malformed_result_is_still_an_error_when_actually_consulted() {
    assert!(matches!(
        effective_outcome(
            FailurePolicy::ReportOnly,
            None,
            None,
            Some("not-a-real-outcome"),
            1
        ),
        Err(VeridictError::UnrecognizedOutcome { .. })
    ));
}

#[test]
fn winrate_excludes_draws_from_n() {
    let records = [
        (1, rec("a", None, None, Some("candidate_win"), None, None)),
        (2, rec("b", None, None, Some("baseline_win"), None, None)),
        (3, rec("c", None, None, Some("draw"), None, None)),
    ];
    let out = compute(
        records.iter().cloned(),
        MetricConfig::WinRate {
            ci_method: CiMethod::Wilson,
            failure_policy: FailurePolicy::ReportOnly,
        },
        0.95,
        1000,
        SEED,
        false,
        false,
    )
    .unwrap();
    assert_eq!(out.paired_count, 2);
    assert_eq!(out.candidate_count, 1);
}

#[test]
fn winrate_all_draws_is_zero_n_warning() {
    let records = [(1, rec("a", None, None, Some("draw"), None, None))];
    let out = compute(
        records.iter().cloned(),
        MetricConfig::WinRate {
            ci_method: CiMethod::Wilson,
            failure_policy: FailurePolicy::ReportOnly,
        },
        0.95,
        1000,
        SEED,
        false,
        false,
    )
    .unwrap();
    assert!(out.warning.is_some());
    assert_eq!(out.paired_count, 0);
}

#[test]
fn mean_diff_paired_count_and_effect() {
    let records = [
        (1, rec("a", Some(1.0), Some(1.5), None, None, None)),
        (2, rec("b", Some(2.0), Some(2.5), None, None, None)),
    ];
    let out = compute(
        records.iter().cloned(),
        MetricConfig::MeanDiff {
            bootstrap_method: BootstrapMethod::Percentile,
        },
        0.95,
        1000,
        SEED,
        false,
        false,
    )
    .unwrap();
    assert_eq!(out.paired_count, 2);
    assert!((out.effect - 0.5).abs() < 1e-9);
}

#[test]
fn mean_diff_rejects_nan() {
    let records = [(1, rec("a", Some(f64::NAN), Some(1.0), None, None, None))];
    let result = compute(
        records.iter().cloned(),
        MetricConfig::MeanDiff {
            bootstrap_method: BootstrapMethod::Percentile,
        },
        0.95,
        1000,
        SEED,
        false,
        false,
    );
    assert!(matches!(
        result,
        Err(VeridictError::InvalidValue {
            field: "baseline",
            ..
        })
    ));
}

#[test]
fn numeric_difference_metrics_reject_an_overflowed_derived_difference() {
    let records = [(1, rec("a", Some(-1e308), Some(1e308), None, None, None))];
    let configs = [
        MetricConfig::MeanDiff {
            bootstrap_method: BootstrapMethod::Percentile,
        },
        MetricConfig::QuantileDiff {
            quantile: 0.5,
            bootstrap_method: BootstrapMethod::Percentile,
        },
        MetricConfig::SignTest {
            ci_method: CiMethod::Wilson,
        },
    ];

    for config in configs {
        let result = compute(
            records.iter().cloned(),
            config,
            0.95,
            1000,
            SEED,
            false,
            false,
        );
        assert!(matches!(
            result,
            Err(VeridictError::InvalidValue {
                line: 1,
                field: "candidate_minus_baseline",
                ..
            })
        ));
    }
}

#[test]
fn mean_diff_pair_average_stays_finite_for_extreme_values() {
    let records = [
        (1, rec("pair", Some(0.0), Some(f64::MAX), None, None, None)),
        (2, rec("pair", Some(0.0), Some(f64::MAX), None, None, None)),
    ];
    let out = compute(
        records.iter().cloned(),
        MetricConfig::MeanDiff {
            bootstrap_method: BootstrapMethod::Percentile,
        },
        0.95,
        100,
        SEED,
        true,
        false,
    )
    .unwrap();
    assert_eq!(out.effect, f64::MAX);
    assert_eq!(out.ci_low, f64::MAX);
    assert_eq!(out.ci_high, f64::MAX);
}

#[test]
fn final_metric_boundary_rejects_non_finite_output() {
    let records = [(1, rec("a", Some(1.0), Some(2.0), None, None, None))];
    let mut out = compute(
        records.iter().cloned(),
        MetricConfig::MeanDiff {
            bootstrap_method: BootstrapMethod::Percentile,
        },
        0.95,
        100,
        SEED,
        false,
        false,
    )
    .unwrap();
    out.ci_low = f64::NEG_INFINITY;

    assert!(matches!(
        validate_finite_output(MetricKind::MeanDiff, &out),
        Err(VeridictError::NonFiniteStatistic {
            metric: "metric mean-diff",
            field: "ci_low",
            ..
        })
    ));
}

#[test]
fn direct_quantile_config_construction_is_revalidated() {
    let records = [(1, rec("a", Some(1.0), Some(2.0), None, None, None))];
    for config in [
        MetricConfig::QuantileDiff {
            quantile: f64::NAN,
            bootstrap_method: BootstrapMethod::Percentile,
        },
        MetricConfig::QuantileDiff {
            quantile: 0.0,
            bootstrap_method: BootstrapMethod::Percentile,
        },
        MetricConfig::QuantileDiff {
            quantile: 0.5,
            bootstrap_method: BootstrapMethod::Bca,
        },
    ] {
        assert!(
            compute(
                records.iter().cloned(),
                config,
                0.95,
                100,
                SEED,
                false,
                false,
            )
            .is_err()
        );
    }
}

#[test]
fn mean_diff_rejects_duplicate_id() {
    let records = [
        (1, rec("dup", Some(1.0), Some(1.1), None, None, None)),
        (2, rec("dup", Some(2.0), Some(2.1), None, None, None)),
    ];
    let result = compute(
        records.iter().cloned(),
        MetricConfig::MeanDiff {
            bootstrap_method: BootstrapMethod::Percentile,
        },
        0.95,
        1000,
        SEED,
        false,
        false,
    );
    assert!(matches!(result, Err(VeridictError::DuplicateId { .. })));
}

#[test]
fn status_only_records_count_but_are_not_schema_mismatch() {
    let records = [(1, rec("a", None, None, None, Some("ok"), Some("timeout")))];
    let out = compute(
        records.iter().cloned(),
        MetricConfig::WinRate {
            ci_method: CiMethod::Wilson,
            failure_policy: FailurePolicy::ReportOnly,
        },
        0.95,
        1000,
        SEED,
        false,
        false,
    )
    .unwrap();
    assert_eq!(out.timeouts, 1);
    assert_eq!(out.failures.candidate.timeout, 1);
    assert_eq!(out.failures.baseline.timeout, 0);
}

#[test]
fn unusable_record_is_schema_mismatch() {
    let records = [(1, rec("a", None, None, None, None, None))];
    let result = compute(
        records.iter().cloned(),
        MetricConfig::WinRate {
            ci_method: CiMethod::Wilson,
            failure_policy: FailurePolicy::ReportOnly,
        },
        0.95,
        1000,
        SEED,
        false,
        false,
    );
    assert!(matches!(result, Err(VeridictError::SchemaMismatch { .. })));
}

#[test]
fn unrecognized_status_is_an_error() {
    let records = [(1, rec("a", None, None, None, Some("bogus"), None))];
    let result = compute(
        records.iter().cloned(),
        MetricConfig::WinRate {
            ci_method: CiMethod::Wilson,
            failure_policy: FailurePolicy::ReportOnly,
        },
        0.95,
        1000,
        SEED,
        false,
        false,
    );
    assert!(matches!(
        result,
        Err(VeridictError::UnrecognizedStatus { .. })
    ));
}

#[test]
fn unrecognized_outcome_is_an_error() {
    let records = [(1, rec("a", None, None, Some("bogus"), None, None))];
    let result = compute(
        records.iter().cloned(),
        MetricConfig::WinRate {
            ci_method: CiMethod::Wilson,
            failure_policy: FailurePolicy::ReportOnly,
        },
        0.95,
        1000,
        SEED,
        false,
        false,
    );
    assert!(matches!(
        result,
        Err(VeridictError::UnrecognizedOutcome { .. })
    ));
}

#[test]
fn empty_records_is_empty_input() {
    let result = compute(
        std::iter::empty::<(usize, Record)>(),
        MetricConfig::WinRate {
            ci_method: CiMethod::Wilson,
            failure_policy: FailurePolicy::ReportOnly,
        },
        0.95,
        1000,
        SEED,
        false,
        false,
    );
    assert!(matches!(result, Err(VeridictError::EmptyInput)));
}

#[test]
fn sign_test_excludes_ties() {
    let records = [
        (1, rec("a", Some(1.0), Some(1.5), None, None, None)), // positive
        (2, rec("b", Some(2.0), Some(1.0), None, None, None)), // negative
        (3, rec("c", Some(3.0), Some(3.0), None, None, None)), // tie, excluded
    ];
    let out = compute(
        records.iter().cloned(),
        MetricConfig::SignTest {
            ci_method: CiMethod::Wilson,
        },
        0.95,
        1000,
        SEED,
        false,
        false,
    )
    .unwrap();
    assert_eq!(out.paired_count, 2);
    assert_eq!(out.candidate_count, 1);
    assert_eq!(out.baseline_count, 1);
}

#[test]
fn sign_test_all_ties_is_zero_n_warning() {
    let records = [(1, rec("a", Some(1.0), Some(1.0), None, None, None))];
    let out = compute(
        records.iter().cloned(),
        MetricConfig::SignTest {
            ci_method: CiMethod::Wilson,
        },
        0.95,
        1000,
        SEED,
        false,
        false,
    )
    .unwrap();
    assert!(out.warning.is_some());
}

#[test]
fn elo_counts_draws_as_half_a_point() {
    let records = [
        (1, rec("a", None, None, Some("candidate_win"), None, None)),
        (2, rec("b", None, None, Some("draw"), None, None)),
    ];
    let out = compute(
        records.iter().cloned(),
        MetricConfig::Elo {
            failure_policy: FailurePolicy::ReportOnly,
        },
        0.95,
        1000,
        SEED,
        false,
        false,
    )
    .unwrap();
    // score = (1 + 0.5) / 2 = 0.75 -> a positive Elo effect.
    assert!(out.effect > 0.0);
    assert_eq!(out.paired_count, 2);
}

#[test]
fn elo_even_record_is_zero_effect() {
    let records = [
        (1, rec("a", None, None, Some("candidate_win"), None, None)),
        (2, rec("b", None, None, Some("baseline_win"), None, None)),
    ];
    let out = compute(
        records.iter().cloned(),
        MetricConfig::Elo {
            failure_policy: FailurePolicy::ReportOnly,
        },
        0.95,
        1000,
        SEED,
        false,
        false,
    )
    .unwrap();
    assert!(out.effect.abs() < 1e-9);
}

#[test]
fn elo_zero_trials_is_a_warning_not_an_error() {
    let records = [(1, rec("a", None, None, None, Some("timeout"), None))];
    let out = compute(
        records.iter().cloned(),
        MetricConfig::Elo {
            failure_policy: FailurePolicy::ReportOnly,
        },
        0.95,
        1000,
        SEED,
        false,
        false,
    )
    .unwrap();
    assert!(out.warning.is_some());
}

// --- paired_by_id ---

#[test]
fn paired_winrate_nets_two_games_per_id_by_points() {
    let records = [
        // id "op1": candidate wins one, loses the other -> net draw (1.0 pt).
        (1, rec("op1", None, None, Some("candidate_win"), None, None)),
        (2, rec("op1", None, None, Some("baseline_win"), None, None)),
        // id "op2": candidate wins both -> net candidate win (2.0 pts).
        (3, rec("op2", None, None, Some("candidate_win"), None, None)),
        (4, rec("op2", None, None, Some("candidate_win"), None, None)),
    ];
    let out = compute(
        records.iter().cloned(),
        MetricConfig::WinRate {
            ci_method: CiMethod::Wilson,
            failure_policy: FailurePolicy::ReportOnly,
        },
        0.95,
        1000,
        SEED,
        true,
        false,
    )
    .unwrap();
    // 4 raw games -> 2 paired samples: 1 draw (excluded from n), 1 candidate win.
    assert_eq!(out.paired_count, 1);
    assert_eq!(out.candidate_count, 1);
    assert_eq!(out.baseline_count, 0);
}

#[test]
fn paired_winrate_unpaired_singleton_still_counts() {
    let records = [(
        1,
        rec("solo", None, None, Some("candidate_win"), None, None),
    )];
    let out = compute(
        records.iter().cloned(),
        MetricConfig::WinRate {
            ci_method: CiMethod::Wilson,
            failure_policy: FailurePolicy::ReportOnly,
        },
        0.95,
        1000,
        SEED,
        true,
        false,
    )
    .unwrap();
    assert_eq!(out.paired_count, 1);
    assert_eq!(out.candidate_count, 1);
}

#[test]
fn paired_winrate_rejects_triple_id() {
    let records = [
        (1, rec("op1", None, None, Some("candidate_win"), None, None)),
        (2, rec("op1", None, None, Some("candidate_win"), None, None)),
        (3, rec("op1", None, None, Some("candidate_win"), None, None)),
    ];
    let result = compute(
        records.iter().cloned(),
        MetricConfig::WinRate {
            ci_method: CiMethod::Wilson,
            failure_policy: FailurePolicy::ReportOnly,
        },
        0.95,
        1000,
        SEED,
        true,
        false,
    );
    assert!(matches!(
        result,
        Err(VeridictError::SchemaMismatch {
            context: "paired-by-id",
            ..
        })
    ));
}

#[test]
fn paired_mean_diff_averages_the_pair() {
    let records = [
        (1, rec("op1", Some(1.0), Some(1.2), None, None, None)), // diff +0.2
        (2, rec("op1", Some(1.0), Some(0.8), None, None, None)), // diff -0.2
    ];
    let out = compute(
        records.iter().cloned(),
        MetricConfig::MeanDiff {
            bootstrap_method: BootstrapMethod::Percentile,
        },
        0.95,
        1000,
        SEED,
        true,
        false,
    )
    .unwrap();
    assert_eq!(out.paired_count, 1);
    assert!(out.effect.abs() < 1e-9); // net-of-bias effect is ~0, not the two raw +-0.2 diffs
}

#[test]
fn paired_mean_diff_allows_duplicate_id_that_unpaired_mode_rejects() {
    let records = [
        (1, rec("dup", Some(1.0), Some(1.1), None, None, None)),
        (2, rec("dup", Some(2.0), Some(2.1), None, None, None)),
    ];
    let result = compute(
        records.iter().cloned(),
        MetricConfig::MeanDiff {
            bootstrap_method: BootstrapMethod::Percentile,
        },
        0.95,
        1000,
        SEED,
        true,
        false,
    );
    assert!(result.is_ok());
}

#[test]
fn mean_diff_recovers_a_small_residual_after_severe_cancellation() {
    let records = [
        (1, rec("a", Some(0.0), Some(1e16), None, None, None)),
        (2, rec("b", Some(0.0), Some(1.0), None, None, None)),
        (3, rec("c", Some(0.0), Some(-1e16), None, None, None)),
    ];
    let out = compute(
        records.iter().cloned(),
        MetricConfig::MeanDiff {
            bootstrap_method: BootstrapMethod::Percentile,
        },
        0.95,
        100,
        SEED,
        false,
        false,
    )
    .unwrap();

    assert!((out.effect - 1.0 / 3.0).abs() < 1e-15);
    assert!(out.ci_low.is_finite());
    assert!(out.ci_high.is_finite());
}

// --- relative-diff ---

fn relative_diff_compute(
    records: &[(usize, Record)],
    paired_by_id: bool,
) -> Result<MetricOutput, VeridictError> {
    compute(
        records.iter().cloned(),
        MetricConfig::RelativeDiff {
            bootstrap_method: BootstrapMethod::Percentile,
        },
        0.95,
        1000,
        SEED,
        paired_by_id,
        false,
    )
}

#[test]
fn relative_diff_basic_formula() {
    // mean of +0.10 and -0.10 is 0 - confirms both signs compute correctly in one pass.
    let records = [
        (1, rec("a", Some(100.0), Some(110.0), None, None, None)),
        (2, rec("b", Some(200.0), Some(180.0), None, None, None)),
    ];
    let out = relative_diff_compute(&records, false).unwrap();
    assert_eq!(out.paired_count, 2);
    assert!(out.effect.abs() < 1e-9);
}

#[test]
fn relative_diff_single_pair_matches_hand_computed_ratio() {
    let records = [(1, rec("a", Some(100.0), Some(110.0), None, None, None))];
    let out = relative_diff_compute(&records, false).unwrap();
    assert!((out.effect - 0.10).abs() < 1e-9);

    let records = [(1, rec("a", Some(200.0), Some(180.0), None, None, None))];
    let out = relative_diff_compute(&records, false).unwrap();
    assert!((out.effect - (-0.10)).abs() < 1e-9);
}

#[test]
fn relative_diff_effect_is_the_mean_of_ratios_not_a_ratio_of_sums() {
    // mean(candidate_i/baseline_i - 1): (0.05 + 0.05 + 0.05) / 3 = 0.05, unaffected by scale.
    let records = [
        (1, rec("a", Some(1_000.0), Some(1_050.0), None, None, None)),
        (
            2,
            rec("b", Some(100_000.0), Some(105_000.0), None, None, None),
        ),
        (
            3,
            rec(
                "c",
                Some(50_000_000.0),
                Some(52_500_000.0),
                None,
                None,
                None,
            ),
        ),
    ];
    let out = relative_diff_compute(&records, false).unwrap();
    assert!((out.effect - 0.05).abs() < 1e-9);
}

#[test]
fn relative_diff_rejects_zero_baseline() {
    let records = [(1, rec("a", Some(0.0), Some(1.0), None, None, None))];
    let result = relative_diff_compute(&records, false);
    assert!(matches!(
        result,
        Err(VeridictError::RelativeDiffRequiresPositiveBaseline { baseline, .. }) if baseline == 0.0
    ));
}

#[test]
fn relative_diff_rejects_negative_baseline() {
    let records = [(1, rec("a", Some(-5.0), Some(1.0), None, None, None))];
    let result = relative_diff_compute(&records, false);
    assert!(matches!(
        result,
        Err(VeridictError::RelativeDiffRequiresPositiveBaseline { baseline, .. }) if baseline == -5.0
    ));
}

#[test]
fn relative_diff_rejects_nan_baseline() {
    let records = [(1, rec("a", Some(f64::NAN), Some(1.0), None, None, None))];
    let result = relative_diff_compute(&records, false);
    assert!(matches!(
        result,
        Err(VeridictError::InvalidValue {
            field: "baseline",
            ..
        })
    ));
}

#[test]
fn relative_diff_rejects_infinite_baseline() {
    let records = [(
        1,
        rec("a", Some(f64::INFINITY), Some(1.0), None, None, None),
    )];
    let result = relative_diff_compute(&records, false);
    assert!(matches!(
        result,
        Err(VeridictError::InvalidValue {
            field: "baseline",
            ..
        })
    ));
}

#[test]
fn relative_diff_rejects_a_ratio_that_overflows_to_infinity() {
    // Both inputs are individually finite, but the division overflows f64::INFINITY.
    let records = [(1, rec("a", Some(1e-300), Some(1e300), None, None, None))];
    let result = relative_diff_compute(&records, false);
    assert!(matches!(
        result,
        Err(VeridictError::InvalidValue {
            field: "relative_diff",
            ..
        })
    ));
}

#[test]
fn relative_diff_accepts_a_negative_candidate_against_a_positive_baseline() {
    // candidate isn't mechanically rejected for being negative - only baseline's sign/zero-ness
    // is constrained.
    let records = [(1, rec("a", Some(10.0), Some(-5.0), None, None, None))];
    let out = relative_diff_compute(&records, false).unwrap();
    assert!((out.effect - (-1.5)).abs() < 1e-9);
}

#[test]
fn relative_diff_avoids_a_representable_subtraction_overflow() {
    let records = [(1, rec("a", Some(1e308), Some(-1e308), None, None, None))];
    let out = relative_diff_compute(&records, false).unwrap();
    assert_eq!(out.effect, -2.0);
    assert_eq!(out.ci_low, -2.0);
    assert_eq!(out.ci_high, -2.0);
}

#[test]
fn paired_relative_diff_averages_the_pair_after_transform_not_before() {
    // Deliberately unequal baselines within the pair: averaging the two *ratios* (r1=+0.10,
    // r2=-0.02 -> mean +0.04) differs from first averaging baseline/candidate and then taking
    // one ratio of the averages (which would give a different number here) - this pins that
    // DiffCollector nets already-transformed relative diffs, not raw baseline/candidate pairs.
    let records = [
        (1, rec("op1", Some(100.0), Some(110.0), None, None, None)), // +0.10
        (2, rec("op1", Some(1_000.0), Some(980.0), None, None, None)), // -0.02
    ];
    let out = relative_diff_compute(&records, true).unwrap();
    assert_eq!(out.paired_count, 1);
    assert!((out.effect - 0.04).abs() < 1e-9);

    // Confirm this really differs from "average baseline/candidate first, then one ratio":
    let ratio_of_averages = (1_080.0 / 1_100.0) - 1.0;
    assert!((out.effect - ratio_of_averages).abs() > 1e-3);
}

#[test]
fn relative_diff_paired_by_id_allows_duplicate_id_that_unpaired_mode_rejects() {
    let records = [
        (1, rec("dup", Some(10.0), Some(11.0), None, None, None)),
        (2, rec("dup", Some(20.0), Some(21.0), None, None, None)),
    ];
    let result = relative_diff_compute(&records, true);
    assert!(result.is_ok());
    let result = relative_diff_compute(&records, false);
    assert!(matches!(result, Err(VeridictError::DuplicateId { .. })));
}

#[test]
fn relative_diff_all_three_bootstrap_methods_run_without_error() {
    let records: Vec<(usize, Record)> = (0..20)
        .map(|i| {
            (
                i + 1,
                rec(
                    &format!("c{i}"),
                    Some(10.0 + i as f64),
                    Some(11.0 + i as f64 * 1.05),
                    None,
                    None,
                    None,
                ),
            )
        })
        .collect();
    for method in [
        BootstrapMethod::Percentile,
        BootstrapMethod::Basic,
        BootstrapMethod::Bca,
    ] {
        let out = compute(
            records.iter().cloned(),
            MetricConfig::RelativeDiff {
                bootstrap_method: method,
            },
            0.95,
            500,
            SEED,
            false,
            false,
        )
        .unwrap();
        assert!(out.ci_low.is_finite() && out.ci_high.is_finite());
    }
}

#[test]
fn paired_sign_test_rejects_triple_id() {
    let records = [
        (1, rec("op1", Some(1.0), Some(1.1), None, None, None)),
        (2, rec("op1", Some(1.0), Some(1.1), None, None, None)),
        (3, rec("op1", Some(1.0), Some(1.1), None, None, None)),
    ];
    let result = compute(
        records.iter().cloned(),
        MetricConfig::SignTest {
            ci_method: CiMethod::Wilson,
        },
        0.95,
        1000,
        SEED,
        true,
        false,
    );
    assert!(matches!(
        result,
        Err(VeridictError::SchemaMismatch {
            context: "paired-by-id",
            ..
        })
    ));
}

#[test]
fn paired_elo_nets_by_points_too() {
    let records = [
        (1, rec("op1", None, None, Some("candidate_win"), None, None)),
        (2, rec("op1", None, None, Some("draw"), None, None)),
    ];
    // total = 1.0 + 0.5 = 1.5 pts across the pair -> net candidate win.
    let out = compute(
        records.iter().cloned(),
        MetricConfig::Elo {
            failure_policy: FailurePolicy::ReportOnly,
        },
        0.95,
        1000,
        SEED,
        true,
        false,
    )
    .unwrap();
    assert_eq!(out.paired_count, 1);
    assert!(out.effect > 0.0);
}

// --- compute_many: single-scan multi-metric behavior ---

#[test]
fn compute_many_rejects_record_unusable_by_one_of_several_metrics() {
    // Usable by MeanDiff (has baseline/candidate) but not by WinRate (no result, no status).
    let records = [(1, rec("a", Some(1.0), Some(1.1), None, None, None))];
    let result = compute_many(
        records.iter().cloned(),
        &[
            MetricConfig::MeanDiff {
                bootstrap_method: BootstrapMethod::Percentile,
            },
            MetricConfig::WinRate {
                ci_method: CiMethod::Wilson,
                failure_policy: FailurePolicy::ReportOnly,
            },
        ],
        0.95,
        1000,
        SEED,
        false,
        false,
    );
    assert!(matches!(result, Err(VeridictError::SchemaMismatch { .. })));
}

#[test]
fn compute_many_matches_independent_compute_calls() {
    let records = [
        (
            1,
            rec("a", Some(1.0), Some(1.5), Some("candidate_win"), None, None),
        ),
        (
            2,
            rec("b", Some(2.0), Some(1.5), Some("baseline_win"), None, None),
        ),
        (3, rec("c", Some(1.0), Some(1.0), Some("draw"), None, None)),
    ];
    let combined = compute_many(
        records.iter().cloned(),
        &[
            MetricConfig::WinRate {
                ci_method: CiMethod::Wilson,
                failure_policy: FailurePolicy::ReportOnly,
            },
            MetricConfig::MeanDiff {
                bootstrap_method: BootstrapMethod::Percentile,
            },
            MetricConfig::SignTest {
                ci_method: CiMethod::Wilson,
            },
        ],
        0.95,
        1000,
        SEED,
        false,
        false,
    )
    .unwrap();
    let winrate = compute(
        records.iter().cloned(),
        MetricConfig::WinRate {
            ci_method: CiMethod::Wilson,
            failure_policy: FailurePolicy::ReportOnly,
        },
        0.95,
        1000,
        SEED,
        false,
        false,
    )
    .unwrap();
    let mean_diff = compute(
        records.iter().cloned(),
        MetricConfig::MeanDiff {
            bootstrap_method: BootstrapMethod::Percentile,
        },
        0.95,
        1000,
        SEED,
        false,
        false,
    )
    .unwrap();
    let sign_test = compute(
        records.iter().cloned(),
        MetricConfig::SignTest {
            ci_method: CiMethod::Wilson,
        },
        0.95,
        1000,
        SEED,
        false,
        false,
    )
    .unwrap();
    assert_eq!(combined[0].paired_count, winrate.paired_count);
    assert!((combined[0].effect - winrate.effect).abs() < 1e-12);
    assert_eq!(combined[1].paired_count, mean_diff.paired_count);
    assert!((combined[1].effect - mean_diff.effect).abs() < 1e-12);
    assert_eq!(combined[2].paired_count, sign_test.paired_count);
    assert!((combined[2].effect - sign_test.effect).abs() < 1e-12);
}
