//! Exact small-sample calibration for the fixed-sample decision floor.
//!
//! With one two-trial pair under a fair independent Bernoulli model, all four ordered outcomes
//! are equally likely. A raw paired-bootstrap point interval promotes the all-candidate-win case,
//! so its false-pass rate is exactly 1/4. The public report gate must make all four cases
//! inconclusive until at least two independent pairs are available.

use veridict::input::Record;
use veridict::metrics::compute;
use veridict::stats::bootstrap::DEFAULT_SEED;
use veridict::verdict::{Thresholds, decide};
use veridict::{FailurePolicy, MetricConfig, Verdict, compare_one};

fn paired_records(first: &str, second: &str) -> [(usize, Record); 2] {
    [
        (
            1,
            Record {
                id: Some("pair".to_string()),
                baseline: None,
                candidate: None,
                result: Some(first.to_string()),
                baseline_status: None,
                candidate_status: None,
            },
        ),
        (
            2,
            Record {
                id: Some("pair".to_string()),
                baseline: None,
                candidate: None,
                result: Some(second.to_string()),
                baseline_status: None,
                candidate_status: None,
            },
        ),
    ]
}

#[test]
fn one_pair_floor_removes_the_exact_twenty_five_percent_false_pass_case() {
    let metric = MetricConfig::PairedElo {
        failure_policy: FailurePolicy::ReportOnly,
    };
    let thresholds = Thresholds::symmetric(10.0).unwrap();
    let mut raw_passes = 0;
    let mut gated_passes = 0;
    let mut gated_decisions = 0;

    for first in ["candidate_win", "baseline_win"] {
        for second in ["candidate_win", "baseline_win"] {
            let records = paired_records(first, second);
            let raw = compute(
                records.clone(),
                metric,
                0.95,
                2_000,
                DEFAULT_SEED,
                true,
                false,
            )
            .unwrap();
            raw_passes +=
                usize::from(decide(raw.ci_low, raw.ci_high, &thresholds).0 == Verdict::Pass);

            let report = compare_one(
                records,
                metric,
                0.95,
                &thresholds,
                2_000,
                DEFAULT_SEED,
                true,
                false,
            )
            .unwrap();
            gated_passes += usize::from(report.verdict == Verdict::Pass);
            gated_decisions += usize::from(report.verdict != Verdict::Inconclusive);
        }
    }

    assert_eq!(
        raw_passes, 1,
        "one of four fair outcomes was a raw false pass"
    );
    assert_eq!(gated_passes, 0);
    assert_eq!(gated_decisions, 0);
}
