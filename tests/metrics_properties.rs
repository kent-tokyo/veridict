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
use veridict::{BootstrapMethod, CiMethod, MetricKind};

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
        let metrics = [MetricKind::WinRate, MetricKind::MeanDiff, MetricKind::SignTest, MetricKind::Elo];

        let combined = compute_many(records.iter().cloned().map(Ok), &metrics, 0.95, 1000, SEED, false, CiMethod::Wilson, BootstrapMethod::Percentile).unwrap();

        for (i, &metric) in metrics.iter().enumerate() {
            let independent = compute(records.iter().cloned().map(Ok), metric, 0.95, 1000, SEED, false, CiMethod::Wilson, BootstrapMethod::Percentile).unwrap();
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
