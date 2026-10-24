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

Repository documentation describes the current source tree. The latest published release is
`0.19.6`; use the [v0.19.6 tag](https://github.com/kent-tokyo/veridict/tree/v0.19.6) for its exact
source and docs.

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
veridict compare examples/winloss.jsonl \
  --metric winrate \
  --pass-above 0.02 \
  --fail-below -0.01
```

Compare paired numeric scores and save reports:

```bash
veridict compare examples/paired_scores.jsonl \
  --metric mean-diff \
  --min-effect 0.01 \
  --report-json report.json \
  --report-md report.md
```

Run a sequential Elo test:

```bash
veridict sprt examples/winloss.jsonl --elo0 0 --elo1 10
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
veridict compare examples/chess_engine_paired_openings.jsonl \
  --metric paired-elo \
  --paired-by-id \
  --min-effect 10
```

Important contracts:

- `--paired-by-id` combines up to two records with the same ID. `paired-elo` is stricter and
  requires exactly two usable outcomes for every ID.
- `--cluster-by-id` cluster-bootstraps `winrate` or `elo`; it is not pairing.
- `--failure-policy` controls outcome treatment. Failure counts are always reported; a breached
  validity cap appears in `validity` and `reason`.
- Repeating `--metric` creates a joint gate. Overall precedence is
  `fail > inconclusive > pass`.
- `--claim-correction bonferroni|holm` adds family-adjusted results where supported.
- Fixed-sample reports need at least two independent analysis units for a decisive verdict;
  otherwise they are `inconclusive`.

## Verdicts and output

For confidence-interval gates, `pass` requires the lower bound to clear the pass threshold,
`fail` requires the upper bound to fall below the fail threshold, and all other valid results are
`inconclusive`. Breaching a validity cap forces `validity=invalid`, `verdict=inconclusive`, and
`promotion=not_promoted`.

`compare` and `sprt` use deterministic exit codes:

| Code | Meaning |
|---|---|
| `0` | pass |
| `1` | fail |
| `2` | inconclusive |
| `3` | invalid input or configuration |

`time-sensitive` uses `0` for pass, `2` for inconclusive, and `3` for invalid input or
configuration; its one-sided contract has no statistical-fail exit code `1`.

JSON is the machine-readable contract. Markdown is a human-readable summary. Use `verdict` and
`promotion` rather than reconstructing decisions from intermediate fields.

## Reusable GitHub Actions gate

The repository includes a reusable workflow for `compare`, `sprt`, and `verify-run`. It installs
the exact veridict version declared by the workflow, reads results already produced by the caller,
and requires no consumer secret.

```yaml
jobs:
  gate:
    uses: kent-tokyo/veridict/.github/workflows/veridict-gate.yml@v0.19.6
    with:
      command: compare
      input_path: results.jsonl
      arguments_json: '["--metric","winrate","--pass-above","0.02","--fail-below","-0.01"]'
```

This workflow revision installs veridict `0.19.6`. For production use:

- Pin the workflow to a release tag or full commit SHA.
- Valid `compare` and `sprt` runs upload `report.json` and `report.md` before preserving exit code
  `0`, `1`, or `2`. A valid `verify-run` report uses code `0` or `1`.
- Input or configuration errors use code `3` and upload `gate.json`, `gate.md`, and `stderr.txt`
  without fabricating a statistical report.
- Pass `arguments_json` as a JSON array. Each argument retains its boundary and is never evaluated
  by a shell.

For `verify-run`, code `1` means structural invalidity; it is not a statistical `fail` verdict.

## Documentation

- [`docs/product-scope.md`](docs/product-scope.md): purpose, responsibilities, and development priorities.
- [`docs/metrics.md`](docs/metrics.md): shipped statistical and report contracts.
- [`docs/performance.md`](docs/performance.md): streaming, retained-state, and benchmark contract.
- [`docs/research-map.md`](docs/research-map.md): unshipped candidates and activation conditions.
- [`CHANGELOG.md`](CHANGELOG.md): release history.
- [JSON Schemas](schemas/): versioned contracts for the listed input and report formats;
  `time-sensitive` does not yet have a report schema.

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
