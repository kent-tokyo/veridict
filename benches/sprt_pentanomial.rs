//! Quantifies the cost of `sprt::run` with `SprtVariant::Pentanomial` on a large paired
//! dataset: the strict per-id pairing pass (`PentanomialCollector`) plus the LLR's per-pair
//! secular-equation solve (`stats::pentanomial_sprt`'s bisection) - the one SPRT variant here
//! that does real per-report numerical work rather than a closed-form sum (`wald`) or a single
//! non-iterative estimate (`trinomial`).

use criterion::{Criterion, criterion_group, criterion_main};
use veridict::VeridictError;
use veridict::input::Record;
use veridict::sprt::{SprtConfig, SprtVariant, run};

const PAIRS: usize = 20_000;

fn outcome_record(id: &str, result: &str) -> Record {
    Record {
        id: Some(id.to_string()),
        baseline: None,
        candidate: None,
        result: Some(result.to_string()),
        baseline_status: None,
        candidate_status: None,
    }
}

/// A mix spanning all 5 pentanomial buckets, so the bench exercises `regularize`'s ordinary
/// (non-degenerate) path rather than the all-in-one-bucket edge case.
fn pentanomial_records() -> Vec<(usize, Record)> {
    let outcomes = ["candidate_win", "draw", "baseline_win"];
    (0..PAIRS)
        .flat_map(|i| {
            let id = format!("pair{i}");
            let a = outcomes[i % 3];
            let b = outcomes[(i / 3) % 3];
            [
                (i * 2 + 1, outcome_record(&id, a)),
                (i * 2 + 2, outcome_record(&id, b)),
            ]
        })
        .collect()
}

fn bench_pentanomial_sprt_large_jsonl(c: &mut Criterion) {
    let records = pentanomial_records();
    let config = SprtConfig::new(0.0, 20.0, 0.05, 0.05).unwrap();
    c.bench_function(&format!("sprt::run pentanomial ({PAIRS} pairs)"), |b| {
        b.iter(|| {
            run(
                records.iter().cloned().map(Ok::<_, VeridictError>),
                &config,
                SprtVariant::Pentanomial,
                true,
            )
            .unwrap()
        });
    });
}

criterion_group!(benches, bench_pentanomial_sprt_large_jsonl);
criterion_main!(benches);
