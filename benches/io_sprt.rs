//! Large-input diagnostics for the two paths covered by the performance/memory contract:
//! reusable-buffer JSONL parsing and unpaired sequential replay. These are local diagnostics,
//! not cross-machine or competitor speed claims.

use std::hint::black_box;
use std::io::Cursor;
use std::time::Duration;

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use veridict::input::{Record, parse_jsonl};
use veridict::sprt::{SprtConfig, SprtRunOptions, run_with_options};
use veridict::{FailurePolicy, VeridictError};

const RECORDS: usize = 100_000;

fn jsonl_fixture() -> Vec<u8> {
    let mut bytes = Vec::with_capacity(RECORDS * 52);
    for index in 0..RECORDS {
        let outcome = match index % 3 {
            0 => "candidate_win",
            1 => "baseline_win",
            _ => "draw",
        };
        bytes.extend_from_slice(
            format!("{{\"id\":\"case-{index}\",\"result\":\"{outcome}\"}}\n").as_bytes(),
        );
    }
    bytes
}

fn outcome_records() -> Vec<(usize, Record)> {
    (0..RECORDS)
        .map(|index| {
            let result = match index % 3 {
                0 => "candidate_win",
                1 => "baseline_win",
                _ => "draw",
            };
            (
                index + 1,
                Record {
                    id: None,
                    baseline: None,
                    candidate: None,
                    result: Some(result.to_string()),
                    baseline_status: None,
                    candidate_status: None,
                },
            )
        })
        .collect()
}

fn bench_jsonl_parse(c: &mut Criterion) {
    let fixture = jsonl_fixture();
    let mut group = c.benchmark_group("jsonl_parse");
    group.throughput(Throughput::Elements(RECORDS as u64));
    group.bench_function("100k_records", |b| {
        b.iter(|| {
            let count = parse_jsonl::<Record, _>(Cursor::new(black_box(fixture.as_slice()))).fold(
                0usize,
                |count, record| {
                    black_box(record.unwrap());
                    count + 1
                },
            );
            black_box(count)
        });
    });
    group.finish();
}

fn bench_unpaired_wald(c: &mut Criterion) {
    let records = outcome_records();
    // A symmetric interval and balanced fixture keep the LLR between bounds so the benchmark
    // exercises the full schedule instead of measuring only an early crossing.
    let config = SprtConfig::new(-10.0, 10.0, 0.05, 0.05).unwrap();
    let mut group = c.benchmark_group("sprt_unpaired_wald");
    group.throughput(Throughput::Elements(RECORDS as u64));
    group.bench_function("100k_records", |b| {
        b.iter(|| {
            run_with_options(
                records.iter().cloned().map(Ok::<_, VeridictError>),
                &config,
                SprtRunOptions {
                    failure_policy: FailurePolicy::ReportOnly,
                    ..SprtRunOptions::default()
                },
            )
            .unwrap()
        });
    });
    group.finish();
}

fn config() -> Criterion {
    Criterion::default()
        .sample_size(20)
        .warm_up_time(Duration::from_secs(1))
        .measurement_time(Duration::from_secs(5))
}

criterion_group! {
    name = benches;
    config = config();
    targets = bench_jsonl_parse, bench_unpaired_wald
}
criterion_main!(benches);
