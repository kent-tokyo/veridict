# veridict

A small, domain-agnostic evaluation gate: decide whether a candidate is
actually better than a baseline, from a JSONL file of trial results.

`veridict` is not a benchmark runner or an experiment tracker. It is the
statistical decision layer that consumes results and returns a verdict:

* `pass`
* `fail`
* `inconclusive`

When the data is noisy, small, or unclear, it says `inconclusive` rather
than overclaiming. A false pass is worse than an inconclusive result.

## Install / build

```bash
cargo build --release
```

## Usage

```bash
veridict compare results.jsonl --metric winrate --min-effect 0.02 --confidence 0.95
veridict compare scores.jsonl  --metric mean-diff --min-effect 0.01 --confidence 0.95
```

Regression gate with asymmetric thresholds:

```bash
veridict compare results.jsonl \
  --metric winrate \
  --fail-below -0.01 \
  --pass-above 0.02 \
  --confidence 0.95 \
  --report-json report.json
```

Read from stdin with `-`:

```bash
cat results.jsonl | veridict compare - --metric winrate
```

### Exit codes

| Code | Meaning |
|------|---------|
| 0 | pass |
| 1 | fail |
| 2 | inconclusive |
| 3 | invalid input or configuration error |

## Input format

One JSON object per line. See `examples/`:

* `examples/winloss.jsonl` - win/loss/draw records, for `--metric winrate`.
* `examples/paired_scores.jsonl` - paired baseline/candidate scores, for `--metric mean-diff`.
* `examples/status_failures.jsonl` - all supported record shapes together, illustrating the format (not meant to be run against a single metric as-is: a record must carry a field the chosen metric understands, or a `baseline_status`/`candidate_status` field, or it is rejected as a schema mismatch).

```json
{"id":"case-001","baseline":0.81,"candidate":0.84}
{"id":"case-002","result":"candidate_win"}
{"id":"case-003","result":"draw"}
{"id":"case-004","baseline_status":"ok","candidate_status":"timeout"}
{"id":"case-005","baseline_status":"ok","candidate_status":"invalid"}
```

## Metrics

* **`winrate`** - Wilson score interval on decisive (non-draw) `result`
  records. `effect`/`ci_low`/`ci_high` are centered on 0 (deviation from a
  50/50 split), so they compose directly with `--min-effect`.
* **`mean-diff`** - percentile bootstrap confidence interval on
  `candidate - baseline` for paired numeric records.

Every trial's `baseline_status`/`candidate_status` (`timeout`, `crash`,
`invalid`) is tallied and reported regardless of which metric you run.

## Verdict logic

The gate compares the confidence interval, not the point estimate, against
the thresholds: `pass` requires the CI's pessimistic (lower) bound to clear
`--pass-above`; `fail` requires the CI's optimistic (upper) bound to be at
or below `--fail-below`. Anything else, including zero usable trials, is
`inconclusive`.

`--min-effect X` is shorthand for symmetric thresholds
(`--pass-above X --fail-below -X`) and defaults to `0`.

## Development

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
```
