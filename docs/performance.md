# Performance and memory

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
| Wald / trinomial SPRT, unpaired | Full-input and decision-prefix counters; no outcome schedule |
| Wald / trinomial SPRT, paired | ID groups and the completed-pair schedule for validation and ordered replay |
| Pentanomial SPRT | ID pairs and completed-pair buckets |

The CLI still parses and validates the entire input after an SPRT boundary crossing. It freezes the
decision-prefix counters at the first crossing while continuing full-input counts and status checks.

## Reproducing the diagnostics

```bash
cargo bench --bench io_sprt -- --noplot
```

The benchmark generates its data in memory. `jsonl_parse/100k_records` parses 100,000 records with
unique IDs and rotating candidate-win, baseline-win, and draw outcomes.
`sprt_unpaired_wald/100k_records` processes the same number of ID-less balanced outcomes with
`elo0=-10`, `elo1=10`, `alpha=beta=0.05`; the balanced stream avoids an early boundary crossing.
Neither case uses randomness, so the seed is not applicable. Criterion uses 20 samples, a one-second
warm-up, and a five-second measurement target.

## 2026-10-13 local diagnostic

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
