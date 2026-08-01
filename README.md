# veridict

English | [日本語](README_ja.md)

[![CI](https://github.com/kent-tokyo/veridict/actions/workflows/ci.yml/badge.svg)](https://github.com/kent-tokyo/veridict/actions/workflows/ci.yml)
[![Crates.io](https://img.shields.io/crates/v/veridict.svg)](https://crates.io/crates/veridict)
[![docs.rs](https://img.shields.io/docsrs/veridict)](https://docs.rs/veridict)
[![Downloads](https://img.shields.io/crates/d/veridict.svg)](https://crates.io/crates/veridict)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)
[![GitHub stars](https://img.shields.io/github/stars/kent-tokyo/veridict.svg?style=social)](https://github.com/kent-tokyo/veridict)

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
  `--metric winrate`, `--metric elo`, or `veridict sprt` for sequential
  testing (`examples/chess_engine_winloss.jsonl`).
* **OCR or extraction-pipeline accuracy** - per-document accuracy scores ->
  `--metric mean-diff` or `--metric sign-test` (`examples/ocr_accuracy_paired.jsonl`,
  `examples/extraction_quality_paired.jsonl`).
* **LLM prompt or model comparison** - pairwise judge verdicts or numeric
  quality scores -> `--metric winrate` or `--metric mean-diff`
  (`examples/llm_prompt_ab.jsonl`).
* **Ranking/optimization algorithm tuning** - a numeric objective per run
  (NDCG, loss, throughput) -> `--metric mean-diff` (`examples/ranking_elo.jsonl`
  if the objective is itself win/loss/draw-shaped).
* **Latency/tail-performance regression gate** - a mean can hide a worsened
  worst case -> `--metric quantile-diff --quantile 0.95` (or `0.99`) on paired
  per-request latencies (`examples/paired_scores.jsonl`).
* **Benchmark suite with wildly different problem sizes per case** - raw
  score differences would be dominated by the largest cases ->
  `--metric relative-diff` for proportional change instead of absolute
  (`examples/mixed_scale_scores.jsonl`).
* **Release regression gate in CI** - candidate build vs last known-good
  baseline, wired into a pipeline with `--fail-below`/`--pass-above` and a
  `veridict` exit code (see [Regression gate](#usage) below).
* **Ranking more than two variants** - several prompts/configs against the
  same shared baseline -> `veridict matrix`.
* **A deadline or decaying value on how soon you learn the answer** - a
  fixed compute/time budget where an early decisive result is worth more
  than a late one -> `veridict time-sensitive` (Bernoulli simple-vs-simple;
  see [Time-sensitive testing](#time-sensitive-testing)).

## Install / build

As a CLI, from crates.io:

```bash
cargo install veridict
```

As a library dependency:

```bash
cargo add veridict
```

Or build from source:

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

Guard each metric's own claim against a lucky pass, read as a family of simultaneous claims (see
[Multiple-comparison correction](#multiple-comparison-correction)):

```bash
veridict compare results.jsonl --metric winrate --metric elo --min-effect 0.02 --claim-correction holm
```

Exact binomial CI on a small sample, BCa bootstrap on a skewed one:

```bash
veridict compare results.jsonl --metric winrate --ci-method exact
veridict compare scores.jsonl --metric mean-diff --bootstrap-method bca
```

Proportional change relative to the baseline, for benchmarks where problem size or raw score
varies a lot across cases (see [Scale-mismatch diagnostic](#scale-mismatch-diagnostic)):

```bash
veridict compare examples/mixed_scale_scores.jsonl --metric relative-diff --min-effect 0.03
```

Asymmetric thresholds work the same way as for every other metric:

```bash
veridict compare examples/mixed_scale_scores.jsonl \
  --metric relative-diff \
  --pass-above 0.03 \
  --fail-below -0.02
```

Treat a candidate crash/timeout as a loss instead of just reporting it (see [Metrics](#metrics)
for `report-only`/`exclude`/`loss`'s exact semantics):

```bash
veridict compare examples/chess_engine_with_crashes.jsonl --metric winrate --failure-policy loss
```

Sequential testing: keep feeding it results until it can confidently say
the candidate is at least `--elo1` points stronger (pass), at most `--elo0`
points stronger (fail), or it needs more data (inconclusive):

```bash
veridict sprt results.jsonl --elo0 0 --elo1 10 --alpha 0.05 --beta 0.05
```

On draw-heavy data (e.g. chess-engine testing), the trinomial variant converges faster by
estimating the draw rate instead of discarding draws entirely:

```bash
veridict sprt examples/chess_engine_draw_heavy.jsonl --sprt-variant trinomial --belo0 0 --belo1 30
```

For paired-game test designs (same opening played twice, colors swapped), the pentanomial
variant uses the pair's full 5-value combined score instead of netting it down to a single
win/loss/draw - requires `--paired-by-id`:

```bash
veridict sprt examples/chess_engine_paired_openings.jsonl --sprt-variant pentanomial --elo0 0 --elo1 20 --paired-by-id
```

Compare more than two candidates at once, each measured against the same
shared baseline, and tabulate pairwise Elo differences:

```bash
veridict matrix prompt_a.jsonl prompt_b.jsonl prompt_c.jsonl
```

Or rank named competitors directly from head-to-head data, no shared
baseline required:

```bash
veridict matrix --matches examples/matches_head_to_head.jsonl
```

Recommend which of those pairs would most benefit from more trials, ranked most-uncertain first
(same input as `matrix`, plus a required `--min-elo`):

```bash
veridict plan --matches examples/matches_head_to_head.jsonl --min-elo 20
```

Estimate how many trials you'd need, before running a real `compare`:

```bash
veridict power --metric elo --min-effect 20 --assume-effect 35 --target-power 0.80
```

For `--metric mean-diff`, supply an assumed standard deviation directly or estimate one from real
pilot data:

```bash
veridict power --metric mean-diff --min-effect 0.02 --assume-effect 0.10 --assume-sd 0.15
veridict power --metric mean-diff --min-effect 0.02 --assume-effect 0.10 --pilot examples/pilot_scores.jsonl
```

Or estimate SPRT's expected sample size under each hypothesis directly:

```bash
veridict power --sprt --elo0 0 --elo1 20
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
* `examples/paired_scores.jsonl` (and `examples/paired_scores.csv`, same data in the CSV format
  below) - paired baseline/candidate scores, for `--metric mean-diff` / `--metric sign-test`.
* `examples/mixed_scale_scores.jsonl` - paired baseline/candidate scores with baselines spanning a
  wide range, all strictly positive, for `--metric relative-diff` (and to demonstrate `--metric
  mean-diff`'s [scale-mismatch diagnostic](#scale-mismatch-diagnostic)).
* `examples/status_failures.jsonl` - all supported record shapes together, illustrating the format (not meant to be run against a single metric as-is: a record must carry a field the chosen metric understands, or a `baseline_status`/`candidate_status` field, or it is rejected as a schema mismatch).
* `examples/chess_engine_draw_heavy.jsonl` - win/loss/draw records with a high draw rate, for
  `veridict sprt --sprt-variant trinomial` (see [SPRT](#sprt)).
* `examples/chess_engine_paired_openings.jsonl` - each `id` appears exactly twice (same opening,
  colors swapped), for `veridict sprt --sprt-variant pentanomial --paired-by-id` (see [SPRT](#sprt)).
* `examples/chess_engine_with_crashes.jsonl` - win/loss/draw records with a couple of candidate
  failures mixed in, for `--failure-policy loss` (see [Metrics](#metrics)).

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

```bash
veridict compare examples/paired_scores.csv --format csv --metric mean-diff
```

## Metrics

* **`winrate`** - confidence interval on decisive (non-draw) `result`
  records. `--ci-method wilson` (default), `--ci-method exact`
  (Clopper-Pearson - exact coverage at any sample size, but always at least
  as wide as Wilson's), or `--ci-method jeffreys` (Bayesian credible interval
  using the non-informative Jeffreys prior - sits between Wilson and
  Clopper-Pearson in width for most `p`, but can beat both at the boundary,
  i.e. near all-wins/all-losses). `exact`/`jeffreys` both require a true
  integer-count binomial, same restriction, same reason.
* **`sign-test`** - same CI, on the proportion of paired numeric records
  where the candidate beat the baseline (ties excluded). Nonparametric
  alternative to `mean-diff`: only the direction of each pair matters, not
  its magnitude. Also takes `--ci-method`.
* **`mean-diff`** - bootstrap confidence interval on `candidate - baseline`
  for paired numeric records. `--bootstrap-method percentile` (default),
  `--bootstrap-method basic` (reflects the percentile interval around the
  point estimate - simpler than BCa, but with no bias-correction of its
  own), or `--bootstrap-method bca` (bias-corrected and accelerated -
  corrects for a skewed diff distribution; `percentile` stays the default so
  existing CI numbers don't shift under you). `--resamples` controls the
  bootstrap sample count; `--seed` controls its RNG seed (fixed by default,
  so output is bit-identical across CI runs of the same input).
* **`elo`** - Elo rating difference from win/loss/draw `result` records
  (draws count as half a win, unlike `winrate`/`sign-test` which exclude
  them). Reported in Elo points, via the standard logistic model. Doesn't
  support `--ci-method exact`: its win rate is fractional (a draw is half a
  win), and Clopper-Pearson's coverage guarantee only holds for a true
  integer-count binomial.
* **`quantile-diff`** - bootstrap confidence interval on the `--quantile Q`
  (default `0.5`, the median; e.g. `0.95` for p95) quantile of
  `candidate - baseline` for paired numeric records - `mean-diff` generalized
  from the mean to an arbitrary quantile, for gates where the typical worst
  case matters more than the average (a latency p95/p99 regression gate, for
  instance). Same `--resamples`/`--seed` as `mean-diff`; only
  `--bootstrap-method percentile`/`basic` (not `bca` - the sample quantile is
  a non-smooth statistic, so BCa's jackknife acceleration has no solid
  footing for it; see [`docs/metrics.md`](docs/metrics.md)). One quantile per
  invocation - run `compare` again for a second quantile.
* **`relative-diff`** - bootstrap confidence interval on
  `(candidate - baseline) / baseline` for paired numeric records: proportional
  change relative to the baseline, instead of `mean-diff`'s absolute change.
  Requires every baseline to be strictly positive (`baseline > 0`) - zero and
  negative baselines are rejected as a configuration/data error, not silently
  computed against `abs(baseline)` or a shifted denominator. Same
  `--bootstrap-method percentile`/`basic`/`bca`/`--resamples`/`--seed` support
  as `mean-diff`. Reports `effect`/`ci_low`/`ci_high` as a ratio (`0.05` =
  +5%), rendered as a percentage in Markdown (`+5.0%`) - distinct from
  `winrate`'s percentage-point `pp` suffix. Not supported this round:
  `veridict power --metric relative-diff` and `--claim-correction` (see
  [`docs/metrics.md`](docs/metrics.md)).

Use `mean-diff` when one unit has the same practical meaning across all records. Use
`relative-diff` when proportional change is the pre-specified estimand and all baseline values are
strictly positive. They answer different questions (mean absolute change vs. mean proportional
change), not the same question at different precision - a few things worth knowing before picking
one:

* `relative-diff` is directional, not symmetric under swapping the arms: a +100% increase followed
  by a -50% decrease returns to the original value.
* It's unusable when any baseline is zero or negative (`compare` rejects the run outright rather
  than guessing what you meant).
* A small baseline can produce a large ratio from a small absolute change - know your data's
  baseline scale before trusting a lone large `relative-diff` outlier.
* The mean of each pair's own relative diff and the ratio of summed totals
  (`sum(candidate)/sum(baseline) - 1`) are generally different numbers when baselines vary in
  scale - `relative-diff` reports the former (see [`docs/metrics.md`](docs/metrics.md)).
* Picking whichever metric happens to pass *after* looking at both is exploratory analysis, not a
  pre-specified estimand - see [Scale-mismatch diagnostic](#scale-mismatch-diagnostic) below.

`winrate` and `sign-test` report `effect`/`ci_low`/`ci_high` centered on 0
(deviation from a 50/50 split); `elo` is centered on 0 by construction (an
even score is 0 Elo). All three compose directly with `--min-effect`.
`mean-diff`/`quantile-diff` report them in the input's own units; `relative-diff` reports them as a
ratio.

Every trial's `baseline_status`/`candidate_status` (`timeout`, `crash`,
`invalid`) is tallied and reported regardless of which metric you run, both
combined and broken down by which side failed (`failure_breakdown` in the
JSON report). `--failure-policy` (on `compare --metric winrate`/`--metric elo`
and `sprt`, all three `--sprt-variant` choices) controls whether a failure
also affects the *computation*, not just the report:

* **`report-only`** (default) - unchanged from before this flag existed: a
  failure is tallied, but a status-only record (no `result`) still
  contributes nothing to the metric either way. A record carrying *both* a
  failure status and a `result` still has that `result` counted.
* **`exclude`** - a failed side's `result` is never counted, even when
  present alongside the status. Only diverges from `report-only` in that
  mixed-field case - the common status-only case already behaves the same
  under both.
* **`loss`** - a failed side's outcome is synthesized instead of read from
  `result`: candidate failed -> `baseline_win`, baseline failed ->
  `candidate_win`, both failed -> `draw`. This *overrides* any literal
  `result` on the same record - a failure status is trusted over whatever
  `result` says next to it.

`exclude`/`loss` only apply to outcome-based metrics (`winrate`/`elo`);
requesting either with `--metric mean-diff`/`--metric sign-test`/
`--metric relative-diff` is a config error, not an arbitrary numeric penalty
for a failed numeric trial.

Requesting several `--metric` flags together scans the input once, feeding
every record to every requested metric, rather than one full pass per
metric.

## Scale-mismatch diagnostic

When you run `--metric mean-diff`, `data_quality.wide_baseline_scale` (advisory, never changes
`verdict`) fires if baselines are all positive but span at least a ~10x range - a sign the absolute
difference may be dominated by the largest-scale cases:

```console
$ veridict compare examples/mixed_scale_scores.jsonl --metric mean-diff --min-effect 0
...
"warnings": [
  "small sample: 15 paired trial(s), below the conventional 30-trial threshold for confidence-interval methods to be reliable",
  "baseline values span 1.8 orders of magnitude; absolute differences may be dominated by larger-scale cases. If the scientific question is proportional change, consider --metric relative-diff. Choose the metric before confirmatory analysis; switching after inspecting the verdict is exploratory."
]
```

If some baselines are zero or negative, the warning instead recommends a domain-justified
normalization rather than `relative-diff` (which isn't well-defined for that data either):

```text
baseline values vary widely, but some baselines are zero or negative, so
relative-diff is not well-defined for this dataset. Use a domain-justified
normalization rather than adding an arbitrary denominator offset.
```

**This diagnostic never recommends switching metrics based on the observed result.** It's computed
from baseline values alone - `positive_baseline_count`, `non_positive_baseline_count`,
`min_positive_baseline`, `max_positive_baseline`, `raw_orders_of_magnitude`, and (at 20+ positive
baselines, to keep one outlier from dominating) `robust_orders_of_magnitude`, all exposed as
`scale_diagnostics` on `mean-diff`/`relative-diff` reports - never candidate values, the effect
size, the CI, or the verdict. Choose `--metric mean-diff` or `--metric relative-diff` based on the
measurement goal before treating a result as confirmatory; switching metrics after inspecting a
verdict is exploratory, not a second confirmatory analysis, and should be validated on fresh or
held-out data. See [`docs/metrics.md`](docs/metrics.md) for the exact threshold and warning-text
rules.

## Multiple-comparison correction

Running several `--metric` flags together means several independent chances for *some individual
metric's own* `verdict`/`promotion` to clear its bar by luck alone. `--claim-correction bonferroni`/
`holm` keeps that per-metric risk, read as a family of simultaneous claims, at or below what a
single, uncorrected metric already has today - but it never touches the deployment-gate
`verdict`/`promotion` `compare` already reports for the whole run: requiring every metric to pass
already makes that combined result at least as conservative as a single metric, with no correction
needed for *that* guarantee (see [`docs/metrics.md`](docs/metrics.md)'s `--claim-correction`
section for the full reasoning). Instead, correction populates a separate
`family_adjusted_verdict`/`family_adjusted_promotion` per report (`unadjusted_verdict` also
appears alongside them, a deprecated compatibility alias that always equals `verdict` - read
`verdict` instead) and an overall `simultaneous_claims_promotion`. Not supported together with
`--cluster-by-id` or `--metric mean-diff`/`quantile-diff`/`relative-diff` (rejected as a
configuration error - see
`docs/metrics.md`). `--correction` is kept as a deprecated alias for one release. Default is
`none` - today's existing behavior, unchanged, unless you opt in.

```console
$ veridict compare examples/chess_engine_multi_metric.jsonl --metric winrate --metric elo --min-effect 0.02 --claim-correction bonferroni
{
  "schema_version": 1,
  "verdict": "pass",
  "promotion": "promoted",
  "simultaneous_claims_promotion": "not_promoted",
  "reports": [
    {
      "verdict": "pass",
      "promotion": "promoted",
      "metric": "winrate",
      "reason": "CI lower bound 0.0221 meets the pass threshold 0.0200",
      "correction_method": "bonferroni",
      "family_size": 2,
      "achieved_alpha": 0.045327562117809694,
      "adjusted_alpha_threshold": 0.025000000000000022,
      "unadjusted_verdict": "pass",
      "family_adjusted_verdict": "inconclusive",
      "family_adjusted_promotion": "not_promoted"
      // ...
    },
    {
      "verdict": "pass",
      "promotion": "promoted",
      "metric": "elo",
      "reason": "CI lower bound 15.3650 meets the pass threshold 0.0200",
      "correction_method": "bonferroni",
      "family_size": 2,
      "achieved_alpha": 0.016420872210740903,
      "adjusted_alpha_threshold": 0.025000000000000022,
      "unadjusted_verdict": "pass",
      "family_adjusted_verdict": "pass",
      "family_adjusted_promotion": "promoted"
      // ...
    }
  ]
}
```

`winrate`'s evidence is real but comparatively weak; split two ways it no longer clears its
corrected bar, so *that metric's own* `family_adjusted_verdict` drops to `inconclusive` and the
overall `simultaneous_claims_promotion` becomes `not_promoted`. The deployment-gate `verdict`/
`promotion` stay `pass`/`promoted` throughout - both metrics still individually pass at the
original, uncorrected confidence, and that's what the combined result is built from.
`--claim-correction holm` is uniformly more powerful than `bonferroni` for the same guarantee (it
would keep both metrics' family-adjusted claims at `pass` here); either can only downgrade an
unadjusted pass to inconclusive, never invent a fail.

## Report extras

Every report (`compare`, `sprt`, `matrix` alike) carries a `schema_version`
integer (currently `1`). It stays `1` across purely additive changes (new
fields, new enum variants); it only bumps when a field is removed or
renamed, so a machine consumer can key its parsing off this instead of
guessing from field presence. See [`schemas/`](schemas/) for a JSON Schema
per report/record type.

Every `compare` report also carries advisory fields that never affect
`verdict`:

* **`estimated_additional_trials`** - a rough estimate (`O(1/sqrt(n))` CI
  scaling) of how many more trials would likely turn an `inconclusive`
  result decisive, assuming the effect size itself doesn't move. `null`
  when there's nothing useful to suggest - already decided, zero trials, or
  the effect sits *inside* the pass/fail threshold band (the "dead zone"):
  shrinking the CI around a point estimate that's already in the dead zone
  can never cross either boundary, no matter how much data you add.
  Also `null` under `--cluster-by-id`: the search binary-searches wilson/
  jeffreys/exact, none of which describe a cluster bootstrap CI's width at a
  hypothetical `n`, and the independent unit under clustering is the
  cluster, not the trial.
  Treat the number as "roughly this many, plausibly more," not a
  guarantee - it has a documented, quantified bias (e.g. an ~18%
  under-estimate at n=100 for one verified case).
* **`warnings`** - human-readable data-quality flags, empty when there's
  nothing to flag: a tiny sample (under 30 paired trials), an excessive
  failure rate (over 20% timeout/crash/invalid), for `elo`, a draw-heavy run
  (over 50% draws leaves few decisive outcomes to rate from), the measured
  effect being smaller than the CI's own half-width (plausibly noise around
  zero), for `quantile-diff`, too few expected observations in the thinner
  tail at the requested quantile (fewer than 10, `paired_count * min(q, 1-q)`),
  for `mean-diff`, baselines spanning a wide range (see [Scale-mismatch
  diagnostic](#scale-mismatch-diagnostic) above), or, in unpaired mode, one
  `id` being repeated 3+ times among 10+ id-tagged trials (a sign the same
  test case was logged multiple times rather than run that many independent
  times - silent when every `id` appears exactly twice, the common case of
  forgetting `--paired-by-id`).
* **`data_quality`** - the same flags as `warnings`, as booleans
  (`tiny_sample`, `high_failure_rate`, `draw_heavy`, `effect_within_noise_floor`,
  `low_id_diversity`, `wide_baseline_scale`) rather than strings, for a
  machine consumer that wants to branch on a flag instead of parsing prose.
  Added alongside `warnings`, not a replacement - both are always present.
* **`scale_diagnostics`** - `mean-diff`/`relative-diff` only: the raw
  distribution of baseline values (`positive_baseline_count`,
  `non_positive_baseline_count`, `min_positive_baseline`,
  `max_positive_baseline`, `raw_orders_of_magnitude`,
  `robust_orders_of_magnitude`) `wide_baseline_scale` is computed from - see
  [Scale-mismatch diagnostic](#scale-mismatch-diagnostic).

See [`docs/metrics.md`](docs/metrics.md) for the full detail on every method
above, including assumptions and known failure modes.

## Validity, strength, and promotion

Every `compare`/`sprt` report carries three separate fields instead of collapsing everything into
`verdict`:

* **`validity`** (`valid`/`invalid`) - whether this run's data is trustworthy enough to read a
  verdict off of at all. `invalid` means a hard technical-failure cap was breached (see below);
  it is not the same claim as `verdict: inconclusive` ("the evidence was too weak to decide" -
  still `valid` data).
* **`verdict`** (`pass`/`fail`/`inconclusive`) - unchanged from before this existed. Forced to
  `inconclusive` whenever `validity` is `invalid`, so a technical-failure-corrupted run can never
  surface as a clean `pass`/`fail` - see the `--failure-policy loss` case below.
* **`promotion`** (`promoted`/`not_promoted`) - the one field a deployment pipeline should
  actually gate on: `promoted` only when `validity` is `valid` and `verdict` is `pass`.

`--max-timeouts`/`--max-crashes`/`--max-invalid` (on both `compare` and `sprt`) set hard,
zero-tolerance-style caps on the same `timeout`/`crash`/`invalid` counts every report already
tallies - unset (the default) is uncapped, today's existing behavior. Breaching a cap sets
`validity: invalid` regardless of how many clean trials surround it - unlike
`data_quality.high_failure_rate` (a rate-based, purely advisory warning that never changes
`verdict`), this is an absolute count, so a single crash can matter even in a run of thousands:

```bash
veridict compare examples/chess_engine_with_crashes.jsonl --metric winrate --min-effect 0.02 \
  --failure-policy loss --max-crashes 0
```

Under `--failure-policy loss`, a crash synthesizes a `baseline_win` - enough crashes can tip a
numeric `winrate` verdict to `fail` even though the real cause was infrastructure, not strength.
`--max-crashes 0` catches that before it reaches the report: `validity` becomes `invalid`,
`verdict` is forced back to `inconclusive`, and `promotion` is `not_promoted`, with `reason`
naming the breached cap. A multi-metric `compare` run's overall `validity`/`promotion` are
`invalid`/`not_promoted` if *any* individual metric's report is invalid.

## SPRT

`veridict sprt` is a separate mode from `compare`: instead of an effect
size and a confidence interval checked against a threshold, it accumulates
a log-likelihood ratio and becomes decisive as soon as the evidence
crosses one of two boundaries derived from `--alpha`/`--beta`. For
`wald`/`trinomial` this means: re-invoke `sprt` on a growing input and it
recomputes the LLR fresh from the whole file each time, so a caller
stopping as soon as a call returns `pass`/`fail` gets sequential-test
behavior across that *sequence* of invocations. `--sprt-variant
pentanomial` goes further and needs no such caller discipline: a *single*
call replays completed pairs in the order they complete and stops
internally at the first pair that decides, so feeding it a file that
keeps going past that point changes nothing (see below). `pass` means "confident the
candidate is at least `--elo1` points stronger"; `fail` means "confident
it's at most `--elo0` points stronger"; `inconclusive` means "keep
collecting data". `--alpha`/`--beta` are its actual guaranteed false
positive/negative rates, not tunable knobs on a report - there's no
`--min-effect`/`--confidence` for this subcommand. Three variants
(`--sprt-variant`):

* **`wald`** (default) - the classic two-outcome SPRT over decisive
  (non-draw) `result` records only; draws carry no information about which
  Elo hypothesis is true under this model and are excluded from the LLR
  entirely. Hypotheses are `--elo0`/`--elo1`, in standard logistic Elo.
* **`trinomial`** - a draw-aware generalized LLR test (the BayesElo
  parameterization historically used by chess-engine testing tools like
  Fishtest). Estimates the draw rate as a nuisance parameter from the
  pooled win/draw/loss counts, which lets it converge faster than `wald` on
  draw-heavy data. **Units are BayesElo, not logistic Elo** - the two only
  coincide when the estimated draw rate is exactly zero - so hypotheses are
  given via separate `--belo0`/`--belo1` flags rather than reinterpreting
  `--elo0`/`--elo1`. The estimated draw rate (`drawelo`) is reported in the
  output for transparency, since it's estimated from the same data being
  judged.
* **`pentanomial`** - a paired-game test (Fishtest's `LLR_logistic`): two
  records sharing an `id` (same opening, colors swapped) are combined into
  one of 5 outcome buckets by their pair's combined candidate score
  (`0`/`0.5`/`1`/`1.5`/`2`), instead of netted down to a single win/loss/
  draw. **Always requires `--paired-by-id`** - a 5-value pair score has no
  meaning for a lone game, so an id that doesn't appear exactly twice is a
  hard error, not silently treated as an unpaired sample. Hypotheses are
  `--elo0`/`--elo1`, the same logistic Elo scale as `wald` (this model has
  no drawelo-style nuisance parameter). Unlike running `trinomial` on twice
  as many ungrouped games, this captures the *negative correlation* between
  a pair's two games (an unbalanced opening helps one side in one game and
  hurts it in the other) - see [`docs/metrics.md`](docs/metrics.md) for why
  that correlation, not just draw-awareness, is what lets it converge in
  fewer pairs on real paired-game data. The report adds `sprt_variant`,
  `pentanomial_counts` (the 5-bucket breakdown), `raw_trial_count`,
  `paired_count` (pairs actually analyzed, up to the sequential stopping
  point), `available_paired_count` (total pairs present in the input),
  `stopping_pair_count`/`stopping_reason` (where and why the walk
  stopped), and `ignored_pairs_after_stop` (pairs present in the input but
  completed after the stopping point, and so never analyzed).

See [`docs/metrics.md`](docs/metrics.md) for the full mechanics of all
three variants, including the BayesElo/logistic-Elo unit conversion.

`sprt` also accepts `--failure-policy` (see [Metrics](#metrics) for the exact
`report-only`/`exclude`/`loss` semantics) - applies identically across all three
`--sprt-variant` choices, including `pentanomial`: a `loss`-synthesized outcome nets
against its pair partner the same way any other outcome would.

`--sprt-variant pentanomial` also accepts:

* **`--min-paired-ids`** (must be `>= 1`) - the walk never even evaluates a boundary crossing
  before this many pairs have completed, so an early crossing on too little data can't be
  "remembered" once the minimum is reached: the decision is always the LLR at whichever pair the
  walk actually stops on, using every pair completed up to and including that point. Below the
  minimum, `verdict` is `inconclusive` regardless of where the accumulated LLR sits; `validity` is
  untouched (this is "not enough data yet," not "the run was corrupt").
* **`--max-paired-ids`** (must be `>= --min-paired-ids` when both are given) - if the walk reaches
  this many completed pairs without a boundary crossing, it stops there with an `inconclusive`
  verdict (no truncated-SPRT decision rule) and `reason`/`stopping_reason` note the cap was hit.
  Pairs completed after this point, even if present in the input, never affect the verdict, LLR,
  or bucket counts.
* **`--require-complete-pairs`** - extends `pentanomial`'s existing unconditional "every id
  appears exactly twice, no exceptions" pairing to `wald`/`trinomial` too (requires
  `--paired-by-id`; a lone id is normally tolerated there as an ordinary unpaired sample). A
  documented no-op for `pentanomial` itself, which is already this strict regardless of the flag.

The report echoes `--min-paired-ids`/`--max-paired-ids`/`--require-complete-pairs` as
`min_paired_ids`, `max_paired_ids`, and `require_complete_pairs`.

## Time-sensitive testing

`veridict sprt` answers "is the candidate decisively better?" `veridict time-sensitive` answers a
different question: given a reward that favors an *early* rejection (a deadline, a decaying value
of waiting), which betting policy maximizes expected reward under the alternative - while keeping
the exact same type-I error guarantee `sprt` has, for *any* betting choice? It's an independent
addition, not a replacement: `sprt`/`compare`/`power`'s behavior, JSON, and public API are
unchanged by this feature existing.

> This implementation is independently derived from the mathematical framework in:
>
> E. Clerico, T. Wegel, I. Azangulov, and P. Rebeschini, "Time-sensitive anytime-valid testing,"
> arXiv:2605.06521v1, 2026. Paper licensed under [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/).
>
> The implementation is modified and independent. The paper's authors do not endorse or maintain
> this software.

**v1 scope is deliberately narrow: Bernoulli simple-vs-simple only** - a fixed null `p0` and
alternative `p1` (`0 < p0 < p1 < 1`), one-sided rejection. `p0`/`p1` are *decisive-observation-
conditional* success probabilities, the same convention `sprt --sprt-variant wald` uses for
`elo0`/`elo1` under the hood: a `draw` result advances the trial count and the reward-schedule
clock, but carries no information about which hypothesis is true, so it never moves the wealth
process. Trinomial/pentanomial (draw-aware) time-sensitive policies, composite hypotheses, and
online `p1` estimation are out of scope for this round - see
[`docs/research-map.md`](docs/research-map.md).

Three policies (`--policy`):

* **`gro`** (growth-rate-optimal) - the classical anytime-valid baseline: bet `p1` every trial,
  ignoring the reward schedule entirely. Betting-equivalent to `sprt`'s own Wald log-likelihood-
  ratio walk (both bet the exact likelihood ratio each trial), though implemented independently
  (`sprt`'s batch/aggregate-count API has no notion of time-stamped rejection - see
  `veridict::time_sensitive`'s module doc). `gro` is the large-`time_scale` limit of `edo`.
* **`bellman`** - a numerical approximation of the time-sensitive-optimal policy on a finite
  `(action_grid_size, wealth_grid_size)` grid, for any reward schedule. Reported as
  `bellman_grid_approximation`, never "optimal": it's a grid approximation, not a proof of
  optimality on the continuous action/wealth space.
* **`edo`** (exponential-decay-optimal) - a closed-form *stationary* (time-independent)
  approximation, valid only for `--reward exponential`. Cheap (no grid at all), and more
  aggressive than `gro` the tighter `--time-scale` is. Reported as `edo_stationary_approximation`,
  never "Bellman-optimal": ignoring how close `t` is to a wealth-dependent effective horizon is
  exactly what makes it stationary and cheap, not exact.

**Why the approximation in `bellman`/`edo` never weakens the guarantee.** Every policy here
reduces to picking an action `a` in `(0,1)` each trial and multiplying wealth by a Bernoulli
e-variable that is valid under H0 for *every* `a`, not just the "correct" one - so grid
resolution, floor truncation, and closed-form approximation error can only ever change how *fast*
wealth grows under the true alternative (optimality), never whether crossing `1/alpha` under the
null happens at rate `<= alpha` (validity). This separation is proved exactly (not simulated) for
small horizons by exhaustive `2^T`-path enumeration, and checked at realistic scale (`T=400`) by
Monte Carlo calibration - see `time_sensitive`'s own test suite.

Reward schedules (`--reward`, or `--reward-schedule FILE` for a custom one):

```console
$ veridict time-sensitive examples/chess_engine_time_sensitive.jsonl \
    --p0 0.50 --p1 0.55 --alpha 0.05 \
    --policy bellman --reward hard-deadline --deadline 400
{
  "schema_version": 1,
  "verdict": "pass",
  "validity": "valid",
  "promotion": "promoted",
  "method": "bellman_grid_approximation",
  "policy_kind": "bellman",
  "p0": 0.5,
  "p1": 0.55,
  "alpha": 0.05,
  "reward_kind": "hard_deadline",
  "reward_parameters": { "deadline": 400 },
  "trial_count": 313,
  "rejection_time": 313,
  "reward_at_rejection": 1.0,
  "planned_expected_reward_under_p1": 0.645940711706933,
  ...
}
```

```console
$ veridict time-sensitive examples/chess_engine_time_sensitive.jsonl \
    --p0 0.50 --p1 0.55 --alpha 0.05 \
    --policy edo --reward exponential --time-scale 800 --horizon 3200
{
  "verdict": "pass",
  "method": "edo_stationary_approximation",
  "reward_kind": "exponential_decay",
  "reward_parameters": { "time_scale": 800.0, "horizon": 3200 },
  "trial_count": 218,
  "rejection_time": 218,
  "reward_at_rejection": 0.7614734291752052,
  ...
}
```

A custom schedule (`examples/time_sensitive_reward_schedule.json`) declares reward tiers by
trial-count breakpoint; the tail (`after`) value must be `0.0` - `R(t) -> 0` as `t -> infinity` is
part of what makes this a well-posed finite-horizon problem, not a value this format can encode
otherwise:

```json
{
  "rewards": [
    {"until": 400,  "value": 1.0},
    {"until": 1600, "value": 0.4},
    {"until": 3200, "value": 0.1},
    {"after": 3200, "value": 0.0}
  ]
}
```

```console
$ veridict time-sensitive examples/chess_engine_time_sensitive.jsonl \
    --p0 0.50 --p1 0.55 --alpha 0.05 \
    --policy bellman --reward-schedule examples/time_sensitive_reward_schedule.json
{
  "verdict": "pass",
  "reward_kind": "tabulated",
  "reward_parameters": { "horizon": 3200, "entries": 3201 },
  "trial_count": 312,
  "rejection_time": 312,
  "reward_at_rejection": 1.0,
  ...
}
```

**Decision semantics are one-sided.** `pass` means wealth crossed `1/alpha`; `inconclusive` means
the reward schedule's horizon was reached first - never read as a statistical fail, since there is
no lower rejection boundary in this model (a non-crossing is an absence of evidence for H1, not
evidence for H0). `promotion` is `promoted` only when `validity: "valid"` and `verdict: "pass"`,
the same rule every other subcommand uses. `--failure-policy`/`--max-timeouts`/`--max-crashes`/
`--max-invalid` (see [Metrics](#metrics)) work exactly as they do for `sprt`, forcing
`validity: "invalid"` and `verdict: "inconclusive"` on a cap breach.

**Exit codes are `0` (pass) / `2` (inconclusive) / `3` (config/input error) - never `1`.** There is
no two-sided fail in this model, so exit code `1` is never returned by this subcommand.

`reward_parameters`/`planned_expected_reward_under_p1` deserve one caveat: the latter is computed
*exactly* (via the same grid the policy itself was built on, not simulated) under an all-decisive
idealization - the value recursion has no draw-rate input, so on a draw-heavy stream the realized
reward will lag this number, since draws consume reward-schedule time the recursion never modeled.
`notes` in every report spells this out alongside the scope/approximation caveats above.

## Comparison matrix

`veridict matrix` compares more than two candidates at once and tabulates
pairwise Elo differences. It's report-only (no verdict, always exits 0 on
success): there's no single pass/fail for a whole matrix. Two ways to feed
it data, freely combinable in one run:

* **Legacy**: one file per candidate, each measured against the *same
  shared baseline*, using the same `result`-field win/loss/draw records as
  `--metric elo`/`--metric winrate`.
* **`--matches`** (repeatable): head-to-head records between named
  competitors - `{"id": ..., "a": "...", "b": "...", "result":
  "a_win"|"b_win"|"draw"}` - so candidates can play each other directly,
  not just the shared baseline. Use the literal name `"baseline"` in `a`/`b`
  to connect this data to the baseline node implied by the legacy files.

If the resulting graph is still topologically a star (every game touches
baseline, whichever source it came from), `matrix` uses a closed form: each
candidate's rating is exactly its own Elo-vs-baseline (the Bradley-Terry
MLE on a star graph has no shared terms to solve jointly). Once real
candidate-vs-candidate games are present, it fits a general Bradley-Terry
model instead (an iterative solver over the whole graph). Either way, each
matrix cell is marked:

* **`direct`** - a real head-to-head edge exists between that row and column.
* **`inferred`** (`*` in the Markdown table) - both are rated and
  comparable, but never played each other; a model-extrapolated
  `elo_i - elo_j`.
* **`disconnected`** (`n/a` in the Markdown table) - no path connects them
  (e.g. two separate head-to-head clusters that never share a competitor).
  There is no finite rating difference between them, not merely an
  uncertain one - `elo_diff` is `null`, not a guess.

Star-graph/legacy cells keep their real Wilson interval, as before. General-
graph matrix cells (`direct`/`inferred`) get a real bootstrap CI too: each
resample redraws every edge's tally from its own observed win/loss/draw
proportions and refits the whole graph, and `ci_low`/`ci_high` come from
`elo_i - elo_j` across resamples that kept the pair in the same component.
`--resamples` (default 2,000), `--seed`, and `--bootstrap-method percentile`
(default) or `--bootstrap-method basic`/`bca` - same three methods and same
meaning as `compare`'s flag of the same name - control this (all ignored in
star-graph mode, which keeps its closed-form Wilson interval regardless). A
cell can
still show `ci_low`/`ci_high: null` even though `elo_diff` isn't - that
means the pair is connected in the observed data, but the connection is too
fragile under resampling (fewer than 90% of resamples kept them in the same
component) for a reliable interval, deliberately reported as "no CI" rather
than a falsely narrow one. `CandidateSummary`'s own `ci_low`/`ci_high` stay
`null` in general-graph mode regardless: an individual rating is only
meaningful relative to its component's arbitrary reference competitor, so a
CI on it would be misleading in a way `elo_i - elo_j`'s CI isn't.

## Plan

`veridict plan` takes the exact same input as `matrix` (legacy files and/or `--matches`, freely
combinable) plus a required `--min-elo <f64>` - the Elo gap worth being able to detect - and
recommends which pairs would most benefit from more trials, ranked most-uncertain first:

```console
$ veridict plan candidate_a.jsonl candidate_b.jsonl --min-elo 100
{
  "schema_version": 1,
  "min_elo": 100.0,
  "recommendations": [
    { "row": "baseline", "col": "candidate_b", "status": "direct",
      "current_ci_half_width": 254.6, "estimated_additional_trials": 53, "note": null },
    { "row": "candidate_a", "col": "candidate_b", "status": "inferred",
      "current_ci_half_width": 274.4, "estimated_additional_trials": 52, "note": null },
    { "row": "baseline", "col": "candidate_a", "status": "direct",
      "current_ci_half_width": 102.4, "estimated_additional_trials": 4, "note": null }
  ]
}
```

It's report-only, like `matrix`: no verdict, always exits `0` on success. Each recommendation is
one `matrix` cell, with:

* **`current_ci_half_width`** - the cell's current CI half-width, or `null` when no CI exists
  yet to narrow at all (a `disconnected` pair, or a `direct`/`inferred` cell too fragile under
  resampling for a reliable CI - see `matrix`'s docs above for both).
* **`estimated_additional_trials`** - `0` when the current CI already meets `--min-elo`; `null`
  alongside a `note` explaining why for the same cells `current_ci_half_width` is `null` for.
  Disconnected pairs sort first in the list (no estimate is even possible until a game connects
  them - a stronger need than any finite-but-wide CI), then the rest by largest estimate first.

Dropped from an earlier, broader idea: a `--budget N`/`--goal identify-best` constrained
allocator. No real algorithm for either exists in this codebase yet - see
[`docs/research-map.md`](docs/research-map.md) for what's deliberately deferred.

## Power

`veridict power` estimates how many trials `compare --metric winrate/sign-test/elo` would need
for a target probability (power) of reaching a passing verdict - *before* running any of them. No
input file: a pure calculation from flags.

```console
$ veridict power --metric elo --min-effect 20 --assume-effect 35 --target-power 0.80
{
  "schema_version": 1,
  "metric": "elo",
  "ci_method": "wilson",
  "min_effect": 20.0,
  "assume_effect": 35.0,
  "confidence": 0.95,
  "target_power": 0.8,
  "estimated_trials": 4281,
  "achieved_power": 0.8043871725361499,
  "method": "exact_binomial_search",
  "notes": [
    "Assumes the true effect is exactly assume_effect; a smaller real effect needs more trials than this number, not fewer - this is a design estimate for how much data to collect, not a guarantee about what a real run will show."
  ]
}
```

Two effect values are **both required**, and `--assume-effect` must exceed `--min-effect`:

* **`--min-effect`** - the pass bar, identical meaning to `compare --min-effect`/`--pass-above`.
* **`--assume-effect`** - the true effect actually being powered for. Evaluating power with the
  true effect set equal to the pass bar only recovers the CI's own miscoverage at that boundary
  (`≈ 1 - confidence`) - flat, and it never climbs toward `--target-power` no matter how many
  trials you add. See [`docs/metrics.md`](docs/metrics.md)'s `power` section for the reasoning,
  not just the rule.

`estimated_trials` is found by an *exact* search (`sum Binomial_pmf(n, p1, k) * [CI_lower(k,n) >=
p0]`, not a textbook approximation) against the same real `wilson`/`exact`/`jeffreys` CI functions
`compare` itself uses (`elo` accepts only `wilson`, same as `compare --metric elo`).
`--paired-by-id` is accepted but doesn't change the number - see the docs section for why.

`--metric mean-diff` is a closed-form calculation instead of that search - there's no closed-form
CI-width-at-n function for a bootstrap CI, so it needs an assumed standard deviation of the paired
difference from you, either directly (`--assume-sd`) or estimated from real pilot data
(`--pilot FILE`):

```console
$ veridict power --metric mean-diff --min-effect 0.02 --assume-effect 0.10 --assume-sd 0.15
{
  "schema_version": 1,
  "metric": "mean-diff",
  "ci_method": "normal",
  "min_effect": 0.02,
  "assume_effect": 0.1,
  "confidence": 0.95,
  "target_power": 0.8,
  "estimated_trials": 28,
  "achieved_power": 0.805703217265413,
  "method": "normal_approximation_closed_form",
  "notes": [
    "This is a normal approximation of compare --metric mean-diff's real bootstrap decision rule, not an exact search against it: there is no real data pre-experiment to bootstrap, so a normal model of the paired differences is the standard assumption. For skewed real diffs the bootstrap CI and this estimate will diverge - treat this as a design estimate for how much data to collect, not a guarantee about what a real run will show.",
    "assume_sd is the standard deviation of the paired difference (candidate - baseline), not either arm's own standard deviation - using an arm's SD here would understate the true variance for anything but a perfectly correlated pair."
  ],
  "assume_sd": 0.15,
  "sd_source": "assume-sd"
}
```

Or estimate that same standard deviation from real pilot data instead of guessing it:

```console
$ veridict power --metric mean-diff --min-effect 0.02 --assume-effect 0.10 --pilot examples/pilot_scores.jsonl
```

**`assume_sd` is the standard deviation of the paired *difference* (`candidate - baseline`), not
either arm's own standard deviation** - the classic paired-design mislabeling risk; getting this
wrong silently corrupts every number downstream. See
[`docs/metrics.md`](docs/metrics.md)'s `power --metric mean-diff` section for the formula, why
`z_conf` must be the two-sided confidence quantile (the same correctness point behind `power`'s
own two-effect-value design and `--claim-correction`'s `alpha/2` family target), and `--pilot`'s
tiny-sample/small-pilot caveats.

`--sprt` switches to a structurally different question: Wald's SPRT already guarantees its
`alpha`/`beta` error rates regardless of `n`, so there's no target power to search a sample size
for. Instead, this reports the *expected* number of trials to a decision (Wald's own "Average
Sample Number") under each hypothesis, given the same `--elo0`/`--elo1`/`--alpha`/`--beta` `sprt`
itself takes:

```console
$ veridict power --sprt --elo0 0 --elo1 20
{
  "schema_version": 1,
  "elo0": 0.0,
  "elo1": 20.0,
  "alpha": 0.05,
  "beta": 0.05,
  "expected_trials_under_h0": 1601,
  "expected_trials_under_h1": 1603,
  "method": "wald_asn_approximation",
  "notes": [
    "expected_trials_under_h0/h1 are the two endpoint cases (the true strength sitting exactly at elo0 or elo1) - a real candidate whose true strength lies between elo0 and elo1, the common case since you're running SPRT precisely because that strength is unknown, needs substantially more trials than either endpoint: a Wald SPRT's expected sample size peaks near the midpoint between the two hypotheses, not at either one. Budget above these two numbers, not at them, when the candidate's true strength is genuinely uncertain.",
    "Wald's classical Average Sample Number approximation - ignores \"overshoot\" (the LLR's excess past a boundary at the moment of stopping), so a real run typically needs somewhat more trials than this number in practice.",
    "Counts decisive trials only (same as --sprt-variant wald itself) - a draw-heavy testcase needs more real games than this number, since draws don't move the LLR at all. Use --sprt-variant trinomial/pentanomial for draw-heavy testing."
  ]
}
```

**These two numbers are the optimistic endpoints, not the worst case.** Expected sample size is
highest when the candidate's true strength lies *between* `elo0` and `elo1` - a real, measured
effect (about 1.6x either endpoint at this same elo0/elo1/alpha/beta, see
`tests/calibration/sprt_asn_calibration.rs`), not a small correction like the overshoot caveat
below it. Budget above these numbers when the candidate's true strength is genuinely uncertain.

Add `--horizon N` to also ask the sharper planning question: "if I cap this gate at N trials no
matter what, how often will I still have no verdict?" - a Monte Carlo estimate (2,000
replications, fixed seed) evaluated at that same worst-case midpoint strength, not either
endpoint:

```console
$ veridict power --sprt --elo0 0 --elo1 20 --horizon 3000
{
  ...
  "horizon": 3000,
  "probability_no_decision_by_horizon": 0.306,
  "notes": [ ... ]
}
```

This is a planning number for the *next* gate's trial budget/cutoff, not a stopping rule - a real
`veridict sprt` run's own `--alpha`/`--beta` boundaries already fully determine when it stops.

See [`docs/metrics.md`](docs/metrics.md)'s `power --sprt` section for the formula, its citation,
and the *measured* overshoot bias (not just a cited caveat).

## Verify run

`veridict compare`/`sprt` judge a run's *statistics*; they have no way to check whether the raw
input those statistics were computed from is itself structurally sound. `veridict verify-run`
closes that gap: given a declared `manifest.toml` and the actual `games.jsonl`, it checks pairing,
ordering, contamination, and opaque-identifier consistency *before* you trust any verdict computed
from the same file.

```console
$ veridict verify-run examples/manifest.toml examples/games_with_contamination.jsonl
{
  "schema_version": 1,
  "validity": "invalid",
  "reason": "3 structural violation(s) found; see `violations`.",
  "violations": [
    {
      "check": "experiment_contamination",
      "id": "stray-op9",
      "lines": [5],
      "field": "experiment_id",
      "detail": "record's experiment_id 'exp-2025-11-02-old-run' does not match manifest's 'exp-2026-07-26-001'"
    },
    ...
  ],
  ...
}
```

**Self-consistency only.** Every hash/identifier field (`dataset_sha256`, `binary_sha256`,
`weight_sha256`, `config_sha256`, `experiment_id`, and the rest of the shared envelope - see
below) is an opaque, caller-supplied string: `verify-run` compares `manifest.toml`'s declared
value against what each record in `games.jsonl` repeats, looking for drift. It never opens,
hashes, or interprets an actual binary/weight/corpus/config file itself - that stays the caller's
responsibility, matching this project's domain-agnostic, "judge results, don't produce them"
design (see [`docs/research-map.md`](docs/research-map.md)).

Six checks run unconditionally over the whole input, collecting *every* violation rather than
stopping at the first one:

* **`pair_completeness`** - every `id` present in `games.jsonl` must appear in exactly 2 records.
* **`global_index_uniqueness`** - a caller-declared `global_index` field must not repeat.
* **`role_consistency`** - the two records sharing an `id` must declare two different `role`
  values (the domain-agnostic stand-in for "color/side reversed within a pair" - whatever `role`
  means is the caller's convention, not veridict's), and only one `role`-pairing convention may
  be used across the whole run.
* **`schedule_order`** - the actual first-occurrence order of gated (`include_in_gate != false`)
  pair ids must match `manifest.toml`'s declared `schedule`.
* **`experiment_contamination`** - a record's `experiment_id`, if present, must match the
  manifest's - catches games from a different run mixed into this one.
* **`environment_consistency`** - `dataset_sha256`/`binary_sha256`/`weight_sha256`/
  `config_sha256`, each checked independently: a record's value, if present, must match the
  manifest's declared value - catches a binary/weight/dataset swap or a config change mid-run.

A check whose backing field never appears anywhere in the manifest/records is honestly reported
in `checks_skipped` (and mirrored in `warnings`) - never a silent pass (false reassurance) and
never a hard failure (which would make the command unusable until every optional field is
emitted). `baseline_status`/`candidate_status` (the same `ok`/`timeout`/`crash`/`invalid`
vocabulary every other subcommand uses) are tallied into `failure_breakdown`/`timeouts`/
`crashes`/`invalid` for visibility, not checked against a cap - that's `compare`/`sprt`'s
`--max-timeouts`/`--max-crashes`/`--max-invalid` job.

`report.paired_count` is the *formal, gated* pair count: `id`-groups of exactly 2 records,
excluding burn-in pairs (either record marked `include_in_gate: false`) - a burn-in pair never
votes on the gated run's pairing convention or counts toward this number, even though
`pair_completeness`/`role_consistency` still check it like any other pair.

`violations` is sorted deterministically, then capped at 500 entries so a badly corrupted run
can't produce a report proportional to the whole input; `violation_count` always holds the exact
total and `violations_truncated` is `true` whenever the array above is a sample, not the full
list.

**Exit codes are `verify-run`'s own, not the usual verdict-based ones**: `0` if no violation was
found, `1` if one or more were (`validity: "invalid"` in the report - there's no "inconclusive"
for a structural invariant; it either holds or it doesn't, so `verdict`'s three-way shape doesn't
apply here), `3` only for a genuine parse/config error where no report can be produced at all
(malformed `manifest.toml`, an unsupported `manifest_schema_version`, malformed `games.jsonl`,
empty input, or a manifest that declares nothing for the three manifest-dependent checks above to
check against).

**The shared experiment envelope.** `experiment_id`/`candidate_id`/`baseline_id`/`lineage_id`/
`dataset_sha256`/`split_sha256`/`teacher_manifest_sha256`/`binary_sha256`/`weight_sha256`/
`init_seed`/`split_seed`/`shuffle_seed`/`schema_version`/`validity` form a cross-repo contract
(see [`schemas/experiment-envelope.schema.json`](schemas/experiment-envelope.schema.json)),
meant to be vendored verbatim by any pipeline of tools around veridict so a downstream tool can
join a verdict back to its own provenance store without needing to understand another tool's
identifiers. `manifest.toml`'s full shape (the envelope's fields plus `config_sha256`/`schedule`,
which aren't part of the shared envelope) is
[`schemas/manifest.schema.json`](schemas/manifest.schema.json); `games.jsonl`'s per-record shape
for `verify-run` is [`schemas/verify-run-record.schema.json`](schemas/verify-run-record.schema.json)
(a separate schema from [`schemas/input-record.schema.json`](schemas/input-record.schema.json) -
`verify-run` computes no metric, so it needs no `baseline`/`candidate`/`result` fields, and needs
several fields those other subcommands have no room for).

Note the same caveat as everywhere else in this codebase: CSV line numbers are record indices, not
physical file lines, if a quoted field embeds a newline - since "point me at the bad record" is
this command's entire deliverable, prefer JSONL for `games.jsonl`.

## Paired testcases

`--paired-by-id` (on `compare`, `sprt`, and `matrix`) treats two records
sharing the same `id` as one testcase played twice - e.g. re-run with roles
swapped to cancel that testcase's own bias - and combines them into a
single net observation instead of two independent ones:

* `winrate`/`elo`: net by total points across the pair (win=1, draw=0.5,
  loss=0, the standard "paired game" convention) - `>1` is a net candidate
  win, `<1` a net baseline win, exactly `1` a net draw.
* `mean-diff`/`quantile-diff`/`sign-test`: net by averaging the pair's two diffs.
* `relative-diff`: each raw record is relative-transformed
  (`(candidate - baseline) / baseline`) *first*, then the pair's two ratios are netted by
  averaging - not by averaging baseline/candidate first and taking one ratio of the averages (see
  [`docs/metrics.md`](docs/metrics.md)).

An `id` used only once is an ordinary unpaired sample (mixing paired and
unpaired testcases in one file is fine). Three or more records sharing an
`id` is rejected as a data error, not silently truncated to a pair. Without
`--paired-by-id`, a duplicate `id` on `mean-diff`/`quantile-diff`/`sign-test`/
`relative-diff` records is still rejected outright, same as before this flag existed.

**`sprt --sprt-variant pentanomial` is the one exception to "an id used once
is an ordinary unpaired sample":** it keeps the pair's full 5-value score
instead of netting it (see [SPRT](#sprt)), which has no meaning for a lone
game, so it always requires `--paired-by-id` and rejects any id that
doesn't appear exactly twice - a lone id is a hard error here, not treated
as an unpaired sample the way it is everywhere else.

## Clustered testcases

`--cluster-by-id` (`compare --metric winrate`/`--metric elo` only) treats every id as a group of
*correlated* trials instead of netting an exact pair - the same opening/testcase replayed several
times, not just twice. Where `--paired-by-id` combines a pair into one observation,
`--cluster-by-id` keeps every record but switches the CI from the closed-form method to a cluster
bootstrap: each resample draws whole id-groups with replacement, so trials that share a common
source of correlation don't each count as independent evidence. The two flags are mutually
exclusive (they're different treatments of a repeated id, not one stricter than the other).

```console
$ veridict compare openings.jsonl --metric winrate --cluster-by-id
```

The concrete failure mode this catches: 20 openings, 5 games each, half the openings strongly
favor the candidate (5-0) and half strongly favor the baseline (1-4) - same aggregate 60/100
candidate wins either way. Treated as 100 independent trials, the naive Wilson CI reads as a
confident **pass** (and separately fires `low_id_diversity`, itself a hint that the "N independent
trials" assumption doesn't hold):

```console
$ veridict compare openings.jsonl --metric winrate
"verdict": "pass",
"ci_low": 0.0020, "ci_high": 0.1906
```

With `--cluster-by-id`, the same data honestly reflects that there are really only ~20
independent units of evidence (which opening you drew, not game-to-game noise, dominates the
outcome) - a wider CI and the correct **inconclusive**:

```console
$ veridict compare openings.jsonl --metric winrate --cluster-by-id
"verdict": "inconclusive",
"ci_low": -0.0600, "ci_high": 0.2600,
"cluster_count": 20, "max_cluster_size": 5,
"effective_sample_size": 30.2, "design_effect": 3.31
```

`design_effect` (`Var(cluster bootstrap) / Var(i.i.d. bootstrap)` on the same data, both from the
same bootstrap family so they're directly comparable - see `docs/metrics.md`) is 1.0 when
clustering has no measurable effect; here it's 3.31, meaning the naive CI would have understated
the true uncertainty by roughly that factor. `effective_sample_size` (`paired_count /
design_effect`, the standard Kish 1965 deflation) says the 100 raw trials are worth about 30
genuinely independent ones. `cluster_count`/`max_cluster_size` are always present alongside these
(no estimator involved) - the number of distinct clusters and the largest one's size, e.g. how
repeated the most-replayed opening is.

Only `winrate`/`elo` this round - `mean-diff`/`sign-test`/`quantile-diff`/`relative-diff` cluster
support is deferred (see `docs/research-map.md`), a separate piece of wiring since those metrics
bootstrap by individual record today, not by outcome tally.

## Verdict logic

The gate compares the confidence interval, not the point estimate, against
the thresholds: `pass` requires the CI's pessimistic (lower) bound to clear
`--pass-above`; `fail` requires the CI's optimistic (upper) bound to be at
or below `--fail-below`. Anything else, including zero usable trials, is
`inconclusive`.

`--min-effect X` is shorthand for symmetric thresholds
(`--pass-above X --fail-below -X`) and defaults to `0`.

## Statistical basis

veridict's numbers are standard, published statistics, not a bespoke scoring system. See
[`docs/metrics.md`](docs/metrics.md) for the full per-metric detail (assumptions, failure modes)
and [`docs/research-map.md`](docs/research-map.md) for methods considered but not shipped, and
what's deliberately out of scope.

* **`winrate`/`sign-test` CI** - Wilson score interval (Wilson 1927); `--ci-method exact` gives
  the Clopper-Pearson exact binomial interval (Clopper & Pearson 1934) instead.
* **`mean-diff`/`quantile-diff`/`relative-diff` CI** - percentile or BCa (bias-corrected and
  accelerated) bootstrap, both from Efron & Tibshirani, *An Introduction to the Bootstrap* (1993,
  ch. 14). BCa is CLI-gated for `quantile-diff` only (see [`docs/metrics.md`](docs/metrics.md));
  `relative-diff` supports all three `--bootstrap-method` variants, same as `mean-diff`.
* **`elo`** - the logistic Elo model, the widely-used variant of Elo's original rating system
  (Elo 1978).
* **`sprt`** - Wald's sequential probability ratio test (Wald 1945, `--sprt-variant wald`); the
  `trinomial`/`pentanomial` variants are generalized LLR tests in the style historically used by
  chess-engine testing tools (Fishtest's `LLRlegacy`/`LLR_logistic`).
* **`matrix`'s general-graph mode** - the Bradley-Terry paired-comparison model (Bradley & Terry
  1952), fit via the Zermelo (1929)/Hunter (2004) Minorization-Maximization fixed-point iteration;
  the existence condition for a finite solution comes from Ford (1957).

The following are *not* citation-backed statistical results - they're this project's own design
choices or heuristics, and are labeled that way deliberately rather than dressed up as theorems:

* **`pass`/`fail`/`inconclusive`** - comparing a CI against a threshold is a standard decision
  rule, but which threshold to use and the "false pass is worse than inconclusive" bias (see
  Verdict logic) are this project's own conservative design choice.
* **`estimated_additional_trials`** - for `winrate`/`sign-test`/`elo` this binary-searches the
  real CI formula the report already uses, which is exact for the stated model (point estimate
  held fixed). `mean-diff`/`quantile-diff`/`relative-diff` are the exception: there's no such
  closed form for a bootstrap CI, so all three fall back to an `O(1/sqrt(n))` scaling heuristic
  with a documented bias (see Report extras).
* **`warnings`** - the 30-trial, 20%-failure-rate, 50%-draw-rate, (`quantile-diff` only)
  10-expected-observations-in-the-tail, and (`mean-diff` only) ~10x-baseline-scale thresholds are
  conventional rules of thumb, not derived from a specific paper.

### References

- Wilson, E. B. (1927). "Probable Inference, the Law of Succession, and Statistical Inference."
  *Journal of the American Statistical Association*, 22(158), 209-212.
- Clopper, C. J.; Pearson, E. S. (1934). "The use of confidence or fiducial limits illustrated in
  the case of the binomial." *Biometrika*, 26(4), 404-413.
- Efron, B.; Tibshirani, R. J. (1993). *An Introduction to the Bootstrap*. Chapman & Hall/CRC.
- Wald, A. (1945). "Sequential Tests of Statistical Hypotheses." *Annals of Mathematical
  Statistics*, 16(2), 117-186.
- Elo, A. (1978). *The Rating of Chessplayers, Past and Present*. Arco Publishing.
- Bradley, R. A.; Terry, M. E. (1952). "Rank Analysis of Incomplete Block Designs: I. The Method
  of Paired Comparisons." *Biometrika*, 39(3/4), 324-345.
- Zermelo, E. (1929). "Die Berechnung der Turnier-Ergebnisse als ein Maximumproblem der
  Wahrscheinlichkeitsrechnung." *Mathematische Zeitschrift*, 29, 436-460.
- Hunter, D. R. (2004). "MM algorithms for generalized Bradley-Terry models." *Annals of
  Statistics*, 32(1), 384-406.
- Ford, L. R. Jr. (1957). "Solution of a Ranking Problem from Binary Comparisons." *The American
  Mathematical Monthly*, 64(8), 28-33.

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
