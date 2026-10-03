# veridict

English | [日本語](README_ja.md)

[![CI](https://github.com/kent-tokyo/veridict/actions/workflows/ci.yml/badge.svg)](https://github.com/kent-tokyo/veridict/actions/workflows/ci.yml)
[![Crates.io](https://img.shields.io/crates/v/veridict.svg)](https://crates.io/crates/veridict)
[![docs.rs](https://img.shields.io/docsrs/veridict)](https://docs.rs/veridict)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)

`veridict` is a small, domain-agnostic statistical gate. It reads evaluation results and returns
`pass`, `fail`, or `inconclusive`. It does not run benchmarks or track experiments. When the
evidence is insufficient, the result is `inconclusive`.

Use it for candidate-versus-baseline checks such as model accuracy, extraction quality, search
strength, ranking quality, latency, or release regressions.

## Install

```bash
cargo install veridict
```

For library use:

```bash
cargo add veridict
```

## Quick start

Compare win/loss/draw results:

```bash
veridict compare results.jsonl \
  --metric winrate \
  --pass-above 0.02 \
  --fail-below -0.01 \
  --confidence 0.95
```

Compare paired numeric scores:

```bash
veridict compare scores.jsonl --metric mean-diff --min-effect 0.01
```

Write machine- and human-readable reports:

```bash
veridict compare results.jsonl \
  --metric winrate \
  --min-effect 0.02 \
  --report-json report.json \
  --report-md report.md
```

Use `-` to read stdin. JSONL is the default; CSV is selected by `.csv` or `--format csv`.

## Input

Each line is one observation. A record may contain a paired numeric score, an outcome, or status
fields:

```json
{"id":"case-001","baseline":0.81,"candidate":0.84}
{"id":"case-002","result":"candidate_win"}
{"id":"case-003","result":"draw"}
{"id":"case-004","baseline_status":"ok","candidate_status":"timeout"}
```

CSV uses the same fields:

```csv
id,baseline,candidate,result,baseline_status,candidate_status
case-001,0.81,0.84,,,
case-002,,,candidate_win,,
case-004,,,,ok,timeout
```

Input is validated strictly. Invalid JSON, missing metric fields, ID multiplicity that violates the
pairing rule, non-finite numbers, empty input, and invalid configuration return exit code `3`. See [`examples/`](examples/)
and [`schemas/input-record.schema.json`](schemas/input-record.schema.json).

## Metrics

| Metric | Input | Effect and interval |
|---|---|---|
| `winrate` | `result` | Candidate decisive-win-rate margin above `0.5`; Wilson by default |
| `sign-test` | paired scores | Positive-sign-rate margin above `0.5` among non-ties |
| `mean-diff` | paired scores | Mean of `candidate - baseline`; bootstrap interval |
| `quantile-diff` | paired scores | Quantile of paired differences; bootstrap interval |
| `relative-diff` | positive baseline scores | Mean proportional change; bootstrap interval |
| `elo` | `result` | Logistic Elo difference, with draws scored as one half |

Important options:

- `--ci-method wilson|exact|jeffreys` for binomial metrics.
- `--bootstrap-method percentile|basic|bca` where supported.
- `--paired-by-id` to combine up to two records with the same ID into one metric-specific
  observation. A singleton remains usable; three or more records for one ID are invalid.
- `--cluster-by-id` to cluster-bootstrap `winrate` or `elo`; this is not pairing.
- `--failure-policy report-only|exclude|loss` for outcome-based metrics and sequential tests.
- `--max-timeouts`, `--max-crashes`, and `--max-invalid` to cap invalid evidence.
- `--claim-correction bonferroni|holm` for simultaneous multi-metric claims.

Run multiple metrics by repeating `--metric`. The overall result is the strictest verdict:
`fail`, then `inconclusive`, then `pass`.

```bash
veridict compare results.jsonl \
  --metric winrate \
  --metric elo \
  --min-effect winrate=0.02 \
  --min-effect elo=10 \
  --claim-correction holm
```

The full assumptions, formulas, incompatible option combinations, and report fields are in
[`docs/metrics.md`](docs/metrics.md).

## Sequential tests

`sprt` evaluates an ordered stream and stops at the first accepted boundary crossing:

```bash
veridict sprt results.jsonl --elo0 0 --elo1 10 --alpha 0.05 --beta 0.05
```

Variants:

- `wald` (default): decisive outcomes only; logistic-Elo hypotheses.
- `trinomial`: draw-aware; BayesElo hypotheses supplied with `--belo0` and `--belo1`.
- `pentanomial`: paired two-trial outcomes; requires `--paired-by-id`.

All variants replay input in order. Wald and trinomial evaluate each usable observation, or each
completed net pair with `--paired-by-id`; pentanomial evaluates each completed pair. Later records
do not alter the decision LLR or outcome prefix, but they are still validated and counted as
failures. A breached failure cap can therefore make the final verdict `inconclusive`.

Reports use `decision_*` and generic `available_*`, `analyzed_*`, `stopping_*`, and
`ignored_*_after_stop` fields for the decision prefix. For schema-v1 compatibility, unprefixed
Wald/trinomial counts and `llr` still describe the full input. Consume `verdict` rather than
reconstructing it from the legacy `llr`.

## Other commands

### Matrix and planning

```bash
veridict matrix --matches examples/matches_head_to_head.jsonl
veridict plan --matches examples/matches_head_to_head.jsonl --min-elo 20
```

`matrix` estimates pairwise Elo differences. `plan` ranks pairs that need more evidence; it does
not schedule or run trials. `matrix` also accepts candidate files measured against a shared
baseline.

### Power

```bash
veridict power --metric elo --min-effect 20 --assume-effect 35 --target-power 0.80
veridict power --metric mean-diff --min-effect 0.02 --assume-effect 0.10 --assume-sd 0.15
veridict power --sprt --elo0 0 --elo1 20
```

For `mean-diff`, use `--assume-sd` or `--pilot FILE`. Power results are model-based planning
estimates, not guarantees.

### Verify a run envelope

```bash
veridict verify-run examples/manifest.toml run.jsonl
```

`verify-run` checks pairing, order, contamination, and consistency between manifest values and
recorded opaque identifiers or hashes. It does not open artifacts or recompute hashes, and it does
not prove benchmark quality or candidate strength.

### Time-sensitive testing

```bash
veridict time-sensitive examples/chess_engine_time_sensitive.jsonl \
  --p0 0.50 --p1 0.55 --policy bellman \
  --reward hard-deadline --deadline 400
```

The experimental `time-sensitive` command handles Bernoulli simple-vs-simple tests where an early
decision has more value than a late one. Policies are `bellman`, `edo`, and `gro`; rewards are
`hard-deadline`, `exponential`, or a custom schedule. This command is one-sided and separate from
Elo SPRT.

## Verdicts and exit codes

A confidence-interval gate returns `pass` when the lower bound clears the pass threshold, `fail`
when the upper bound is below the fail threshold, and `inconclusive` otherwise. A breached validity
cap sets `validity=invalid`, `verdict=inconclusive`, and `promotion=not_promoted`. Warnings and
planning estimates are advisory.

For `compare` and `sprt`:

| Exit code | Meaning |
|---|---|
| `0` | pass |
| `1` | fail |
| `2` | inconclusive |
| `3` | invalid input or configuration |

`verify-run` uses `1` for a structural violation. `time-sensitive` has no statistical fail and uses
`2` when it reaches the reward horizon without a decision.

## Documentation

- [`docs/metrics.md`](docs/metrics.md): statistical contracts and report semantics.
- [`docs/research-map.md`](docs/research-map.md): unshipped candidates and activation conditions.
- [`CHANGELOG.md`](CHANGELOG.md): release history.
- `veridict <command> --help`: current CLI options.

## Development

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
cargo audit
```

## License

Licensed under either [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT), at your option.
