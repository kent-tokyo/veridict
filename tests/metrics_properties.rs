//! Property test for the P0-1 refactor's core invariant: computing several
//! metrics in one `compute_many` pass must produce byte-for-byte the same
//! per-metric output as running each metric independently through
//! `compute`. Every generated record carries both a `result` field and
//! `baseline`/`candidate` fields, so it's simultaneously valid input for
//! every metric under test - this isolates "does single-pass batching change
//! anything" from "is a mixed-schema file rejected" (already covered by
//! `metrics.rs`'s own unit tests).

use proptest::prelude::*;
use veridict::input::Record;
use veridict::metrics::{compute, compute_many};
use veridict::{BootstrapMethod, CiMethod, FailurePolicy, MetricConfig};

const SEED: u64 = 0x5EED;

fn arb_record() -> impl Strategy<Value = Record> {
    (
        prop_oneof![Just("candidate_win"), Just("baseline_win"), Just("draw")],
        -100.0f64..100.0,
        -100.0f64..100.0,
    )
        .prop_map(|(result, baseline, candidate)| Record {
            id: None,
            baseline: Some(baseline),
            candidate: Some(candidate),
            result: Some(result.to_string()),
            baseline_status: None,
            candidate_status: None,
        })
}

proptest! {
    #[test]
    fn compute_many_matches_independent_compute_calls(records in prop::collection::vec(arb_record(), 1..40)) {
        let records: Vec<(usize, Record)> = records.into_iter().enumerate().map(|(i, r)| (i + 1, r)).collect();
        let metrics = [
            MetricConfig::WinRate { ci_method: CiMethod::Wilson, failure_policy: FailurePolicy::ReportOnly },
            MetricConfig::MeanDiff { bootstrap_method: BootstrapMethod::Percentile },
            MetricConfig::SignTest { ci_method: CiMethod::Wilson },
            MetricConfig::Elo { failure_policy: FailurePolicy::ReportOnly },
        ];

        let combined = compute_many(records.iter().cloned(), &metrics, 0.95, 1000, SEED, false, false).unwrap();

        for (i, &metric) in metrics.iter().enumerate() {
            let independent = compute(records.iter().cloned(), metric, 0.95, 1000, SEED, false, false).unwrap();
            prop_assert_eq!(combined[i].paired_count, independent.paired_count);
            prop_assert_eq!(combined[i].baseline_count, independent.baseline_count);
            prop_assert_eq!(combined[i].candidate_count, independent.candidate_count);
            prop_assert_eq!(combined[i].timeouts, independent.timeouts);
            prop_assert_eq!(combined[i].crashes, independent.crashes);
            prop_assert_eq!(combined[i].invalid, independent.invalid);
            prop_assert!((combined[i].effect - independent.effect).abs() < 1e-9);
            prop_assert!((combined[i].ci_low - independent.ci_low).abs() < 1e-9);
            prop_assert!((combined[i].ci_high - independent.ci_high).abs() < 1e-9);
        }
    }
}

// --- relative-diff ---

fn numeric_record(baseline: f64, candidate: f64) -> Record {
    Record {
        id: None,
        baseline: Some(baseline),
        candidate: Some(candidate),
        result: None,
        baseline_status: None,
        candidate_status: None,
    }
}

fn numeric_records(pairs: &[(f64, f64)]) -> Vec<(usize, Record)> {
    pairs
        .iter()
        .enumerate()
        .map(|(i, &(b, c))| (i + 1, numeric_record(b, c)))
        .collect()
}

/// Pins that `relative-diff` is exactly `mean-diff` bootstrapped over pre-transformed relative
/// observations - not a coincidentally-similar independent implementation. Feeding the same raw
/// (baseline, candidate) pairs through `RelativeDiff`, versus hand-computing each pair's own
/// `(candidate - baseline) / baseline` and feeding *that* to `MeanDiff` as `candidate` against a
/// `baseline` of `0.0` (so `MeanDiff`'s `candidate - baseline` reproduces the precomputed ratio
/// exactly, via `x - 0.0 == x` for every finite `x`), must produce bit-identical `effect`/`ci_low`/
/// `ci_high` under the same seed/resamples/bootstrap method - both paths end up bootstrapping the
/// exact same `Vec<f64>` through the exact same resampling code.
#[test]
fn relative_diff_matches_a_manually_normalized_mean_diff_bit_identically() {
    let raw: &[(f64, f64)] = &[
        (100.0, 110.0),
        (200.0, 180.0),
        (1_000.0, 1_050.0),
        (50.0, 48.0),
        (5_000.0, 5_400.0),
        (7.5, 7.2),
    ];
    let relative_records = numeric_records(raw);
    let manual_pairs: Vec<(f64, f64)> = raw.iter().map(|&(b, c)| (0.0, (c - b) / b)).collect();
    let manual_records = numeric_records(&manual_pairs);

    for method in [
        BootstrapMethod::Percentile,
        BootstrapMethod::Basic,
        BootstrapMethod::Bca,
    ] {
        let relative = compute(
            relative_records.iter().cloned(),
            MetricConfig::RelativeDiff {
                bootstrap_method: method,
            },
            0.95,
            2000,
            SEED,
            false,
            false,
        )
        .unwrap();
        let manual = compute(
            manual_records.iter().cloned(),
            MetricConfig::MeanDiff {
                bootstrap_method: method,
            },
            0.95,
            2000,
            SEED,
            false,
            false,
        )
        .unwrap();
        assert_eq!(
            relative.effect, manual.effect,
            "effect mismatch for {method:?}"
        );
        assert_eq!(
            relative.ci_low, manual.ci_low,
            "ci_low mismatch for {method:?}"
        );
        assert_eq!(
            relative.ci_high, manual.ci_high,
            "ci_high mismatch for {method:?}"
        );
    }
}

/// Same 5% relative improvement expressed at two wildly different scales: `relative-diff`'s effect
/// stays put, `mean-diff`'s scales up by exactly the same 1000x factor as the input - the two
/// metrics measuring genuinely different quantities, not one being a display variant of the other.
/// Deliberately does not assert which one (if either) should pass any given threshold.
#[test]
fn relative_diff_effect_is_scale_invariant_while_mean_diff_effect_scales_with_baseline() {
    let small_scale: &[(f64, f64)] = &[(1_000.0, 1_050.0), (2_000.0, 2_100.0), (500.0, 525.0)];
    let large_scale: &[(f64, f64)] = &[
        (1_000_000.0, 1_050_000.0),
        (2_000_000.0, 2_100_000.0),
        (500_000.0, 525_000.0),
    ];

    let rel_small = compute(
        numeric_records(small_scale),
        MetricConfig::RelativeDiff {
            bootstrap_method: BootstrapMethod::Percentile,
        },
        0.95,
        500,
        SEED,
        false,
        false,
    )
    .unwrap();
    let rel_large = compute(
        numeric_records(large_scale),
        MetricConfig::RelativeDiff {
            bootstrap_method: BootstrapMethod::Percentile,
        },
        0.95,
        500,
        SEED,
        false,
        false,
    )
    .unwrap();
    assert!(
        (rel_small.effect - rel_large.effect).abs() < 1e-9,
        "relative-diff's effect should be scale-invariant: {} vs {}",
        rel_small.effect,
        rel_large.effect
    );

    let mean_small = compute(
        numeric_records(small_scale),
        MetricConfig::MeanDiff {
            bootstrap_method: BootstrapMethod::Percentile,
        },
        0.95,
        500,
        SEED,
        false,
        false,
    )
    .unwrap();
    let mean_large = compute(
        numeric_records(large_scale),
        MetricConfig::MeanDiff {
            bootstrap_method: BootstrapMethod::Percentile,
        },
        0.95,
        500,
        SEED,
        false,
        false,
    )
    .unwrap();
    assert!((mean_large.effect / mean_small.effect - 1000.0).abs() < 1e-6);
}

fn arb_scale_triple() -> impl Strategy<Value = (f64, f64, f64)> {
    (1.0f64..1_000.0, 1.0f64..1_000.0, 0.01f64..100.0)
}

proptest! {
    /// `relative-diff`'s effect/CI are invariant under rescaling every pair's baseline and
    /// candidate by the same positive per-pair constant `k_i` - "candidate is 5% above baseline"
    /// doesn't depend on which units baseline/candidate happen to be measured in. Not bit-exact
    /// (multiplying introduces its own rounding, unlike `numeric_records`' `x - 0.0` shortcut
    /// above), so this checks a relative tolerance rather than exact equality.
    #[test]
    fn relative_diff_effect_and_ci_are_invariant_under_positive_per_pair_rescaling(
        triples in prop::collection::vec(arb_scale_triple(), 3..25)
    ) {
        let base_pairs: Vec<(f64, f64)> = triples.iter().map(|&(b, c, _)| (b, c)).collect();
        let scaled_pairs: Vec<(f64, f64)> = triples.iter().map(|&(b, c, k)| (b * k, c * k)).collect();

        let base = compute(
            numeric_records(&base_pairs).into_iter(),
            MetricConfig::RelativeDiff { bootstrap_method: BootstrapMethod::Percentile },
            0.95, 500, SEED, false, false,
        ).unwrap();
        let scaled = compute(
            numeric_records(&scaled_pairs).into_iter(),
            MetricConfig::RelativeDiff { bootstrap_method: BootstrapMethod::Percentile },
            0.95, 500, SEED, false, false,
        ).unwrap();

        let rel_tol = |a: f64, b: f64| (a - b).abs() < 1e-6 * (1.0 + a.abs().max(b.abs()));
        prop_assert!(rel_tol(base.effect, scaled.effect), "effect: {} vs {}", base.effect, scaled.effect);
        prop_assert!(rel_tol(base.ci_low, scaled.ci_low), "ci_low: {} vs {}", base.ci_low, scaled.ci_low);
        prop_assert!(rel_tol(base.ci_high, scaled.ci_high), "ci_high: {} vs {}", base.ci_high, scaled.ci_high);
    }
}
