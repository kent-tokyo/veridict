use super::*;
use crate::input::Record;
use crate::stats::bootstrap::DEFAULT_SEED;
use crate::verdict::Thresholds;

fn rec(
    id: &str,
    baseline: Option<f64>,
    candidate: Option<f64>,
    result: Option<&str>,
    baseline_status: Option<&str>,
    candidate_status: Option<&str>,
) -> Record {
    Record {
        id: Some(id.to_string()),
        baseline,
        candidate,
        result: result.map(str::to_string),
        baseline_status: baseline_status.map(str::to_string),
        candidate_status: candidate_status.map(str::to_string),
    }
}

#[test]
fn end_to_end_winrate_pass() {
    let mut records = Vec::new();
    for i in 0..80 {
        records.push((
            i + 1,
            rec(
                &format!("c{i}"),
                None,
                None,
                Some("candidate_win"),
                None,
                None,
            ),
        ));
    }
    for i in 0..20 {
        records.push((
            80 + i + 1,
            rec(
                &format!("b{i}"),
                None,
                None,
                Some("baseline_win"),
                None,
                None,
            ),
        ));
    }
    let thresholds = Thresholds::symmetric(0.02).unwrap();
    let report = compare_one(
        records.iter().cloned(),
        MetricConfig::WinRate {
            ci_method: CiMethod::Wilson,
            failure_policy: FailurePolicy::ReportOnly,
        },
        0.95,
        &thresholds,
        2000,
        DEFAULT_SEED,
        false,
        false,
    )
    .unwrap();
    assert_eq!(report.verdict, Verdict::Pass);
    assert_eq!(report.candidate_count, 80);
    assert_eq!(report.baseline_count, 20);
}

#[test]
fn end_to_end_mean_diff_inconclusive_on_tiny_sample() {
    let records = [
        (1, rec("a", Some(1.0), Some(1.1), None, None, None)),
        (2, rec("b", Some(2.0), Some(1.9), None, None, None)),
    ];
    let thresholds = Thresholds::symmetric(0.02).unwrap();
    let report = compare_one(
        records.iter().cloned(),
        MetricConfig::MeanDiff {
            bootstrap_method: BootstrapMethod::Percentile,
        },
        0.95,
        &thresholds,
        2000,
        DEFAULT_SEED,
        false,
        false,
    )
    .unwrap();
    assert_eq!(report.verdict, Verdict::Inconclusive);
}

#[test]
fn one_numeric_observation_cannot_pass_or_fail() {
    let thresholds = Thresholds::symmetric(0.01).unwrap();
    for candidate in [1.1, 0.9] {
        let records = [(1, rec("only", Some(1.0), Some(candidate), None, None, None))];
        let report = compare_one(
            records,
            MetricConfig::MeanDiff {
                bootstrap_method: BootstrapMethod::Percentile,
            },
            0.95,
            &thresholds,
            2000,
            DEFAULT_SEED,
            false,
            false,
        )
        .unwrap();
        assert_eq!(report.verdict, Verdict::Inconclusive);
        assert_eq!(report.promotion, Promotion::NotPromoted);
        assert_eq!(report.inconclusive_kind, None);
        assert!(report.reason.contains("at least 2"));
    }
}

#[test]
fn one_paired_elo_pair_cannot_pass() {
    let records = [
        (
            1,
            rec("pair", None, None, Some("candidate_win"), None, None),
        ),
        (
            2,
            rec("pair", None, None, Some("candidate_win"), None, None),
        ),
    ];
    let report = compare_one(
        records,
        MetricConfig::PairedElo {
            failure_policy: FailurePolicy::ReportOnly,
        },
        0.95,
        &Thresholds::symmetric(10.0).unwrap(),
        2000,
        DEFAULT_SEED,
        true,
        false,
    )
    .unwrap();
    assert_eq!(report.verdict, Verdict::Inconclusive);
    assert_eq!(report.paired_count, 1);
    assert!(report.reason.contains("independent pair"));
}

#[test]
fn one_cluster_cannot_produce_a_decisive_verdict() {
    let records: Vec<_> = (0..30)
        .map(|i| {
            (
                i + 1,
                rec("one-cluster", None, None, Some("candidate_win"), None, None),
            )
        })
        .collect();
    let report = compare_one(
        records,
        MetricConfig::Elo {
            failure_policy: FailurePolicy::ReportOnly,
        },
        0.95,
        &Thresholds::symmetric(10.0).unwrap(),
        200,
        DEFAULT_SEED,
        false,
        true,
    )
    .unwrap();
    assert_eq!(report.verdict, Verdict::Inconclusive);
    assert_eq!(report.cluster_count, Some(1));
    assert!(report.reason.contains("independent cluster"));
}

#[test]
fn zero_usable_trials_is_inconclusive_not_error() {
    let records = [(1, rec("a", None, None, None, Some("timeout"), None))];
    let thresholds = Thresholds::symmetric(0.02).unwrap();
    let report = compare_one(
        records.iter().cloned(),
        MetricConfig::WinRate {
            ci_method: CiMethod::Wilson,
            failure_policy: FailurePolicy::ReportOnly,
        },
        0.95,
        &thresholds,
        2000,
        DEFAULT_SEED,
        false,
        false,
    )
    .unwrap();
    assert_eq!(report.verdict, Verdict::Inconclusive);
    assert_eq!(report.timeouts, 1);
    // Zero usable trials is a data-availability problem, not a real CI judgment - ci_low ==
    // ci_high == 0.0 here would misleadingly read as `Noise` if this weren't guarded.
    assert_eq!(report.inconclusive_kind, None);
}

#[test]
fn empty_input_is_an_error() {
    let records: Vec<(usize, Record)> = Vec::new();
    let thresholds = Thresholds::symmetric(0.02).unwrap();
    let result = compare_one(
        records.iter().cloned(),
        MetricConfig::WinRate {
            ci_method: CiMethod::Wilson,
            failure_policy: FailurePolicy::ReportOnly,
        },
        0.95,
        &thresholds,
        2000,
        DEFAULT_SEED,
        false,
        false,
    );
    assert!(matches!(result, Err(VeridictError::EmptyInput)));
}

#[test]
fn compare_many_passes_overall_when_every_metric_passes() {
    let records: Vec<_> = (0..20)
        .map(|i| {
            (
                i + 1,
                rec(
                    &format!("r{i}"),
                    Some(1.0),
                    Some(2.0),
                    Some("candidate_win"),
                    None,
                    None,
                ),
            )
        })
        .collect();
    let thresholds = [
        Thresholds::symmetric(0.1).unwrap(),
        Thresholds::symmetric(0.1).unwrap(),
    ];
    let report = compare_many(
        records.iter().cloned(),
        &[
            MetricConfig::WinRate {
                ci_method: CiMethod::Wilson,
                failure_policy: FailurePolicy::ReportOnly,
            },
            MetricConfig::MeanDiff {
                bootstrap_method: BootstrapMethod::Percentile,
            },
        ],
        0.95,
        &thresholds,
        2000,
        DEFAULT_SEED,
        false,
        false,
    )
    .unwrap();
    assert_eq!(report.verdict, Verdict::Pass);
    assert_eq!(report.reports.len(), 2);
}

#[test]
fn compare_many_fails_overall_if_any_metric_fails() {
    // Each record carries both fields: result says the candidate always
    // loses (winrate -> Fail), but the numeric score always favors the
    // candidate (mean-diff -> Pass). Fail must dominate the aggregate.
    let records: Vec<_> = (0..20)
        .map(|i| {
            (
                i + 1,
                rec(
                    &format!("r{i}"),
                    Some(1.0),
                    Some(2.0),
                    Some("baseline_win"),
                    None,
                    None,
                ),
            )
        })
        .collect();
    let thresholds = [
        Thresholds::symmetric(0.1).unwrap(),
        Thresholds::symmetric(0.1).unwrap(),
    ];
    let report = compare_many(
        records.iter().cloned(),
        &[
            MetricConfig::WinRate {
                ci_method: CiMethod::Wilson,
                failure_policy: FailurePolicy::ReportOnly,
            },
            MetricConfig::MeanDiff {
                bootstrap_method: BootstrapMethod::Percentile,
            },
        ],
        0.95,
        &thresholds,
        2000,
        DEFAULT_SEED,
        false,
        false,
    )
    .unwrap();
    assert_eq!(report.reports[0].verdict, Verdict::Fail);
    assert_eq!(report.reports[1].verdict, Verdict::Pass);
    assert_eq!(report.verdict, Verdict::Fail);
}

#[test]
fn compare_many_rejects_an_empty_metric_family() {
    let records = [(1, rec("a", None, None, Some("candidate_win"), None, None))];
    let result = compare_many(records, &[], 0.95, &[], 100, DEFAULT_SEED, false, false);
    assert!(matches!(result, Err(VeridictError::InvalidThreshold(_))));
}

#[test]
fn compare_many_rejects_metric_threshold_count_mismatch() {
    let records = [(1, rec("a", None, None, Some("candidate_win"), None, None))];
    let metrics = [MetricConfig::WinRate {
        ci_method: CiMethod::Wilson,
        failure_policy: FailurePolicy::ReportOnly,
    }];
    let result = compare_many(
        records,
        &metrics,
        0.95,
        &[],
        100,
        DEFAULT_SEED,
        false,
        false,
    );
    assert!(matches!(result, Err(VeridictError::InvalidThreshold(_))));
}

// --- Report.warnings ---

#[test]
fn tiny_sample_produces_a_warning() {
    let records: Vec<_> = (0..10)
        .map(|i| {
            (
                i + 1,
                rec(
                    &format!("r{i}"),
                    None,
                    None,
                    Some("candidate_win"),
                    None,
                    None,
                ),
            )
        })
        .collect();
    let thresholds = Thresholds::symmetric(0.0).unwrap();
    let report = compare_one(
        records.iter().cloned(),
        MetricConfig::WinRate {
            ci_method: CiMethod::Wilson,
            failure_policy: FailurePolicy::ReportOnly,
        },
        0.95,
        &thresholds,
        2000,
        DEFAULT_SEED,
        false,
        false,
    )
    .unwrap();
    assert!(report.warnings.iter().any(|w| w.contains("small sample")));
    assert!(report.data_quality.tiny_sample);
    // A wide CI from a tiny sample shouldn't ALSO trip the noise-floor
    // flag - see DataQuality's doc comment.
    assert!(!report.data_quality.effect_within_noise_floor);
}

// --- low_id_diversity: the case table that actually matters here ---

fn winrate_records_with_ids(ids: &[String]) -> Vec<(usize, Record)> {
    ids.iter()
        .enumerate()
        .map(|(i, id)| {
            (
                i + 1,
                rec(id, None, None, Some("candidate_win"), None, None),
            )
        })
        .collect()
}

#[test]
fn id_diversity_healthy_mostly_unique_ids_is_silent() {
    let ids: Vec<String> = (0..12).map(|i| format!("r{i}")).collect();
    let records = winrate_records_with_ids(&ids);
    let thresholds = Thresholds::symmetric(0.0).unwrap();
    let report = compare_one(
        records.iter().cloned(),
        MetricConfig::WinRate {
            ci_method: CiMethod::Wilson,
            failure_policy: FailurePolicy::ReportOnly,
        },
        0.95,
        &thresholds,
        2000,
        DEFAULT_SEED,
        false,
        false,
    )
    .unwrap();
    assert!(!report.data_quality.low_id_diversity);
}

#[test]
fn id_diversity_every_id_exactly_twice_is_silent() {
    // The common, innocent mistake: genuinely paired data run without
    // --paired-by-id. Every id at exactly 2 must NOT fire - that would
    // be noise on a case this ordinary, not a real diversity problem.
    let mut ids = Vec::new();
    for i in 0..6 {
        ids.push(format!("pair{i}"));
        ids.push(format!("pair{i}"));
    }
    let records = winrate_records_with_ids(&ids);
    let thresholds = Thresholds::symmetric(0.0).unwrap();
    let report = compare_one(
        records.iter().cloned(),
        MetricConfig::WinRate {
            ci_method: CiMethod::Wilson,
            failure_policy: FailurePolicy::ReportOnly,
        },
        0.95,
        &thresholds,
        2000,
        DEFAULT_SEED,
        false,
        false,
    )
    .unwrap();
    assert!(!report.data_quality.low_id_diversity);
}

#[test]
fn id_diversity_one_dominant_id_fires() {
    let mut ids = vec!["dup".to_string(); 5];
    ids.extend((0..7).map(|i| format!("r{i}")));
    let records = winrate_records_with_ids(&ids);
    let thresholds = Thresholds::symmetric(0.0).unwrap();
    let report = compare_one(
        records.iter().cloned(),
        MetricConfig::WinRate {
            ci_method: CiMethod::Wilson,
            failure_policy: FailurePolicy::ReportOnly,
        },
        0.95,
        &thresholds,
        2000,
        DEFAULT_SEED,
        false,
        false,
    )
    .unwrap();
    assert!(report.data_quality.low_id_diversity);
    assert!(
        report
            .warnings
            .iter()
            .any(|w| w.contains("low id diversity"))
    );
}

#[test]
fn id_diversity_skipped_entirely_under_paired_by_id() {
    // Same shape as the exactly-twice case, but paired mode: repeated
    // ids mean something different there (net to one observation), so
    // this tracking must stay at its 0 sentinel, not fire.
    let mut ids = Vec::new();
    for i in 0..6 {
        ids.push(format!("pair{i}"));
        ids.push(format!("pair{i}"));
    }
    let records = winrate_records_with_ids(&ids);
    let thresholds = Thresholds::symmetric(0.0).unwrap();
    let report = compare_one(
        records.iter().cloned(),
        MetricConfig::WinRate {
            ci_method: CiMethod::Wilson,
            failure_policy: FailurePolicy::ReportOnly,
        },
        0.95,
        &thresholds,
        2000,
        DEFAULT_SEED,
        true,
        false,
    )
    .unwrap();
    assert!(!report.data_quality.low_id_diversity);
}

#[test]
fn id_diversity_below_the_floor_is_silent_even_if_skewed() {
    // Same skew as the dominant-id case (one id x4), but only 6
    // id-tagged records total - below the >= 10 floor, so the signal
    // isn't meaningful enough to report yet.
    let mut ids = vec!["dup".to_string(); 4];
    ids.extend((0..2).map(|i| format!("r{i}")));
    let records = winrate_records_with_ids(&ids);
    let thresholds = Thresholds::symmetric(0.0).unwrap();
    let report = compare_one(
        records.iter().cloned(),
        MetricConfig::WinRate {
            ci_method: CiMethod::Wilson,
            failure_policy: FailurePolicy::ReportOnly,
        },
        0.95,
        &thresholds,
        2000,
        DEFAULT_SEED,
        false,
        false,
    )
    .unwrap();
    assert!(!report.data_quality.low_id_diversity);
}

#[test]
fn excessive_failures_produce_a_warning() {
    let mut records: Vec<_> = (0..30)
        .map(|i| {
            (
                i + 1,
                rec(
                    &format!("r{i}"),
                    None,
                    None,
                    Some("candidate_win"),
                    None,
                    None,
                ),
            )
        })
        .collect();
    for i in 0..8 {
        records.push((
            31 + i,
            rec(&format!("t{i}"), None, None, None, Some("timeout"), None),
        ));
    }
    let thresholds = Thresholds::symmetric(0.0).unwrap();
    let report = compare_one(
        records.iter().cloned(),
        MetricConfig::WinRate {
            ci_method: CiMethod::Wilson,
            failure_policy: FailurePolicy::ReportOnly,
        },
        0.95,
        &thresholds,
        2000,
        DEFAULT_SEED,
        false,
        false,
    )
    .unwrap();
    assert_eq!(report.paired_count, 30);
    assert!(!report.warnings.iter().any(|w| w.contains("small sample")));
    assert!(
        report
            .warnings
            .iter()
            .any(|w| w.contains("failed to execute"))
    );
}

#[test]
fn excessive_draws_produce_a_warning_for_elo() {
    let mut records = Vec::new();
    for i in 0..3 {
        records.push((
            i + 1,
            rec(
                &format!("c{i}"),
                None,
                None,
                Some("candidate_win"),
                None,
                None,
            ),
        ));
    }
    for i in 0..2 {
        records.push((
            4 + i,
            rec(
                &format!("b{i}"),
                None,
                None,
                Some("baseline_win"),
                None,
                None,
            ),
        ));
    }
    for i in 0..6 {
        records.push((
            6 + i,
            rec(&format!("d{i}"), None, None, Some("draw"), None, None),
        ));
    }
    let thresholds = Thresholds::symmetric(0.0).unwrap();
    let report = compare_one(
        records.iter().cloned(),
        MetricConfig::Elo {
            failure_policy: FailurePolicy::ReportOnly,
        },
        0.95,
        &thresholds,
        2000,
        DEFAULT_SEED,
        false,
        false,
    )
    .unwrap();
    assert!(report.warnings.iter().any(|w| w.contains("draws")));
}

#[test]
fn clean_large_sample_has_no_warnings() {
    let records: Vec<_> = (0..40)
        .map(|i| {
            (
                i + 1,
                rec(
                    &format!("r{i}"),
                    None,
                    None,
                    Some("candidate_win"),
                    None,
                    None,
                ),
            )
        })
        .collect();
    let thresholds = Thresholds::symmetric(0.0).unwrap();
    let report = compare_one(
        records.iter().cloned(),
        MetricConfig::WinRate {
            ci_method: CiMethod::Wilson,
            failure_policy: FailurePolicy::ReportOnly,
        },
        0.95,
        &thresholds,
        2000,
        DEFAULT_SEED,
        false,
        false,
    )
    .unwrap();
    assert!(report.warnings.is_empty());
    assert_eq!(report.data_quality, report::DataQuality::default());
}

#[test]
fn noise_floor_flag_fires_on_a_large_but_swamped_effect() {
    // n=40 (not tiny), but a near-50/50 split leaves the effect (0.025)
    // far smaller than the CI's own half-width (~0.148) - independently
    // verified against a direct Wilson recompute.
    let mut records: Vec<_> = (0..21)
        .map(|i| {
            (
                i + 1,
                rec(
                    &format!("c{i}"),
                    None,
                    None,
                    Some("candidate_win"),
                    None,
                    None,
                ),
            )
        })
        .collect();
    records.extend((0..19).map(|i| {
        (
            22 + i,
            rec(
                &format!("b{i}"),
                None,
                None,
                Some("baseline_win"),
                None,
                None,
            ),
        )
    }));
    let thresholds = Thresholds::symmetric(0.0).unwrap();
    let report = compare_one(
        records.iter().cloned(),
        MetricConfig::WinRate {
            ci_method: CiMethod::Wilson,
            failure_policy: FailurePolicy::ReportOnly,
        },
        0.95,
        &thresholds,
        2000,
        DEFAULT_SEED,
        false,
        false,
    )
    .unwrap();
    assert!(!report.data_quality.tiny_sample);
    assert!(report.data_quality.effect_within_noise_floor);
    assert!(
        report
            .warnings
            .iter()
            .any(|w| w.contains("smaller than the CI's own half-width"))
    );
}

// --- wide_baseline_scale / scale_diagnostics ---

fn scale_records(pairs: &[(f64, f64)]) -> Vec<(usize, Record)> {
    pairs
        .iter()
        .enumerate()
        .map(|(i, &(b, c))| {
            (
                i + 1,
                rec(&format!("s{i}"), Some(b), Some(c), None, None, None),
            )
        })
        .collect()
}

#[test]
fn wide_baseline_scale_silent_below_ten_x_spread() {
    let records = scale_records(&[
        (10.0, 10.5),
        (20.0, 21.0),
        (30.0, 31.5),
        (40.0, 42.0),
        (50.0, 52.0),
    ]);
    let thresholds = Thresholds::symmetric(0.0).unwrap();
    let report = compare_one(
        records.iter().cloned(),
        MetricConfig::MeanDiff {
            bootstrap_method: BootstrapMethod::Percentile,
        },
        0.95,
        &thresholds,
        500,
        DEFAULT_SEED,
        false,
        false,
    )
    .unwrap();
    assert!(!report.data_quality.wide_baseline_scale);
    let diag = report
        .scale_diagnostics
        .expect("mean-diff always carries scale_diagnostics for nonempty data");
    assert!(diag.raw_orders_of_magnitude < report::WIDE_BASELINE_SCALE_ORDERS);
}

#[test]
fn wide_baseline_scale_fires_at_or_above_ten_x_spread() {
    let records = scale_records(&[
        (10.0, 10.5),
        (40.0, 42.0),
        (70.0, 73.0),
        (100.0, 105.0),
        (150.0, 157.0),
    ]);
    let thresholds = Thresholds::symmetric(0.0).unwrap();
    let report = compare_one(
        records.iter().cloned(),
        MetricConfig::MeanDiff {
            bootstrap_method: BootstrapMethod::Percentile,
        },
        0.95,
        &thresholds,
        500,
        DEFAULT_SEED,
        false,
        false,
    )
    .unwrap();
    assert!(report.data_quality.wide_baseline_scale);
    assert!(
        report
            .warnings
            .iter()
            .any(|w| w.contains("orders of magnitude") && w.contains("--metric relative-diff"))
    );
}

#[test]
fn wide_baseline_scale_uses_robust_span_ignoring_a_single_outlier_at_n40() {
    // Same construction verified directly against `compute_scale_diagnostics` in
    // `metrics::common`'s own tests - this confirms the same property survives the full
    // `compare_one` -> `collect_data_quality` pipeline, not just the underlying function.
    let mut pairs: Vec<(f64, f64)> = (0..39)
        .map(|i| {
            let b = 100.0 + i as f64;
            (b, b * 1.05)
        })
        .collect();
    pairs.push((10_000_000.0, 10_500_000.0));
    let records = scale_records(&pairs);
    let thresholds = Thresholds::symmetric(0.0).unwrap();
    let report = compare_one(
        records.iter().cloned(),
        MetricConfig::MeanDiff {
            bootstrap_method: BootstrapMethod::Percentile,
        },
        0.95,
        &thresholds,
        500,
        DEFAULT_SEED,
        false,
        false,
    )
    .unwrap();
    let diag = report.scale_diagnostics.unwrap();
    assert_eq!(diag.positive_baseline_count, 40);
    assert!(diag.raw_orders_of_magnitude > 4.0);
    assert!(diag.robust_orders_of_magnitude.unwrap() < 1.0);
    assert!(
        !report.data_quality.wide_baseline_scale,
        "a single outlier at n=40 should not trip the robust-span-gated warning"
    );
}

#[test]
fn wide_baseline_scale_with_a_non_positive_baseline_recommends_normalization_not_relative_diff() {
    let records = scale_records(&[(-5.0, -4.0), (10.0, 10.5), (5000.0, 5250.0)]);
    let thresholds = Thresholds::symmetric(0.0).unwrap();
    let report = compare_one(
        records.iter().cloned(),
        MetricConfig::MeanDiff {
            bootstrap_method: BootstrapMethod::Percentile,
        },
        0.95,
        &thresholds,
        500,
        DEFAULT_SEED,
        false,
        false,
    )
    .unwrap();
    assert!(report.data_quality.wide_baseline_scale);
    let diag = report.scale_diagnostics.unwrap();
    assert_eq!(diag.non_positive_baseline_count, 1);
    assert!(
        report
            .warnings
            .iter()
            .any(|w| w.contains("zero or negative")
                && w.contains("not well-defined")
                && !w.contains("--metric relative-diff"))
    );
}

#[test]
fn wide_baseline_scale_diagnostics_do_not_depend_on_candidate_effect_or_verdict() {
    let baselines = [10.0, 40.0, 70.0, 100.0, 150.0];
    let passing = scale_records(&baselines.iter().map(|&b| (b, b * 1.5)).collect::<Vec<_>>());
    let failing = scale_records(&baselines.iter().map(|&b| (b, b * 0.5)).collect::<Vec<_>>());
    let thresholds = Thresholds::symmetric(0.01).unwrap();
    let config = MetricConfig::MeanDiff {
        bootstrap_method: BootstrapMethod::Percentile,
    };
    let report_pass = compare_one(
        passing.iter().cloned(),
        config,
        0.95,
        &thresholds,
        500,
        DEFAULT_SEED,
        false,
        false,
    )
    .unwrap();
    let report_fail = compare_one(
        failing.iter().cloned(),
        config,
        0.95,
        &thresholds,
        500,
        DEFAULT_SEED,
        false,
        false,
    )
    .unwrap();
    // Sanity: these two runs really do land on different verdicts/effects - the point of this
    // test is that the *diagnostic* doesn't move even though the *result* clearly does.
    assert_ne!(report_pass.verdict, report_fail.verdict);
    assert!((report_pass.effect - report_fail.effect).abs() > 1.0);

    let diag_pass = report_pass.scale_diagnostics.unwrap();
    let diag_fail = report_fail.scale_diagnostics.unwrap();
    assert_eq!(
        diag_pass.raw_orders_of_magnitude,
        diag_fail.raw_orders_of_magnitude
    );
    assert_eq!(
        diag_pass.robust_orders_of_magnitude,
        diag_fail.robust_orders_of_magnitude
    );
    assert_eq!(
        report_pass.data_quality.wide_baseline_scale,
        report_fail.data_quality.wide_baseline_scale
    );
}

#[test]
fn relative_diff_never_sets_wide_baseline_scale_even_with_wide_positive_scale() {
    let records = scale_records(&[
        (10.0, 10.5),
        (40.0, 42.0),
        (70.0, 73.0),
        (100.0, 105.0),
        (150.0, 157.0),
    ]);
    let thresholds = Thresholds::symmetric(0.0).unwrap();
    let report = compare_one(
        records.iter().cloned(),
        MetricConfig::RelativeDiff {
            bootstrap_method: BootstrapMethod::Percentile,
        },
        0.95,
        &thresholds,
        500,
        DEFAULT_SEED,
        false,
        false,
    )
    .unwrap();
    assert!(!report.data_quality.wide_baseline_scale);
    let diag = report
        .scale_diagnostics
        .expect("relative-diff also carries scale_diagnostics, for transparency");
    // Same wide baselines as `wide_baseline_scale_fires_at_or_above_ten_x_spread` above -
    // proves the bool's silence here is `relative-diff`-specific, not because the scale
    // happened to look narrow this time.
    assert!(diag.raw_orders_of_magnitude >= report::WIDE_BASELINE_SCALE_ORDERS);
}

#[test]
fn inconclusive_kind_is_none_on_a_pass_verdict() {
    let records = [
        (1, rec("a", Some(1.0), Some(2.0), None, None, None)),
        (2, rec("b", Some(1.0), Some(2.0), None, None, None)),
    ];
    let thresholds = Thresholds::symmetric(0.5).unwrap();
    let report = compare_one(
        records.iter().cloned(),
        MetricConfig::MeanDiff {
            bootstrap_method: BootstrapMethod::Percentile,
        },
        0.95,
        &thresholds,
        500,
        DEFAULT_SEED,
        false,
        false,
    )
    .unwrap();
    assert_eq!(report.verdict, Verdict::Pass);
    assert_eq!(report.inconclusive_kind, None);
}

#[test]
fn inconclusive_kind_is_directional_when_ci_excludes_zero_but_stays_in_the_dead_zone() {
    // Every diff is identical (+0.5): resampling an identical value can't move the mean, so
    // the bootstrap CI collapses to a single point at 0.5 - deterministically excludes zero,
    // deterministically inside a +-5.0 dead zone.
    let records: Vec<_> = (0..30)
        .map(|i| {
            (
                i + 1,
                rec(&format!("r{i}"), Some(1.0), Some(1.5), None, None, None),
            )
        })
        .collect();
    let thresholds = Thresholds::symmetric(5.0).unwrap();
    let report = compare_one(
        records.iter().cloned(),
        MetricConfig::MeanDiff {
            bootstrap_method: BootstrapMethod::Percentile,
        },
        0.95,
        &thresholds,
        500,
        DEFAULT_SEED,
        false,
        false,
    )
    .unwrap();
    assert_eq!(report.verdict, Verdict::Inconclusive);
    assert_eq!(
        report.inconclusive_kind,
        Some(InconclusiveKind::Directional)
    );
    // Directly ties into the dead-zone estimated_additional_trials contract documented above.
    assert_eq!(report.estimated_additional_trials, None);
}

#[test]
fn inconclusive_kind_is_noise_when_ci_straddles_zero() {
    // Diffs alternate +0.1/-0.1: effect is exactly 0.0, and the bootstrap CI around a
    // two-point-mass sample straddles zero rather than collapsing to it.
    let records: Vec<_> = (0..40)
        .map(|i| {
            let diff = if i % 2 == 0 { 0.1 } else { -0.1 };
            (
                i + 1,
                rec(
                    &format!("r{i}"),
                    Some(1.0),
                    Some(1.0 + diff),
                    None,
                    None,
                    None,
                ),
            )
        })
        .collect();
    let thresholds = Thresholds::symmetric(5.0).unwrap();
    let report = compare_one(
        records.iter().cloned(),
        MetricConfig::MeanDiff {
            bootstrap_method: BootstrapMethod::Percentile,
        },
        0.95,
        &thresholds,
        500,
        DEFAULT_SEED,
        false,
        false,
    )
    .unwrap();
    assert_eq!(report.verdict, Verdict::Inconclusive);
    assert_eq!(report.inconclusive_kind, Some(InconclusiveKind::Noise));
}

#[test]
fn relative_diff_tied_count_and_dilution_flag_fire_when_most_records_are_exact_matches() {
    // 8 of 10 records are exact matches (candidate == baseline); only 2 carry a real change.
    let mut records = vec![
        (
            1,
            rec("touched-a", Some(10.0), Some(11.0), None, None, None),
        ),
        (
            2,
            rec("touched-b", Some(10.0), Some(11.0), None, None, None),
        ),
    ];
    for i in 0..8 {
        records.push((
            3 + i,
            rec(
                &format!("untouched{i}"),
                Some(10.0),
                Some(10.0),
                None,
                None,
                None,
            ),
        ));
    }
    let thresholds = Thresholds::symmetric(0.0).unwrap();
    let report = compare_one(
        records.iter().cloned(),
        MetricConfig::RelativeDiff {
            bootstrap_method: BootstrapMethod::Percentile,
        },
        0.95,
        &thresholds,
        500,
        DEFAULT_SEED,
        false,
        false,
    )
    .unwrap();
    assert_eq!(report.tied_count, Some(8));
    assert!(report.data_quality.diluted_by_ties);
    assert!(report.warnings.iter().any(|w| w.contains("no change")));
}

#[test]
fn relative_diff_tied_count_is_zero_and_dilution_flag_silent_with_no_ties() {
    let records: Vec<_> = (0..10)
        .map(|i| {
            (
                i + 1,
                rec(&format!("r{i}"), Some(10.0), Some(11.0), None, None, None),
            )
        })
        .collect();
    let thresholds = Thresholds::symmetric(0.0).unwrap();
    let report = compare_one(
        records.iter().cloned(),
        MetricConfig::RelativeDiff {
            bootstrap_method: BootstrapMethod::Percentile,
        },
        0.95,
        &thresholds,
        500,
        DEFAULT_SEED,
        false,
        false,
    )
    .unwrap();
    assert_eq!(report.tied_count, Some(0));
    assert!(!report.data_quality.diluted_by_ties);
}

#[test]
fn relative_diff_tied_count_under_paired_by_id_counts_at_ingest_not_post_netting() {
    // "pair" nets two *opposite* nonzero ratios (+10%/-10%) to a diff of exactly 0.0 - neither
    // record was an exact match, so it must not count toward tied_count. "tied" nets two
    // genuine exact matches - both must count.
    let records = [
        (1, rec("pair", Some(100.0), Some(110.0), None, None, None)),
        (2, rec("pair", Some(100.0), Some(90.0), None, None, None)),
        (3, rec("tied", Some(100.0), Some(100.0), None, None, None)),
        (4, rec("tied", Some(100.0), Some(100.0), None, None, None)),
    ];
    let thresholds = Thresholds::symmetric(0.0).unwrap();
    let report = compare_one(
        records.iter().cloned(),
        MetricConfig::RelativeDiff {
            bootstrap_method: BootstrapMethod::Percentile,
        },
        0.95,
        &thresholds,
        500,
        DEFAULT_SEED,
        true,
        false,
    )
    .unwrap();
    assert_eq!(
        report.tied_count,
        Some(2),
        "only the two literal exact-match records count, not the netted-to-zero pair"
    );
    assert_eq!(
        report.baseline_count, 2,
        "post-netting: 2 groups -> 2 diffs"
    );
    // tied_count(2)/baseline_count(2) would naively read as a 100% tied rate here, double the
    // true 50% pre-netting rate - diluted_by_ties must stay silent under --paired-by-id rather
    // than report that inflated number.
    assert!(!report.data_quality.diluted_by_ties);
}

#[test]
fn tied_count_is_none_for_metrics_other_than_relative_diff() {
    let records = [
        (1, rec("a", Some(1.0), Some(1.5), None, None, None)),
        (2, rec("b", Some(1.0), Some(1.5), None, None, None)),
    ];
    let thresholds = Thresholds::symmetric(0.0).unwrap();
    let report = compare_one(
        records.iter().cloned(),
        MetricConfig::MeanDiff {
            bootstrap_method: BootstrapMethod::Percentile,
        },
        0.95,
        &thresholds,
        500,
        DEFAULT_SEED,
        false,
        false,
    )
    .unwrap();
    assert_eq!(report.tied_count, None);
    assert!(!report.data_quality.diluted_by_ties);
}
