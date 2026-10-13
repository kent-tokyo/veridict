# Performance and memory

English | [日本語](performance_ja.md)

Performance work must preserve the verdict contract. Benchmarks are local diagnostics, not claims
about other tools or hardware.

## Memory model

| Path | Retained state |
|---|---|
| JSONL parser | One reusable line buffer plus the current deserialized record |
| `winrate` / `elo`, unpaired and unclustered | Outcome counters; ID advisories additionally retain counts per distinct supplied ID |
| `sign-test`, unpaired | Counters plus distinct IDs for duplicate validation |
| Numeric bootstrap metrics | All usable differences and bootstrap draws |
| `--paired-by-id` / `--cluster-by-id` | Groups keyed by ID until pairing or resampling is complete |
| Wald / trinomial SPRT, unpaired | Full-input and decision-prefix counters; supplied IDs additionally retain counts per distinct ID for advisories |
| Wald / trinomial SPRT, paired | ID groups and the completed-pair schedule for validation and ordered replay |
| Pentanomial SPRT | ID pairs and completed-pair buckets |

The CLI still parses and validates the entire input after an SPRT boundary crossing. It freezes the
decision-prefix counters at the first crossing while continuing full-input counts and status checks.

## Benchmark contract

```bash
cargo bench --bench io_sprt -- --noplot
```

The benchmark generates its data in memory. `jsonl_parse/100k_records` parses 100,000 records with
unique IDs and rotating candidate-win, baseline-win, and draw outcomes.
`sprt_unpaired_wald/100k_records` processes the same number of ID-less balanced outcomes with
`elo0=-10`, `elo1=10`, `alpha=beta=0.05`; the balanced stream avoids an early boundary crossing.
Neither case uses randomness, so the seed is not applicable. Criterion uses 20 samples, a one-second
warm-up, and a five-second measurement target.

For the paired-mean bootstrap workload and its isolated components:

```bash
cargo bench --bench metrics -- 'mean-diff (N=2000, resamples=10000)' --exact --quick
cargo bench --bench metrics -- 'mean-diff bootstrap components' --quick
```

The component benchmark separates random index generation, the real compensated `mean`, and the
final distribution sort and percentile lookup. The isolated times are diagnostics and must not be
added together as an end-to-end estimate: production interleaves resampling and accumulation, so
cache behavior differs.

## Measured baselines

### JSONL and SPRT (2026-10-13)

- Source: changes released in `0.19.5`, measured from the working tree based on commit `027431a`.
- Hardware: Apple M4, 16 GiB memory.
- Platform: arm64 macOS (Darwin 27.0.0), Rust 1.97.0.
- Command: `cargo bench --bench io_sprt -- --noplot`.

| Benchmark | Before, 95% interval | First optimized run | Final run after numeric guards |
|---|---:|---:|---:|
| JSONL parse, 100k | 41.895–69.475 ms | 24.162–24.753 ms | 23.593–23.811 ms |
| Unpaired Wald SPRT, 100k | 12.664–20.397 ms | 7.765–7.916 ms | 8.576–8.843 ms |

The before measurements were noisy, and the optimized SPRT runs also show run-to-run variation.
Both optimized runs remained below the before interval, but the result does not support a single
universal speedup number. Re-run on the target host before using these values for capacity planning.

### Bootstrap (2026-10-14)

- Hardware: Apple M4, 16 GiB memory.
- Platform: arm64 macOS (Darwin 27.0.1), Rust 1.97.0.
- Workload: paired mean difference, 2,000 observations, 10,000 resamples, fixed seed `0x5EED`.

| Measurement | Quick interval |
|---|---:|
| End-to-end metric computation | 66.468–66.609 ms |
| Random resampling indices, 20 million draws | 59.980–59.985 ms |
| Compensated mean, 10,000 × 2,000 values | 16.242–16.894 ms |
| Sort 10,000 draws and select interval endpoints | 78.457–79.837 µs |

A separate 10-second sample of the production loop attributed 5,450 of 7,652 on-CPU samples
(71.2%) to random-range/ChaCha generation. The remaining 2,202 samples (28.8%) were in the
bootstrap loop, which includes compensated accumulation and the final sort. Together with the
isolated sort measurement, this identifies random resampling as the primary cost and interval
selection as negligible for this workload. This profile does not justify changing the RNG or
parallelizing the loop without first preserving fixed-seed output and measuring the overhead.
