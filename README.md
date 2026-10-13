# veridict

English | [日本語](README_ja.md)

[![CI](https://github.com/kent-tokyo/veridict/actions/workflows/ci.yml/badge.svg)](https://github.com/kent-tokyo/veridict/actions/workflows/ci.yml)
[![Crates.io](https://img.shields.io/crates/v/veridict.svg)](https://crates.io/crates/veridict)
[![docs.rs](https://img.shields.io/docsrs/veridict)](https://docs.rs/veridict)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)

`veridict` is an independent Rust library and CLI for candidate adoption decisions. It reads
externally collected evaluation results, compares a candidate against a baseline under declared
criteria, and returns `pass`, `fail`, or `inconclusive` with an explanation. Its statistical
contracts are domain-agnostic.

The central workflow is a reproducible regression gate, including paired results, sequential
testing, and execution failures. Evaluation runners and adoption actions stay with the caller.
See [Product scope](docs/product-scope.md) for responsibilities and development priorities.

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
  --fail-below -0.01
```

Compare paired numeric scores and save reports:

```bash
veridict compare scores.jsonl \
  --metric mean-diff \
  --min-effect 0.01 \
  --report-json report.json \
  --report-md report.md
```

Run a sequential Elo test:

```bash
veridict sprt results.jsonl --elo0 0 --elo1 10
```

Use `-` for stdin. JSONL is the default; `.csv` or `--format csv` selects CSV.

## Input

Each record is one observation and may contain paired scores, an outcome, or statuses:

```json
{"id":"case-001","baseline":0.81,"candidate":0.84}
{"id":"case-002","result":"candidate_win"}
{"id":"case-003","result":"draw"}
{"id":"case-004","baseline_status":"ok","candidate_status":"timeout"}
```

CSV uses the same fields. Invalid JSON, missing required fields, invalid ID multiplicity,
non-finite numbers, empty input, and invalid configuration return exit code `3`. See
[`examples/`](examples/) and the [input schema](schemas/input-record.schema.json).

## Commands

| Command | Purpose |
|---|---|
| `compare` | Fixed-sample candidate-versus-baseline gate |
| `sprt` | Sequential Wald, trinomial, or pentanomial test |
| `matrix` | Descriptive pairwise Bradley-Terry/Elo matrix |
| `plan` | Rank comparisons that need more evidence |
| `power` | Estimate evidence needs before collection |
| `verify-run` | Validate an experiment envelope and record consistency |
| `time-sensitive` | Experimental one-sided test with time-valued decisions |

`veridict <command> --help` is the source of truth for CLI options. Statistical assumptions and
unsupported combinations are documented in [`docs/metrics.md`](docs/metrics.md).

## Comparison metrics

| Metric | Input | Reported effect |
|---|---|---|
| `winrate` | `result` | Candidate decisive-win-rate margin above `0.5` |
| `sign-test` | paired scores | Positive-sign-rate margin above `0.5` |
| `mean-diff` | paired scores | Mean of `candidate - baseline` |
| `quantile-diff` | paired scores | Selected quantile of paired differences |
| `relative-diff` | positive baseline scores | Mean proportional change |
| `elo` | `result` | Logistic Elo from win/draw/loss score |
| `paired-elo` | exactly two results per ID | Pair-score-aware logistic Elo |

Example with pair-preserving fixed-sample Elo:

```bash
veridict compare paired.jsonl \
  --metric paired-elo \
  --paired-by-id \
  --min-effect 10
```

Important contracts:

- `--paired-by-id` combines up to two records with the same ID. `paired-elo` is stricter and
  requires exactly two usable outcomes for every ID.
- `--cluster-by-id` cluster-bootstraps `winrate` or `elo`; it is not pairing.
- `--failure-policy` controls outcome treatment. Failure counts and validity caps are always
  reported.
- Repeating `--metric` creates a joint gate. Overall precedence is
  `fail > inconclusive > pass`.
- `--claim-correction bonferroni|holm` adds family-adjusted results where supported.

## Verdicts and output

For confidence-interval gates, `pass` requires the lower bound to clear the pass threshold,
`fail` requires the upper bound to fall below the fail threshold, and all other valid results are
`inconclusive`. Breaching a validity cap prevents promotion.

`compare` and `sprt` use deterministic exit codes:

| Code | Meaning |
|---|---|
| `0` | pass |
| `1` | fail |
| `2` | inconclusive |
| `3` | invalid input or configuration |

JSON is the machine-readable contract. Markdown is a human-readable summary. Use `verdict` and
`promotion` rather than reconstructing decisions from intermediate fields.

## Documentation

- [`docs/product-scope.md`](docs/product-scope.md): purpose, responsibilities, and development priorities.
- [`docs/metrics.md`](docs/metrics.md): shipped statistical and report contracts.
- [`docs/performance.md`](docs/performance.md): streaming, retained-state, and benchmark contract.
- [`docs/research-map.md`](docs/research-map.md): unshipped candidates and activation conditions.
- [`CHANGELOG.md`](CHANGELOG.md): release history.
- [JSON Schemas](schemas/): machine-readable input and report contracts.

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
