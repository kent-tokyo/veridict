use super::*;

fn ok_iter(
    records: &[(usize, Record)],
) -> impl Iterator<Item = Result<(usize, Record), VeridictError>> + '_ {
    records.iter().cloned().map(Ok)
}

fn rec(result: Option<&str>) -> Record {
    rec_with_id(None, result)
}

fn rec_with_id(id: Option<&str>, result: Option<&str>) -> Record {
    Record {
        id: id.map(str::to_string),
        baseline: None,
        candidate: None,
        result: result.map(str::to_string),
        baseline_status: None,
        candidate_status: None,
    }
}

fn records_with_ids(ids: &[String], result: &str) -> Vec<(usize, Record)> {
    ids.iter()
        .enumerate()
        .map(|(index, id)| (index + 1, rec_with_id(Some(id.as_str()), Some(result))))
        .collect()
}

#[test]
fn run_options_defaults_are_explicit() {
    assert_eq!(
        SprtRunOptions::default(),
        SprtRunOptions {
            variant: SprtVariant::Wald,
            paired_by_id: false,
            failure_policy: FailurePolicy::ReportOnly,
            require_complete_pairs: false,
            min_paired_ids: None,
            max_paired_ids: None,
        }
    );
    assert_eq!(
        SprtRunOptions::new(SprtVariant::Trinomial).variant,
        SprtVariant::Trinomial
    );
}

#[test]
fn sprt_report_uses_its_own_v2_schema_version() {
    let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();
    let records = [(1, rec(Some("draw")))];
    let report = run_with_options(ok_iter(&records), &config, SprtRunOptions::default()).unwrap();
    assert_eq!(report.schema_version, SPRT_REPORT_SCHEMA_VERSION);
    assert_eq!(report.schema_version, 2);
}

fn assert_incremental_matches_batch(
    records: &[(usize, Record)],
    config: SprtConfig,
    options: SprtRunOptions,
) {
    let batch = run_with_options(ok_iter(records), &config, options).unwrap();

    let mut incremental = SprtAccumulator::new(config, options).unwrap();
    for (line, record) in records.iter().cloned() {
        incremental.push(line, record).unwrap();
    }
    assert_eq!(
        incremental.finish().unwrap().to_json_pretty(),
        batch.to_json_pretty()
    );

    let mut cut_points = vec![1, records.len() / 2, records.len() - 1];
    cut_points.sort_unstable();
    cut_points.dedup();
    for cut in cut_points {
        let mut before_restart = SprtAccumulator::new(config, options).unwrap();
        for (line, record) in records[..cut].iter().cloned() {
            before_restart.push(line, record).unwrap();
        }
        let expected_position = before_restart.position();
        let encoded = serde_json::to_string(&before_restart.snapshot()).unwrap();
        let checkpoint: SprtCheckpoint = serde_json::from_str(&encoded).unwrap();
        let mut resumed =
            SprtAccumulator::from_checkpoint(checkpoint, config, options, expected_position)
                .unwrap();
        for (line, record) in records[cut..].iter().cloned() {
            resumed.push(line, record).unwrap();
        }
        assert_eq!(
            resumed.finish().unwrap().to_json_pretty(),
            batch.to_json_pretty()
        );
    }
}

#[test]
fn incremental_and_resumed_reports_match_batch_for_every_variant() {
    let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();

    // This stream crosses the upper bound and later reverses in its full-input aggregate.
    // Exact report equality therefore also checks preservation of the first crossing.
    let wald: Vec<_> = (0..240)
        .map(|index| {
            let outcome = if index < 120 {
                "candidate_win"
            } else {
                "baseline_win"
            };
            (index + 1, rec(Some(outcome)))
        })
        .collect();
    assert_incremental_matches_batch(&wald, config, SprtRunOptions::default());

    let trinomial: Vec<_> = (0..240)
        .map(|index| {
            let outcome = match index % 4 {
                0 | 1 => "candidate_win",
                2 => "draw",
                _ => "baseline_win",
            };
            (index + 1, rec(Some(outcome)))
        })
        .collect();
    assert_incremental_matches_batch(
        &trinomial,
        config,
        SprtRunOptions::new(SprtVariant::Trinomial),
    );

    let pentanomial = pentanomial_records(120, ("candidate_win", "draw"));
    assert_incremental_matches_batch(
        &pentanomial,
        config,
        SprtRunOptions {
            variant: SprtVariant::Pentanomial,
            paired_by_id: true,
            ..SprtRunOptions::default()
        },
    );
}

#[test]
fn incremental_failure_policy_matches_batch() {
    let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();
    let options = SprtRunOptions {
        failure_policy: FailurePolicy::Loss,
        ..SprtRunOptions::default()
    };
    let records = [
        (
            1,
            Record {
                baseline_status: Some("ok".to_string()),
                candidate_status: Some("timeout".to_string()),
                ..rec(None)
            },
        ),
        (2, rec(Some("candidate_win"))),
    ];
    assert_incremental_matches_batch(&records, config, options);
}

#[test]
fn resumed_pair_errors_match_batch_errors() {
    let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();
    let options = SprtRunOptions {
        variant: SprtVariant::Pentanomial,
        paired_by_id: true,
        ..SprtRunOptions::default()
    };
    for records in [
        vec![(1, rec_with_id(Some("incomplete"), Some("candidate_win")))],
        vec![
            (1, rec_with_id(Some("triple"), Some("candidate_win"))),
            (2, rec_with_id(Some("triple"), Some("draw"))),
            (3, rec_with_id(Some("triple"), Some("baseline_win"))),
        ],
    ] {
        let batch_error = run_with_options(ok_iter(&records), &config, options)
            .unwrap_err()
            .to_string();
        let mut accumulator = SprtAccumulator::new(config, options).unwrap();
        for (line, record) in records {
            accumulator.push(line, record).unwrap();
        }
        assert_eq!(accumulator.finish().unwrap_err().to_string(), batch_error);
    }
}

#[test]
fn resume_rejects_context_or_prefix_mismatch() {
    let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();
    let options = SprtRunOptions::default();
    let mut accumulator = SprtAccumulator::new(config, options).unwrap();
    accumulator.push(10, rec(Some("candidate_win"))).unwrap();
    let checkpoint = accumulator.snapshot();
    let position = checkpoint.position();

    let other_config = SprtConfig::new(-1.0, 10.0, 0.05, 0.05).unwrap();
    assert!(matches!(
        SprtAccumulator::from_checkpoint(checkpoint.clone(), other_config, options, position),
        Err(VeridictError::SprtCheckpointMismatch { .. })
    ));
    assert!(matches!(
        SprtAccumulator::from_checkpoint(
            checkpoint.clone(),
            config,
            SprtRunOptions::new(SprtVariant::Trinomial),
            position
        ),
        Err(VeridictError::SprtCheckpointMismatch { .. })
    ));
    assert!(matches!(
        SprtAccumulator::from_checkpoint(
            checkpoint.clone(),
            config,
            options,
            SprtStreamPosition {
                prefix_hash: position.prefix_hash ^ 1,
                ..position
            }
        ),
        Err(VeridictError::SprtCheckpointMismatch { .. })
    ));

    let mut unsupported = checkpoint;
    unsupported.checkpoint_version += 1;
    assert!(matches!(
        SprtAccumulator::from_checkpoint(unsupported, config, options, position),
        Err(VeridictError::UnsupportedSprtCheckpointVersion { .. })
    ));
}

#[test]
fn incremental_push_requires_strict_stream_order() {
    let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();
    let mut accumulator = SprtAccumulator::new(config, SprtRunOptions::default()).unwrap();
    accumulator.push(7, rec(Some("draw"))).unwrap();
    assert!(matches!(
        accumulator.push(7, rec(Some("draw"))),
        Err(VeridictError::NonMonotonicSprtLine {
            line: 7,
            previous: 7
        })
    ));
}

#[test]
fn rejected_incremental_record_does_not_mutate_checkpoint() {
    let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();
    let mut accumulator = SprtAccumulator::new(config, SprtRunOptions::default()).unwrap();
    accumulator.push(1, rec(Some("candidate_win"))).unwrap();
    let before = serde_json::to_string(&accumulator.snapshot()).unwrap();
    assert!(accumulator.push(2, rec(Some("unknown"))).is_err());
    assert_eq!(
        serde_json::to_string(&accumulator.snapshot()).unwrap(),
        before
    );
}

#[test]
fn batch_api_keeps_iterator_order_when_line_metadata_is_not_monotonic() {
    let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();
    let records = [
        (20, rec(Some("candidate_win"))),
        (10, rec(Some("baseline_win"))),
    ];
    assert!(run_with_options(ok_iter(&records), &config, SprtRunOptions::default()).is_ok());
}

#[test]
fn low_id_diversity_is_structural_and_does_not_change_the_decision() {
    let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();
    let mut ids = vec!["dup".to_string(); 5];
    ids.extend((0..7).map(|i| format!("unique-{i}")));

    let candidate_records = records_with_ids(&ids, "candidate_win");
    let baseline_records = records_with_ids(&ids, "baseline_win");
    let candidate = run_with_options(
        ok_iter(&candidate_records),
        &config,
        SprtRunOptions::default(),
    )
    .unwrap();
    let baseline = run_with_options(
        ok_iter(&baseline_records),
        &config,
        SprtRunOptions::default(),
    )
    .unwrap();

    assert!(candidate.data_quality.low_id_diversity);
    assert!(baseline.data_quality.low_id_diversity);
    assert_eq!(candidate.warnings, baseline.warnings);
    assert_ne!(candidate.decision_llr, baseline.decision_llr);

    let idless_records: Vec<_> = candidate_records
        .iter()
        .cloned()
        .map(|(line, mut record)| {
            record.id = None;
            (line, record)
        })
        .collect();
    let idless =
        run_with_options(ok_iter(&idless_records), &config, SprtRunOptions::default()).unwrap();
    assert_eq!(candidate.verdict, idless.verdict);
    assert_eq!(candidate.llr, idless.llr);
    assert_eq!(candidate.decision_llr, idless.decision_llr);
    assert!(!idless.data_quality.low_id_diversity);
}

#[test]
fn exactly_two_records_per_id_and_paired_mode_are_silent() {
    let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();
    let ids: Vec<_> = (0..6)
        .flat_map(|i| [format!("pair-{i}"), format!("pair-{i}")])
        .collect();
    let records = records_with_ids(&ids, "candidate_win");

    let unpaired = run_with_options(ok_iter(&records), &config, SprtRunOptions::default()).unwrap();
    assert!(!unpaired.data_quality.low_id_diversity);
    assert!(unpaired.warnings.is_empty());
    assert!(!unpaired.to_markdown().contains("Warnings:"));

    let paired = run_with_options(
        ok_iter(&records),
        &config,
        SprtRunOptions {
            paired_by_id: true,
            ..SprtRunOptions::default()
        },
    )
    .unwrap();
    assert!(!paired.data_quality.low_id_diversity);
    assert!(paired.warnings.is_empty());
    assert!(!paired.to_markdown().contains("Warnings:"));
}

#[test]
fn low_id_diversity_is_rendered_in_markdown() {
    let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();
    let mut ids = vec!["dup".to_string(); 5];
    ids.extend((0..7).map(|i| format!("unique-{i}")));
    let records = records_with_ids(&ids, "candidate_win");
    let report = run_with_options(ok_iter(&records), &config, SprtRunOptions::default()).unwrap();
    let markdown = report.to_markdown();

    assert!(markdown.contains("Warnings:\n"));
    assert!(markdown.contains("low id diversity"));
}

#[test]
fn typed_options_match_the_positional_compatibility_wrapper() {
    let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();
    let records: Vec<_> = (0..200)
        .map(|i| (i + 1, rec(Some("candidate_win"))))
        .collect();
    let positional = run(
        ok_iter(&records),
        &config,
        SprtVariant::Wald,
        false,
        FailurePolicy::ReportOnly,
        false,
        None,
        None,
    )
    .unwrap();
    let typed = run_with_options(ok_iter(&records), &config, SprtRunOptions::default()).unwrap();

    assert_eq!(typed.to_json_pretty(), positional.to_json_pretty());
}

#[test]
fn typed_options_centrally_validate_wald_trinomial_and_pentanomial() {
    let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();
    let records = [(1, rec_with_id(Some("pair-1"), Some("candidate_win")))];

    for variant in [SprtVariant::Wald, SprtVariant::Trinomial] {
        let error = run_with_options(
            ok_iter(&records),
            &config,
            SprtRunOptions {
                variant,
                min_paired_ids: Some(1),
                ..SprtRunOptions::default()
            },
        )
        .unwrap_err();
        assert!(matches!(error, VeridictError::InvalidThreshold(_)));
    }

    let error = run_with_options(
        ok_iter(&records),
        &config,
        SprtRunOptions::new(SprtVariant::Pentanomial),
    )
    .unwrap_err();
    assert!(matches!(error, VeridictError::InvalidThreshold(_)));
}

#[test]
fn clear_h1_stream_passes() {
    let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();
    let records: Vec<_> = (0..2000)
        .map(|i| (i + 1, rec(Some("candidate_win"))))
        .collect();
    let report = run(
        ok_iter(&records),
        &config,
        SprtVariant::Wald,
        false,
        FailurePolicy::ReportOnly,
        false,
        None,
        None,
    )
    .unwrap();
    assert_eq!(report.verdict, Verdict::Pass);
    assert!(report.llr >= report.upper_bound);
    assert_eq!(report.drawelo, None);
}

#[test]
fn trinomial_clear_h1_stream_passes_and_reports_drawelo() {
    let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();
    let records: Vec<_> = (0..2000)
        .map(|i| (i + 1, rec(Some("candidate_win"))))
        .collect();
    let report = run(
        ok_iter(&records),
        &config,
        SprtVariant::Trinomial,
        false,
        FailurePolicy::ReportOnly,
        false,
        None,
        None,
    )
    .unwrap();
    assert_eq!(report.verdict, Verdict::Pass);
    assert!(report.llr >= report.upper_bound);
    assert!(report.drawelo.is_some());
}

#[test]
fn trinomial_draw_heavy_stream_still_reaches_a_verdict() {
    // A draw-heavy but clearly candidate-favored stream - the scenario
    // this variant exists for. Not asserting a specific verdict (that
    // depends on the exact mix), just that it computes a finite report
    // rather than getting stuck on the draw-rate estimation.
    let mut records = Vec::new();
    for i in 0..300 {
        records.push((i * 2 + 1, rec(Some("draw"))));
        records.push((i * 2 + 2, rec(Some("candidate_win"))));
    }
    let config = SprtConfig::new(0.0, 30.0, 0.05, 0.05).unwrap();
    let report = run(
        ok_iter(&records),
        &config,
        SprtVariant::Trinomial,
        false,
        FailurePolicy::ReportOnly,
        false,
        None,
        None,
    )
    .unwrap();
    assert!(report.llr.is_finite());
    assert!(report.drawelo.unwrap().is_finite());
}

#[test]
fn trinomial_zero_draws_matches_wald_verdict() {
    // Integration-level companion to stats::trinomial_sprt's exact
    // pure-math reduction test: with no draws in the actual record
    // stream, both variants should reach the same verdict end to end.
    let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();
    let records: Vec<_> = (0..1000)
        .map(|i| {
            (
                i + 1,
                rec(Some(if i % 7 == 0 {
                    "baseline_win"
                } else {
                    "candidate_win"
                })),
            )
        })
        .collect();
    let wald = run(
        ok_iter(&records),
        &config,
        SprtVariant::Wald,
        false,
        FailurePolicy::ReportOnly,
        false,
        None,
        None,
    )
    .unwrap();
    let trinomial = run(
        ok_iter(&records),
        &config,
        SprtVariant::Trinomial,
        false,
        FailurePolicy::ReportOnly,
        false,
        None,
        None,
    )
    .unwrap();
    assert_eq!(wald.verdict, trinomial.verdict);
    assert!((wald.llr - trinomial.llr).abs() < 1e-6);
}

#[test]
fn clear_h0_stream_fails() {
    let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();
    let records: Vec<_> = (0..2000)
        .map(|i| (i + 1, rec(Some("baseline_win"))))
        .collect();
    let report = run(
        ok_iter(&records),
        &config,
        SprtVariant::Wald,
        false,
        FailurePolicy::ReportOnly,
        false,
        None,
        None,
    )
    .unwrap();
    assert_eq!(report.verdict, Verdict::Fail);
    assert!(report.llr <= report.lower_bound);
}

#[test]
fn small_mixed_sample_stays_inconclusive() {
    let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();
    let records = vec![
        (1, rec(Some("candidate_win"))),
        (2, rec(Some("baseline_win"))),
    ];
    let report = run(
        ok_iter(&records),
        &config,
        SprtVariant::Wald,
        false,
        FailurePolicy::ReportOnly,
        false,
        None,
        None,
    )
    .unwrap();
    assert_eq!(report.verdict, Verdict::Inconclusive);
    assert_eq!(report.stopping_observation_count, None);
    assert_eq!(report.stopping_observation_reason, "insufficient_data");
    assert_eq!(report.available_observation_count, 2);
    assert_eq!(report.analyzed_observation_count, 2);
    assert_eq!(report.ignored_observations_after_stop, 0);
}

#[test]
fn wald_and_trinomial_retain_the_first_crossing_even_if_the_full_input_drifts_back() {
    let config = SprtConfig::new(0.0, 100.0, 0.4, 0.4).unwrap();
    let records = vec![
        (1, rec(Some("candidate_win"))),
        (2, rec(Some("candidate_win"))),
        (3, rec(Some("baseline_win"))),
        (4, rec(Some("baseline_win"))),
    ];

    for variant in [SprtVariant::Wald, SprtVariant::Trinomial] {
        let report = run(
            ok_iter(&records),
            &config,
            variant,
            false,
            FailurePolicy::ReportOnly,
            false,
            None,
            None,
        )
        .unwrap();

        assert_eq!(report.verdict, Verdict::Pass);
        assert!(report.decision_llr >= report.upper_bound);
        assert!(report.llr > report.lower_bound && report.llr < report.upper_bound);
        assert_eq!(report.candidate_wins, 2);
        assert_eq!(report.baseline_wins, 2);
        assert_eq!(report.decision_candidate_wins, 2);
        assert_eq!(report.decision_baseline_wins, 0);
        assert_eq!(report.available_observation_count, 4);
        assert_eq!(report.analyzed_observation_count, 2);
        assert_eq!(report.stopping_observation_count, Some(2));
        assert_eq!(report.stopping_observation_reason, "upper_bound_crossed");
        assert_eq!(report.ignored_observations_after_stop, 2);
        assert_eq!(report.available_record_count, 4);
        assert_eq!(report.analyzed_record_count, 2);
        assert_eq!(report.ignored_records_after_stop, 2);
    }
}

#[test]
fn streaming_unpaired_wald_and_trinomial_match_retained_reference_schedule() {
    let config = SprtConfig::new(0.0, 100.0, 0.4, 0.4).unwrap();
    let records = vec![
        (1, rec(Some("candidate_win"))),
        (2, rec(Some("candidate_win"))),
        (3, rec(Some("baseline_win"))),
        (4, rec(Some("draw"))),
        (5, rec(Some("baseline_win"))),
    ];

    for variant in [SprtVariant::Wald, SprtVariant::Trinomial] {
        let streaming = run_with_options(
            ok_iter(&records),
            &config,
            SprtRunOptions {
                variant,
                ..SprtRunOptions::default()
            },
        )
        .unwrap();
        // Id-less records remain singleton observations in paired mode, so this exercises the
        // retained replay implementation as an independent reference for the same schedule.
        let retained = run_with_options(
            ok_iter(&records),
            &config,
            SprtRunOptions {
                variant,
                paired_by_id: true,
                ..SprtRunOptions::default()
            },
        )
        .unwrap();

        assert_eq!(streaming.verdict, retained.verdict);
        assert_eq!(streaming.llr, retained.llr);
        assert_eq!(streaming.decision_llr, retained.decision_llr);
        assert_eq!(streaming.drawelo, retained.drawelo);
        assert_eq!(streaming.decision_drawelo, retained.decision_drawelo);
        assert_eq!(streaming.candidate_wins, retained.candidate_wins);
        assert_eq!(streaming.baseline_wins, retained.baseline_wins);
        assert_eq!(streaming.draws, retained.draws);
        assert_eq!(
            streaming.decision_candidate_wins,
            retained.decision_candidate_wins
        );
        assert_eq!(
            streaming.decision_baseline_wins,
            retained.decision_baseline_wins
        );
        assert_eq!(streaming.decision_draws, retained.decision_draws);
        assert_eq!(
            streaming.available_observation_count,
            retained.available_observation_count
        );
        assert_eq!(
            streaming.analyzed_observation_count,
            retained.analyzed_observation_count
        );
        assert_eq!(
            streaming.stopping_observation_count,
            retained.stopping_observation_count
        );
        assert_eq!(
            streaming.stopping_observation_reason,
            retained.stopping_observation_reason
        );
        assert_eq!(
            streaming.available_record_count,
            retained.available_record_count
        );
        assert_eq!(
            streaming.analyzed_record_count,
            retained.analyzed_record_count
        );
    }
}

#[test]
fn streaming_unpaired_matches_retained_reference_for_all_short_schedules() {
    let config = SprtConfig::new(0.0, 100.0, 0.4, 0.4).unwrap();
    let labels = ["baseline_win", "draw", "candidate_win"];

    for len in 1..=7usize {
        for mut encoded in 0..3usize.pow(len as u32) {
            let records: Vec<_> = (0..len)
                .map(|index| {
                    let label = labels[encoded % 3];
                    encoded /= 3;
                    (index + 1, rec(Some(label)))
                })
                .collect();

            for variant in [SprtVariant::Wald, SprtVariant::Trinomial] {
                let streaming = run_with_options(
                    ok_iter(&records),
                    &config,
                    SprtRunOptions {
                        variant,
                        ..SprtRunOptions::default()
                    },
                )
                .unwrap();
                let retained = run_with_options(
                    ok_iter(&records),
                    &config,
                    SprtRunOptions {
                        variant,
                        paired_by_id: true,
                        ..SprtRunOptions::default()
                    },
                )
                .unwrap();

                assert_eq!(streaming.verdict, retained.verdict);
                assert_eq!(streaming.llr, retained.llr);
                assert_eq!(streaming.decision_llr, retained.decision_llr);
                assert_eq!(streaming.drawelo, retained.drawelo);
                assert_eq!(streaming.decision_drawelo, retained.decision_drawelo);
                assert_eq!(streaming.candidate_wins, retained.candidate_wins);
                assert_eq!(streaming.baseline_wins, retained.baseline_wins);
                assert_eq!(streaming.draws, retained.draws);
                assert_eq!(
                    streaming.analyzed_observation_count,
                    retained.analyzed_observation_count
                );
                assert_eq!(
                    streaming.stopping_observation_count,
                    retained.stopping_observation_count
                );
                assert_eq!(
                    streaming.stopping_observation_reason,
                    retained.stopping_observation_reason
                );
            }
        }
    }
}

#[test]
fn wald_and_trinomial_treat_input_order_as_the_schedule() {
    let config = SprtConfig::new(0.0, 100.0, 0.4, 0.4).unwrap();
    let candidate_first = vec![
        (1, rec(Some("candidate_win"))),
        (2, rec(Some("candidate_win"))),
        (3, rec(Some("baseline_win"))),
        (4, rec(Some("baseline_win"))),
    ];
    let baseline_first = vec![
        (1, rec(Some("baseline_win"))),
        (2, rec(Some("baseline_win"))),
        (3, rec(Some("candidate_win"))),
        (4, rec(Some("candidate_win"))),
    ];

    for variant in [SprtVariant::Wald, SprtVariant::Trinomial] {
        let first = run(
            ok_iter(&candidate_first),
            &config,
            variant,
            false,
            FailurePolicy::ReportOnly,
            false,
            None,
            None,
        )
        .unwrap();
        let reversed = run(
            ok_iter(&baseline_first),
            &config,
            variant,
            false,
            FailurePolicy::ReportOnly,
            false,
            None,
            None,
        )
        .unwrap();

        assert_eq!(first.llr, reversed.llr);
        assert_eq!(first.verdict, Verdict::Pass);
        assert_eq!(reversed.verdict, Verdict::Fail);
        assert_eq!(first.stopping_observation_count, Some(2));
        assert_eq!(reversed.stopping_observation_count, Some(2));
    }
}

#[test]
fn paired_wald_checks_boundaries_only_after_a_pair_completes() {
    let config = SprtConfig::new(0.0, 100.0, 0.4, 0.4).unwrap();
    let records = vec![
        (1, rec_with_id(Some("p1"), Some("candidate_win"))),
        (2, rec_with_id(Some("p1"), Some("candidate_win"))),
        (3, rec_with_id(Some("p2"), Some("candidate_win"))),
        (4, rec_with_id(Some("p2"), Some("candidate_win"))),
        (5, rec_with_id(Some("p3"), Some("baseline_win"))),
        (6, rec_with_id(Some("p3"), Some("baseline_win"))),
        (7, rec_with_id(Some("p4"), Some("baseline_win"))),
        (8, rec_with_id(Some("p4"), Some("baseline_win"))),
    ];
    let report = run(
        ok_iter(&records),
        &config,
        SprtVariant::Wald,
        true,
        FailurePolicy::ReportOnly,
        true,
        None,
        None,
    )
    .unwrap();

    assert_eq!(report.verdict, Verdict::Pass);
    assert_eq!(report.analysis_unit, "pair");
    assert_eq!(report.available_observation_count, 4);
    assert_eq!(report.analyzed_observation_count, 2);
    assert_eq!(report.available_record_count, 8);
    assert_eq!(report.analyzed_record_count, 4);
    assert_eq!(report.ignored_records_after_stop, 4);
}

#[test]
fn draws_do_not_move_the_llr() {
    let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();
    let records = vec![(1, rec(Some("draw"))), (2, rec(Some("draw")))];
    let report = run(
        ok_iter(&records),
        &config,
        SprtVariant::Wald,
        false,
        FailurePolicy::ReportOnly,
        false,
        None,
        None,
    )
    .unwrap();
    assert_eq!(report.llr, 0.0);
    assert_eq!(report.draws, 2);
}

#[test]
fn rejects_elo0_not_less_than_elo1() {
    assert!(matches!(
        SprtConfig::new(10.0, 10.0, 0.05, 0.05),
        Err(VeridictError::InvalidThreshold(_))
    ));
    assert!(matches!(
        SprtConfig::new(10.0, 0.0, 0.05, 0.05),
        Err(VeridictError::InvalidThreshold(_))
    ));
}

#[test]
fn rejects_alpha_beta_out_of_range() {
    assert!(matches!(
        SprtConfig::new(0.0, 10.0, 0.0, 0.05),
        Err(VeridictError::InvalidThreshold(_))
    ));
    assert!(matches!(
        SprtConfig::new(0.0, 10.0, 0.05, 1.0),
        Err(VeridictError::InvalidThreshold(_))
    ));
}

#[test]
fn rejects_alpha_beta_sum_at_or_above_one() {
    for (alpha, beta) in [(0.5, 0.5), (0.9, 0.9)] {
        match SprtConfig::new(0.0, 10.0, alpha, beta) {
            Err(error @ VeridictError::InvalidThreshold(_)) => {
                assert!(
                    error
                        .to_string()
                        .contains("alpha + beta must be less than 1")
                );
            }
            _ => panic!("alpha={alpha}, beta={beta} should be rejected"),
        }
    }
}

#[test]
fn rejects_settings_that_cannot_produce_finite_sprt_numbers() {
    let smallest_positive = f64::from_bits(1);
    for result in [
        SprtConfig::new(0.0, 10.0, smallest_positive, 0.05),
        SprtConfig::new(0.0, 1e308, 0.05, 0.05),
        SprtConfig::new(-1e308, 0.0, 0.05, 0.05),
    ] {
        assert!(matches!(result, Err(VeridictError::InvalidThreshold(_))));
    }
}

#[test]
fn run_revalidates_a_config_struct_literal() {
    let unchecked = SprtConfig {
        elo0: 0.0,
        elo1: 1e308,
        alpha: 0.05,
        beta: 0.05,
    };
    let records = [(1, rec(Some("baseline_win")))];
    assert!(matches!(
        run_with_options(ok_iter(&records), &unchecked, SprtRunOptions::default()),
        Err(VeridictError::InvalidThreshold(_))
    ));
}

#[test]
fn empty_input_is_an_error() {
    let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();
    assert!(matches!(
        run(
            ok_iter(&[]),
            &config,
            SprtVariant::Wald,
            false,
            FailurePolicy::ReportOnly,
            false,
            None,
            None,
        ),
        Err(VeridictError::EmptyInput)
    ));
}

#[test]
fn unusable_record_is_schema_mismatch() {
    let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();
    let records = vec![(1, rec(None))];
    assert!(matches!(
        run(
            ok_iter(&records),
            &config,
            SprtVariant::Wald,
            false,
            FailurePolicy::ReportOnly,
            false,
            None,
            None,
        ),
        Err(VeridictError::SchemaMismatch { .. })
    ));
}

#[test]
fn paired_by_id_nets_split_pairs_to_a_draw() {
    let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();
    // Every testcase splits 1-1 (net draw) when paired: LLR stays at 0,
    // even though un-paired this would be 1000 wins + 1000 losses too -
    // same total either way here, so assert the more telling case below.
    let records: Vec<_> = (0..1000)
        .flat_map(|i| {
            [
                (
                    i * 2 + 1,
                    rec_with_id(Some(&format!("op{i}")), Some("candidate_win")),
                ),
                (
                    i * 2 + 2,
                    rec_with_id(Some(&format!("op{i}")), Some("baseline_win")),
                ),
            ]
        })
        .collect();
    let report = run(
        ok_iter(&records),
        &config,
        SprtVariant::Wald,
        true,
        FailurePolicy::ReportOnly,
        false,
        None,
        None,
    )
    .unwrap();
    assert_eq!(report.llr, 0.0);
    assert_eq!(report.draws, 1000);
}

/// `n` pairs (`2n` records), each pair scoring `outcomes` (candidate's two per-game
/// outcomes for that pair, summed to one pentanomial bucket).
fn pentanomial_records(n: usize, outcomes: (&str, &str)) -> Vec<(usize, Record)> {
    (0..n)
        .flat_map(|i| {
            let id = format!("op{i}");
            [
                (i * 2 + 1, rec_with_id(Some(&id), Some(outcomes.0))),
                (i * 2 + 2, rec_with_id(Some(&id), Some(outcomes.1))),
            ]
        })
        .collect()
}

#[test]
fn pentanomial_paired_2_0_favors_candidate() {
    let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();
    let records = pentanomial_records(200, ("candidate_win", "candidate_win"));
    let report = run(
        ok_iter(&records),
        &config,
        SprtVariant::Pentanomial,
        true,
        FailurePolicy::ReportOnly,
        false,
        None,
        None,
    )
    .unwrap();
    assert_eq!(report.sprt_variant, "pentanomial");
    // Unanimous, extreme evidence like this crosses the upper bound well before all 200
    // input pairs complete - the walk stops there, and the remainder is never analyzed. See
    // the module doc: this is the fix, not a coincidence to work around.
    assert_eq!(report.available_paired_count, Some(200));
    assert_eq!(report.raw_trial_count, Some(400));
    let analyzed = report.paired_count.unwrap();
    assert!(analyzed < 200);
    assert_eq!(report.stopping_pair_count, Some(analyzed));
    assert_eq!(report.stopping_reason, Some("upper_bound_crossed"));
    assert_eq!(report.ignored_pairs_after_stop, Some(200 - analyzed));
    let counts = report.pentanomial_counts.as_ref().unwrap();
    assert_eq!(counts.score_2_0, analyzed);
    assert_eq!(
        [
            counts.score_0_0,
            counts.score_0_5,
            counts.score_1_0,
            counts.score_1_5
        ],
        [0, 0, 0, 0]
    );
    assert_eq!(report.candidate_wins, analyzed);
    assert!(report.llr > 0.0);
    assert_eq!(report.verdict, Verdict::Pass);
    assert_eq!(report.drawelo, None);
}

#[test]
fn pentanomial_paired_1_5_0_5_favors_candidate() {
    let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();
    let records = pentanomial_records(200, ("candidate_win", "draw"));
    let report = run(
        ok_iter(&records),
        &config,
        SprtVariant::Pentanomial,
        true,
        FailurePolicy::ReportOnly,
        false,
        None,
        None,
    )
    .unwrap();
    let analyzed = report.paired_count.unwrap();
    assert!(analyzed < 200);
    assert_eq!(report.stopping_reason, Some("upper_bound_crossed"));
    let counts = report.pentanomial_counts.as_ref().unwrap();
    assert_eq!(counts.score_1_5, analyzed);
    assert_eq!(report.candidate_wins, analyzed);
    assert!(report.llr > 0.0);
}

#[test]
fn pentanomial_paired_1_1_eventually_favors_baseline_despite_a_tiny_elo_gap() {
    // Every pair scores exactly 1-1 (candidate_win + baseline_win), pinning the empirical
    // mean at exactly 0.5 - H0's own target (elo0=0). Zero empirical variance around that
    // point is itself evidence *against* any alternative with a different target mean,
    // however small the gap (elo1=10 here) - so this is not a perpetually-neutral stream;
    // given enough pairs, it crosses the *lower* bound (rejecting H1) purely from having no
    // variance to explain away. This is the generalized LLR test working as intended (see
    // `stats::pentanomial_sprt`'s module doc), not a quirk of this fixture.
    let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();
    let records = pentanomial_records(200, ("candidate_win", "baseline_win"));
    let report = run(
        ok_iter(&records),
        &config,
        SprtVariant::Pentanomial,
        true,
        FailurePolicy::ReportOnly,
        false,
        None,
        None,
    )
    .unwrap();
    let analyzed = report.paired_count.unwrap();
    assert!(analyzed < 200);
    assert_eq!(report.verdict, Verdict::Fail);
    assert_eq!(report.stopping_reason, Some("lower_bound_crossed"));
    let counts = report.pentanomial_counts.as_ref().unwrap();
    assert_eq!(counts.score_1_0, analyzed);
    assert_eq!(report.draws, analyzed);
    assert_eq!(report.candidate_wins, 0);
    assert_eq!(report.baseline_wins, 0);
}

#[test]
fn pentanomial_paired_0_5_1_5_favors_baseline() {
    let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();
    let records = pentanomial_records(200, ("baseline_win", "draw"));
    let report = run(
        ok_iter(&records),
        &config,
        SprtVariant::Pentanomial,
        true,
        FailurePolicy::ReportOnly,
        false,
        None,
        None,
    )
    .unwrap();
    let analyzed = report.paired_count.unwrap();
    assert!(analyzed < 200);
    assert_eq!(report.stopping_reason, Some("lower_bound_crossed"));
    let counts = report.pentanomial_counts.as_ref().unwrap();
    assert_eq!(counts.score_0_5, analyzed);
    assert_eq!(report.baseline_wins, analyzed);
    assert!(report.llr < 0.0);
}

#[test]
fn pentanomial_paired_0_2_favors_baseline() {
    let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();
    let records = pentanomial_records(200, ("baseline_win", "baseline_win"));
    let report = run(
        ok_iter(&records),
        &config,
        SprtVariant::Pentanomial,
        true,
        FailurePolicy::ReportOnly,
        false,
        None,
        None,
    )
    .unwrap();
    let analyzed = report.paired_count.unwrap();
    assert!(analyzed < 200);
    assert_eq!(report.stopping_reason, Some("lower_bound_crossed"));
    let counts = report.pentanomial_counts.as_ref().unwrap();
    assert_eq!(counts.score_0_0, analyzed);
    assert_eq!(report.baseline_wins, analyzed);
    assert!(report.llr < 0.0);
    assert_eq!(report.verdict, Verdict::Fail);
}

#[test]
fn pentanomial_incomplete_pair_is_an_error() {
    let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();
    let mut records = pentanomial_records(50, ("candidate_win", "baseline_win"));
    records.push((999, rec_with_id(Some("lonely"), Some("candidate_win"))));
    assert!(matches!(
        run(
            ok_iter(&records),
            &config,
            SprtVariant::Pentanomial,
            true,
            FailurePolicy::ReportOnly,
            false,
            None,
            None,
        ),
        Err(VeridictError::SchemaMismatch {
            context: "pentanomial",
            ..
        })
    ));
}

#[test]
fn pentanomial_triple_id_is_an_error() {
    let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();
    let mut records = pentanomial_records(50, ("candidate_win", "baseline_win"));
    records.push((997, rec_with_id(Some("op0"), Some("candidate_win"))));
    assert!(matches!(
        run(
            ok_iter(&records),
            &config,
            SprtVariant::Pentanomial,
            true,
            FailurePolicy::ReportOnly,
            false,
            None,
            None,
        ),
        Err(VeridictError::SchemaMismatch {
            context: "pentanomial",
            ..
        })
    ));
}

#[test]
fn pentanomial_record_without_id_is_an_error() {
    let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();
    let records = vec![(1, rec(Some("candidate_win")))];
    assert!(matches!(
        run(
            ok_iter(&records),
            &config,
            SprtVariant::Pentanomial,
            true,
            FailurePolicy::ReportOnly,
            false,
            None,
            None,
        ),
        Err(VeridictError::SchemaMismatch {
            context: "pentanomial",
            ..
        })
    ));
}

#[test]
fn pentanomial_without_paired_by_id_is_rejected() {
    let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();
    let records = pentanomial_records(50, ("candidate_win", "baseline_win"));
    assert!(matches!(
        run(
            ok_iter(&records),
            &config,
            SprtVariant::Pentanomial,
            false,
            FailurePolicy::ReportOnly,
            false,
            None,
            None,
        ),
        Err(VeridictError::InvalidThreshold(_))
    ));
}

// --- apply_failure_caps ---

#[test]
fn sprt_report_defaults_to_valid_with_no_caps_applied() {
    let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();
    let records: Vec<_> = (0..40).map(|_| (1, rec(Some("candidate_win")))).collect();
    let report = run(
        ok_iter(&records),
        &config,
        SprtVariant::Wald,
        false,
        FailurePolicy::ReportOnly,
        false,
        None,
        None,
    )
    .unwrap();
    assert_eq!(report.validity, Validity::Valid);
    assert_eq!(
        report.promotion,
        Promotion::decide(Validity::Valid, report.verdict)
    );
}

#[test]
fn sprt_report_invalidated_by_a_breached_crash_cap() {
    let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();
    let mut records: Vec<_> = (0..40).map(|_| (1, rec(Some("candidate_win")))).collect();
    for i in 0..3 {
        records.push((
            41 + i,
            Record {
                id: None,
                baseline: None,
                candidate: None,
                result: None,
                baseline_status: None,
                candidate_status: Some("crash".to_string()),
            },
        ));
    }
    let mut report = run(
        ok_iter(&records),
        &config,
        SprtVariant::Wald,
        false,
        FailurePolicy::ReportOnly,
        false,
        None,
        None,
    )
    .unwrap();
    assert_eq!(report.crashes, 3);
    let caps = FailureCaps {
        max_crashes: Some(0),
        ..Default::default()
    };
    apply_failure_caps(&mut report, &caps);
    assert_eq!(report.validity, Validity::Invalid);
    assert_eq!(report.verdict, Verdict::Inconclusive);
    assert_eq!(report.promotion, Promotion::NotPromoted);
    assert!(report.reason.contains("INVALID"));
}

// --- min_paired_ids/max_paired_ids validation ---

#[test]
fn min_paired_ids_zero_is_rejected() {
    let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();
    let records = pentanomial_records(5, ("candidate_win", "candidate_win"));
    assert!(matches!(
        run(
            ok_iter(&records),
            &config,
            SprtVariant::Pentanomial,
            true,
            FailurePolicy::ReportOnly,
            false,
            Some(0),
            None,
        ),
        Err(VeridictError::InvalidThreshold(_))
    ));
}

#[test]
fn max_paired_ids_zero_is_rejected() {
    let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();
    let records = pentanomial_records(5, ("candidate_win", "candidate_win"));
    assert!(matches!(
        run(
            ok_iter(&records),
            &config,
            SprtVariant::Pentanomial,
            true,
            FailurePolicy::ReportOnly,
            false,
            None,
            Some(0),
        ),
        Err(VeridictError::InvalidThreshold(_))
    ));
}

#[test]
fn max_paired_ids_below_min_paired_ids_is_rejected() {
    let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();
    let records = pentanomial_records(5, ("candidate_win", "candidate_win"));
    assert!(matches!(
        run(
            ok_iter(&records),
            &config,
            SprtVariant::Pentanomial,
            true,
            FailurePolicy::ReportOnly,
            false,
            Some(10),
            Some(5),
        ),
        Err(VeridictError::InvalidThreshold(_))
    ));
}

#[test]
fn min_paired_ids_may_equal_max_paired_ids() {
    let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();
    let records = pentanomial_records(50, ("candidate_win", "baseline_win")); // neutral
    let report = run(
        ok_iter(&records),
        &config,
        SprtVariant::Pentanomial,
        true,
        FailurePolicy::ReportOnly,
        false,
        Some(50),
        Some(50),
    )
    .unwrap();
    assert_eq!(report.paired_count, Some(50));
    assert_eq!(report.stopping_reason, Some("max_paired_ids_reached"));
}

#[test]
fn min_max_paired_ids_require_pentanomial_variant() {
    let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();
    let records: Vec<_> = (0..10)
        .map(|i| (i + 1, rec(Some("candidate_win"))))
        .collect();
    assert!(matches!(
        run(
            ok_iter(&records),
            &config,
            SprtVariant::Wald,
            false,
            FailurePolicy::ReportOnly,
            false,
            Some(5),
            None,
        ),
        Err(VeridictError::InvalidThreshold(_))
    ));
}

// --- sequential min/max-paired-ids walk ---

#[test]
fn below_minimum_stays_inconclusive_even_though_the_llr_would_already_decide() {
    let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();
    let records = pentanomial_records(200, ("candidate_win", "candidate_win"));
    let unconstrained = run(
        ok_iter(&records),
        &config,
        SprtVariant::Pentanomial,
        true,
        FailurePolicy::ReportOnly,
        false,
        None,
        None,
    )
    .unwrap();
    // Sanity: this unanimous stream decides well before all 200 pairs complete - otherwise
    // the gated assertion below would hold vacuously (nothing to withhold).
    assert_eq!(unconstrained.verdict, Verdict::Pass);
    assert!(unconstrained.paired_count.unwrap() < 200);

    let gated = run(
        ok_iter(&records),
        &config,
        SprtVariant::Pentanomial,
        true,
        FailurePolicy::ReportOnly,
        false,
        Some(250), // above available_paired_count (200): the minimum is never reached
        None,
    )
    .unwrap();
    // The crossing happens well before the minimum, but the walk never even evaluates a
    // boundary check until the minimum is reached - and here it never is, so this is exactly
    // as inconclusive as a stream that never crossed anything, not "decided, but overridden."
    assert_eq!(gated.verdict, Verdict::Inconclusive);
    assert_eq!(gated.validity, Validity::Valid); // insufficient pairs, not a corrupt run
    assert_eq!(gated.paired_count, Some(200));
    assert_eq!(gated.stopping_pair_count, None);
    assert_eq!(gated.stopping_reason, Some("insufficient_data"));
    assert!(gated.reason.contains("200"));
    assert!(gated.reason.contains("250"));
    // Pin the surprising-looking field state deliberately: llr sits past upper_bound even
    // though verdict is Inconclusive, because stopping_pair_count is None (never evaluated
    // against the bounds). A caller that re-derives a decision from llr vs the bounds instead
    // of reading verdict would get this backwards - see the doc comment on SprtReport::llr.
    assert!(gated.llr > gated.upper_bound);
}

#[test]
fn reaching_minimum_lets_a_real_decision_through() {
    let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();
    let records = pentanomial_records(200, ("candidate_win", "candidate_win"));
    let report = run(
        ok_iter(&records),
        &config,
        SprtVariant::Pentanomial,
        true,
        FailurePolicy::ReportOnly,
        false,
        Some(1),
        None,
    )
    .unwrap();
    assert_eq!(report.verdict, Verdict::Pass);
    assert_eq!(report.stopping_reason, Some("upper_bound_crossed"));
    assert_eq!(report.available_paired_count, Some(200));
    // The walk stopped well before the input's end - the remainder was never analyzed.
    assert!(report.paired_count.unwrap() < 200);
    assert_eq!(report.stopping_pair_count, report.paired_count);
    assert_eq!(
        report.ignored_pairs_after_stop,
        Some(200 - report.paired_count.unwrap())
    );
}

#[test]
fn max_paired_ids_reached_without_crossing_stays_inconclusive() {
    let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();
    let records = pentanomial_records(50, ("candidate_win", "baseline_win")); // neutral
    let unconstrained = run(
        ok_iter(&records),
        &config,
        SprtVariant::Pentanomial,
        true,
        FailurePolicy::ReportOnly,
        false,
        None,
        None,
    )
    .unwrap();
    assert_eq!(unconstrained.verdict, Verdict::Inconclusive);

    let report = run(
        ok_iter(&records),
        &config,
        SprtVariant::Pentanomial,
        true,
        FailurePolicy::ReportOnly,
        false,
        None,
        Some(50),
    )
    .unwrap();
    assert_eq!(report.verdict, Verdict::Inconclusive); // no truncated-SPRT decision rule
    assert_eq!(report.stopping_reason, Some("max_paired_ids_reached"));
    assert_eq!(report.stopping_pair_count, Some(50));
    assert_eq!(report.ignored_pairs_after_stop, Some(0));
    assert!(report.reason.contains("--max-paired-ids"));
}

// --- require_complete_pairs ---

#[test]
fn require_complete_pairs_turns_a_lone_wald_id_into_a_hard_error() {
    let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();
    let mut records: Vec<_> = (0..40)
        .map(|i| {
            (
                i + 1,
                rec_with_id(Some(&format!("op{i}")), Some("candidate_win")),
            )
        })
        .collect();
    records.push((999, rec_with_id(Some("lonely"), Some("candidate_win"))));
    assert!(matches!(
        run(
            ok_iter(&records),
            &config,
            SprtVariant::Wald,
            true,
            FailurePolicy::ReportOnly,
            true,
            None,
            None,
        ),
        Err(VeridictError::SchemaMismatch {
            context: "paired-by-id",
            ..
        })
    ));
}

#[test]
fn without_require_complete_pairs_a_lone_wald_id_is_tolerated() {
    let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();
    let mut records: Vec<_> = (0..40)
        .map(|i| {
            (
                i + 1,
                rec_with_id(Some(&format!("op{i}")), Some("candidate_win")),
            )
        })
        .collect();
    records.push((999, rec_with_id(Some("lonely"), Some("candidate_win"))));
    let report = run(
        ok_iter(&records),
        &config,
        SprtVariant::Wald,
        true,
        FailurePolicy::ReportOnly,
        false,
        None,
        None,
    )
    .unwrap();
    assert!(!report.require_complete_pairs);
}

#[test]
fn require_complete_pairs_is_a_no_op_for_pentanomial() {
    // Pentanomial already unconditionally errors on a lone id via its own
    // `PentanomialCollector`, regardless of this flag - same error either way.
    let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();
    let mut records = pentanomial_records(50, ("candidate_win", "baseline_win"));
    records.push((999, rec_with_id(Some("lonely"), Some("candidate_win"))));
    for require_complete_pairs in [false, true] {
        assert!(matches!(
            run(
                ok_iter(&records),
                &config,
                SprtVariant::Pentanomial,
                true,
                FailurePolicy::ReportOnly,
                require_complete_pairs,
                None,
                None,
            ),
            Err(VeridictError::SchemaMismatch {
                context: "pentanomial",
                ..
            })
        ));
    }
}

// --- the discriminating test: no look-ahead on a fully-formed input file ---

/// One pair per bucket index (0..=4), giving full control over the sequential trajectory a
/// test wants to construct - unlike `pentanomial_records`, which only ever repeats one
/// bucket. Config here is always `SprtConfig::new(0.0, 10.0, 0.05, 0.05)` - the same one
/// every other test in this module uses - so the bucket-to-pair mapping below only has to
/// be verified once (see the probing that produced these exact pair counts).
fn pentanomial_records_from_buckets(buckets: &[usize]) -> Vec<(usize, Record)> {
    let outcomes = |b: usize| -> (&'static str, &'static str) {
        match b {
            0 => ("baseline_win", "baseline_win"),
            1 => ("baseline_win", "draw"),
            2 => ("candidate_win", "baseline_win"),
            3 => ("candidate_win", "draw"),
            4 => ("candidate_win", "candidate_win"),
            _ => panic!("bucket index must be 0..=4, got {b}"),
        }
    };
    buckets
        .iter()
        .enumerate()
        .flat_map(|(i, &b)| {
            let id = format!("op{i}");
            let (a, c) = outcomes(b);
            [
                (i * 2 + 1, rec_with_id(Some(&id), Some(a))),
                (i * 2 + 2, rec_with_id(Some(&id), Some(c))),
            ]
        })
        .collect()
}

/// 150 pairs of bucket 4 (2-0, unanimous candidate), then 150 of bucket 0 (0-2, unanimous
/// baseline), then 300 neutral (bucket 2) - elo0=0/elo1=10/alpha=beta=0.05. Verified (by
/// direct trajectory inspection while writing this test) to cross the upper bound at pair
/// 104, fall back within bounds by pair 195, and settle at a final (all-600-pairs) LLR of
/// about -0.50 - comfortably *inside* the bounds. A reader (or an implementation) that only
/// looks at the finished file's aggregate would call this run inconclusive; the formal
/// sequential answer is PASS, decided at pair 104.
fn crosses_upper_then_settles_inconclusive() -> Vec<usize> {
    let mut buckets = vec![4; 150];
    buckets.extend(std::iter::repeat_n(0, 150));
    buckets.extend(std::iter::repeat_n(2, 300));
    buckets
}

/// Mirror of the above: bucket 0 first (crosses lower around pair 101), then bucket 4
/// (back within bounds by pair 201), then neutral padding to 600.
fn crosses_lower_then_settles_inconclusive() -> Vec<usize> {
    let mut buckets = vec![0; 150];
    buckets.extend(std::iter::repeat_n(4, 150));
    buckets.extend(std::iter::repeat_n(2, 300));
    buckets
}

#[test]
fn sequential_walk_decides_at_the_true_crossing_pair_not_the_files_final_state() {
    let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();
    let records = pentanomial_records_from_buckets(&crosses_upper_then_settles_inconclusive());
    let report = run(
        ok_iter(&records),
        &config,
        SprtVariant::Pentanomial,
        true,
        FailurePolicy::ReportOnly,
        false,
        None,
        None,
    )
    .unwrap();
    // The whole point: a naive final-aggregate recompute over all 600 pairs would land
    // within bounds (verified while constructing this fixture) and call this Inconclusive.
    // The correct sequential answer is PASS, decided the moment the walk first crossed.
    assert_eq!(report.verdict, Verdict::Pass);
    assert_eq!(report.stopping_pair_count, Some(104));
    assert_eq!(report.paired_count, Some(104));
    assert_eq!(report.available_paired_count, Some(600));
    assert_eq!(report.ignored_pairs_after_stop, Some(600 - 104));
    assert_eq!(report.stopping_reason, Some("upper_bound_crossed"));
}

#[test]
fn appending_pairs_after_the_stopping_point_changes_nothing() {
    let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();
    let full_buckets = crosses_upper_then_settles_inconclusive();
    let truncated_records = pentanomial_records_from_buckets(&full_buckets[..300]); // phase 1 + phase 2 only
    let full_records = pentanomial_records_from_buckets(&full_buckets); // + 300 more pairs

    let run_on = |records: &[(usize, Record)]| {
        run(
            ok_iter(records),
            &config,
            SprtVariant::Pentanomial,
            true,
            FailurePolicy::ReportOnly,
            false,
            None,
            None,
        )
        .unwrap()
    };
    let truncated = run_on(&truncated_records);
    let full = run_on(&full_records);
    assert_eq!(truncated.verdict, full.verdict);
    assert_eq!(truncated.stopping_pair_count, full.stopping_pair_count);
    assert_eq!(truncated.llr, full.llr);
    assert_eq!(truncated.paired_count, full.paired_count);
}

#[test]
fn sequential_walk_decides_fail_at_the_true_lower_crossing_pair_not_the_files_final_state() {
    let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();
    let records = pentanomial_records_from_buckets(&crosses_lower_then_settles_inconclusive());
    let report = run(
        ok_iter(&records),
        &config,
        SprtVariant::Pentanomial,
        true,
        FailurePolicy::ReportOnly,
        false,
        None,
        None,
    )
    .unwrap();
    // Mirror of the upper-crossing test: a naive final-aggregate recompute over all 600
    // pairs lands within bounds and would call this Inconclusive. The correct sequential
    // answer is FAIL, decided the moment the walk first crossed the lower bound.
    assert_eq!(report.verdict, Verdict::Fail);
    assert_eq!(report.stopping_pair_count, Some(101));
    assert_eq!(report.paired_count, Some(101));
    assert_eq!(report.available_paired_count, Some(600));
    assert_eq!(report.ignored_pairs_after_stop, Some(600 - 101));
    assert_eq!(report.stopping_reason, Some("lower_bound_crossed"));
}

#[test]
fn crossing_upper_before_minimum_that_settles_back_stays_inconclusive() {
    let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();
    let records = pentanomial_records_from_buckets(&crosses_upper_then_settles_inconclusive());
    // At pair 200 (past the 195-pair reversion point, verified while constructing this
    // fixture), the accumulated LLR is already back within bounds - so gating on
    // min_paired_ids=200 must not resurrect the earlier, now-irrelevant pair-104 crossing.
    let report = run(
        ok_iter(&records),
        &config,
        SprtVariant::Pentanomial,
        true,
        FailurePolicy::ReportOnly,
        false,
        Some(200),
        None,
    )
    .unwrap();
    assert_eq!(report.verdict, Verdict::Inconclusive);
    assert_eq!(report.stopping_pair_count, None);
    assert_eq!(report.stopping_reason, Some("insufficient_data"));
    assert_eq!(report.paired_count, Some(600));
}

#[test]
fn crossing_lower_before_minimum_that_settles_back_stays_inconclusive() {
    let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();
    let records = pentanomial_records_from_buckets(&crosses_lower_then_settles_inconclusive());
    // Mirror of the upper case: back within bounds by pair 201 (verified while constructing
    // this fixture), so min_paired_ids=250 must not resurrect the pair-101 lower crossing.
    let report = run(
        ok_iter(&records),
        &config,
        SprtVariant::Pentanomial,
        true,
        FailurePolicy::ReportOnly,
        false,
        Some(250),
        None,
    )
    .unwrap();
    assert_eq!(report.verdict, Verdict::Inconclusive);
    assert_eq!(report.stopping_pair_count, None);
    assert_eq!(report.stopping_reason, Some("insufficient_data"));
    assert_eq!(report.paired_count, Some(600));
}

#[test]
fn max_paired_ids_ignores_a_crossing_that_happens_after_the_cutoff() {
    let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();
    let records = pentanomial_records_from_buckets(&crosses_upper_then_settles_inconclusive());
    // The same file crosses upper at pair 104 if let run - but max_paired_ids=90 stops the
    // walk before that ever happens, so this must land Inconclusive at pair 90, not Pass.
    let report = run(
        ok_iter(&records),
        &config,
        SprtVariant::Pentanomial,
        true,
        FailurePolicy::ReportOnly,
        false,
        None,
        Some(90),
    )
    .unwrap();
    assert_eq!(report.verdict, Verdict::Inconclusive);
    assert_eq!(report.stopping_pair_count, Some(90));
    assert_eq!(report.stopping_reason, Some("max_paired_ids_reached"));
    assert_eq!(report.ignored_pairs_after_stop, Some(600 - 90));
}

#[test]
fn odd_length_prefix_is_an_error_even_length_prefix_is_not() {
    let config = SprtConfig::new(0.0, 10.0, 0.05, 0.05).unwrap();
    let records = pentanomial_records(10, ("candidate_win", "baseline_win"));
    // A prefix ending mid-pair (odd number of records) - the shape a poller would see if it
    // ever checked between a pair's two records instead of at a completed-pair boundary.
    let mut odd_prefix = records.clone();
    odd_prefix.truncate(7);
    assert!(matches!(
        run(
            ok_iter(&odd_prefix),
            &config,
            SprtVariant::Pentanomial,
            true,
            FailurePolicy::ReportOnly,
            false,
            None,
            None,
        ),
        Err(VeridictError::SchemaMismatch {
            context: "pentanomial",
            ..
        })
    ));
    // A prefix at a completed-pair boundary (even number of records) - an ordinary report.
    let mut even_prefix = records.clone();
    even_prefix.truncate(8);
    assert!(
        run(
            ok_iter(&even_prefix),
            &config,
            SprtVariant::Pentanomial,
            true,
            FailurePolicy::ReportOnly,
            false,
            None,
            None,
        )
        .is_ok()
    );
}
