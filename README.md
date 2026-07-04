# veridict

English | [日本語](README_ja.md)

[![CI](https://github.com/kent-tokyo/veridict/actions/workflows/ci.yml/badge.svg)](https://github.com/kent-tokyo/veridict/actions/workflows/ci.yml)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)

A small, domain-agnostic evaluation gate: decide whether a candidate is
actually better than a baseline, from a file of trial results.

`veridict` is not a benchmark runner or an experiment tracker. It is the
statistical decision layer that consumes results and returns a verdict:

* `pass`
* `fail`
* `inconclusive`

When the data is noisy, small, or unclear, it says `inconclusive` rather
than overclaiming. A false pass is worse than an inconclusive result.

## Use cases

Any "candidate vs baseline" comparison where you'd otherwise eyeball a
spreadsheet and guess:

* **Game/search engine regression** - win/loss/draw match results ->
  `--metric winrate`, `--metric elo`, or `veridict sprt` for sequential testing.
* **OCR or extraction-pipeline accuracy** - per-document accuracy scores ->
  `--metric mean-diff` or `--metric sign-test`.
* **LLM prompt or model comparison** - pairwise judge verdicts or numeric
  quality scores -> `--metric winrate` or `--metric mean-diff`.
* **Ranking/optimization algorithm tuning** - a numeric objective per run
  (NDCG, loss, throughput) -> `--metric mean-diff`.
* **Release regression gate in CI** - candidate build vs last known-good
  baseline, wired into a pipeline with `--fail-below`/`--pass-above` and a
  `veridict` exit code (see [Regression gate](#usage) below).
* **Ranking more than two variants** - several prompts/configs against the
  same shared baseline -> `veridict matrix`.

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
  --report-json report.json \
  --report-md report.md
```

Read from stdin with `-`:

```bash
cat results.jsonl | veridict compare - --metric winrate
```

Run several metrics against the same input in one pass; the overall verdict
is the strictest of the individual ones (any `fail` wins, then any
`inconclusive`, else `pass`):

```bash
veridict compare results.jsonl --metric winrate --metric sign-test --min-effect 0.02
```

Exact binomial CI on a small sample, BCa bootstrap on a skewed one:

```bash
veridict compare results.jsonl --metric winrate --ci-method exact
veridict compare scores.jsonl --metric mean-diff --bootstrap-method bca
```

Sequential testing: keep feeding it results until it can confidently say
the candidate is at least `--elo1` points stronger (pass), at most `--elo0`
points stronger (fail), or it needs more data (inconclusive):

```bash
veridict sprt results.jsonl --elo0 0 --elo1 10 --alpha 0.05 --beta 0.05
```

Compare more than two candidates at once, each measured against the same
shared baseline, and tabulate pairwise Elo differences:

```bash
veridict matrix prompt_a.jsonl prompt_b.jsonl prompt_c.jsonl
```

### Exit codes

| Code | Meaning |
|------|---------|
| 0 | pass |
| 1 | fail |
| 2 | inconclusive |
| 3 | invalid input or configuration error |

## Input format

One record per line: JSONL by default, or CSV (`--format csv`, or
auto-detected from a `.csv` file extension). Both share the same fields.
See `examples/`:

* `examples/winloss.jsonl` - win/loss/draw records, for `--metric winrate` / `--metric sign-test`.
* `examples/paired_scores.jsonl` - paired baseline/candidate scores, for `--metric mean-diff` / `--metric sign-test`.
* `examples/status_failures.jsonl` - all supported record shapes together, illustrating the format (not meant to be run against a single metric as-is: a record must carry a field the chosen metric understands, or a `baseline_status`/`candidate_status` field, or it is rejected as a schema mismatch).

```json
{"id":"case-001","baseline":0.81,"candidate":0.84}
{"id":"case-002","result":"candidate_win"}
{"id":"case-003","result":"draw"}
{"id":"case-004","baseline_status":"ok","candidate_status":"timeout"}
{"id":"case-005","baseline_status":"ok","candidate_status":"invalid"}
```

Same shape as CSV, with empty cells treated as absent fields:

```csv
id,baseline,candidate,result,baseline_status,candidate_status
case-001,0.81,0.84,,,
case-002,,,candidate_win,,
case-004,,,,ok,timeout
```

## Metrics

* **`winrate`** - confidence interval on decisive (non-draw) `result`
  records. `--ci-method wilson` (default) or `--ci-method exact`
  (Clopper-Pearson - exact coverage at any sample size, but always at least
  as wide as Wilson's).
* **`sign-test`** - same CI, on the proportion of paired numeric records
  where the candidate beat the baseline (ties excluded). Nonparametric
  alternative to `mean-diff`: only the direction of each pair matters, not
  its magnitude. Also takes `--ci-method`.
* **`mean-diff`** - bootstrap confidence interval on `candidate - baseline`
  for paired numeric records. `--bootstrap-method percentile` (default) or
  `--bootstrap-method bca` (bias-corrected and accelerated - corrects for a
  skewed diff distribution; `percentile` stays the default so existing CI
  numbers don't shift under you). `--resamples` controls the bootstrap
  sample count; `--seed` controls its RNG seed (fixed by default, so output
  is bit-identical across CI runs of the same input).
* **`elo`** - Elo rating difference from win/loss/draw `result` records
  (draws count as half a win, unlike `winrate`/`sign-test` which exclude
  them). Reported in Elo points, via the standard logistic model. Doesn't
  support `--ci-method exact`: its win rate is fractional (a draw is half a
  win), and Clopper-Pearson's coverage guarantee only holds for a true
  integer-count binomial.

`winrate` and `sign-test` report `effect`/`ci_low`/`ci_high` centered on 0
(deviation from a 50/50 split); `elo` is centered on 0 by construction (an
even score is 0 Elo). All three compose directly with `--min-effect`.
`mean-diff` reports them in the input's own units.

Every trial's `baseline_status`/`candidate_status` (`timeout`, `crash`,
`invalid`) is tallied and reported regardless of which metric you run, both
combined and broken down by which side failed (`failure_breakdown` in the
JSON report).

Requesting several `--metric` flags together scans the input once, feeding
every record to every requested metric, rather than one full pass per
metric.

## Report extras

Every `compare` report also carries two advisory fields that never affect
`verdict`:

* **`estimated_additional_trials`** - a rough estimate (`O(1/sqrt(n))` CI
  scaling) of how many more trials would likely turn an `inconclusive`
  result decisive, assuming the effect size itself doesn't move. `null`
  when there's nothing useful to suggest - already decided, zero trials, or
  the effect sits *inside* the pass/fail threshold band (the "dead zone"):
  shrinking the CI around a point estimate that's already in the dead zone
  can never cross either boundary, no matter how much data you add.
  Treat the number as "roughly this many, plausibly more," not a
  guarantee - it has a documented, quantified bias (e.g. an ~18%
  under-estimate at n=100 for one verified case).
* **`warnings`** - data-quality flags, empty when there's nothing to flag:
  a tiny sample (under 30 paired trials), an excessive failure rate (over
  20% timeout/crash/invalid), or, for `elo`, a draw-heavy run (over 50%
  draws leaves few decisive outcomes to rate from).

## SPRT

`veridict sprt` is a separate mode from `compare`: instead of an effect
size and a confidence interval checked against a threshold, it accumulates
a log-likelihood ratio (Wald's classic two-outcome SPRT) over decisive
(non-draw) `result` records and stops as soon as the evidence crosses one
of two boundaries derived from `--alpha`/`--beta`. `pass` means "confident
the candidate is at least `--elo1` points stronger"; `fail` means
"confident it's at most `--elo0` points stronger"; `inconclusive` means
"keep collecting data". `--alpha`/`--beta` are its actual guaranteed false
positive/negative rates, not tunable knobs on a report - there's no
`--min-effect`/`--confidence` for this subcommand.

## Comparison matrix

`veridict matrix` takes one file per candidate - each measured against the
*same shared baseline*, using the same `result`-field win/loss/draw records
as `--metric elo`/`--metric winrate` - and tabulates pairwise Elo
differences. It's report-only (no verdict, always exits 0 on success):
there's no single pass/fail for a whole matrix.

Every candidate only ever plays the shared baseline (never each other), so
the underlying model is a star graph: each candidate's rating is exactly
its own Elo-vs-baseline (the Bradley-Terry MLE on a star graph has no
shared terms to solve jointly - no iterative solver needed). Cells against
`baseline` are direct data; candidate-vs-candidate cells are
model-extrapolated (`elo_i - elo_j`, marked `*` in the Markdown table)
with a CI from normal-approximation error propagation across the two
independent samples - wider than either direct cell's CI, as expected.

## Paired testcases

`--paired-by-id` (on `compare`, `sprt`, and `matrix`) treats two records
sharing the same `id` as one testcase played twice - e.g. re-run with roles
swapped to cancel that testcase's own bias - and combines them into a
single net observation instead of two independent ones:

* `winrate`/`elo`: net by total points across the pair (win=1, draw=0.5,
  loss=0, the standard "paired game" convention) - `>1` is a net candidate
  win, `<1` a net baseline win, exactly `1` a net draw.
* `mean-diff`/`sign-test`: net by averaging the pair's two diffs.

An `id` used only once is an ordinary unpaired sample (mixing paired and
unpaired testcases in one file is fine). Three or more records sharing an
`id` is rejected as a data error, not silently truncated to a pair. Without
`--paired-by-id`, a duplicate `id` on `mean-diff`/`sign-test` records is
still rejected outright, same as before this flag existed.

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
cargo audit
```

CI (`.github/workflows/ci.yml`) runs all four on every push and pull request.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.
