# veridict

English | [日本語](README_ja.md)

[![CI](https://github.com/kent-tokyo/veridict/actions/workflows/ci.yml/badge.svg)](https://github.com/kent-tokyo/veridict/actions/workflows/ci.yml)
[![Crates.io](https://img.shields.io/crates/v/veridict.svg)](https://crates.io/crates/veridict)
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
* **Release regression gate in CI** - candidate build vs last known-good
  baseline, wired into a pipeline with `--fail-below`/`--pass-above` and a
  `veridict` exit code (see [Regression gate](#usage) below).
* **Ranking more than two variants** - several prompts/configs against the
  same shared baseline -> `veridict matrix`.

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
* `examples/status_failures.jsonl` - all supported record shapes together, illustrating the format (not meant to be run against a single metric as-is: a record must carry a field the chosen metric understands, or a `baseline_status`/`candidate_status` field, or it is rejected as a schema mismatch).
* `examples/chess_engine_draw_heavy.jsonl` - win/loss/draw records with a high draw rate, for
  `veridict sprt --sprt-variant trinomial` (see [SPRT](#sprt)).
* `examples/chess_engine_paired_openings.jsonl` - each `id` appears exactly twice (same opening,
  colors swapped), for `veridict sprt --sprt-variant pentanomial --paired-by-id` (see [SPRT](#sprt)).

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

`winrate` and `sign-test` report `effect`/`ci_low`/`ci_high` centered on 0
(deviation from a 50/50 split); `elo` is centered on 0 by construction (an
even score is 0 Elo). All three compose directly with `--min-effect`.
`mean-diff` reports them in the input's own units.

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
requesting either with `--metric mean-diff`/`--metric sign-test` is a config
error, not an arbitrary numeric penalty for a failed numeric trial.

Requesting several `--metric` flags together scans the input once, feeding
every record to every requested metric, rather than one full pass per
metric.

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
  Treat the number as "roughly this many, plausibly more," not a
  guarantee - it has a documented, quantified bias (e.g. an ~18%
  under-estimate at n=100 for one verified case).
* **`warnings`** - human-readable data-quality flags, empty when there's
  nothing to flag: a tiny sample (under 30 paired trials), an excessive
  failure rate (over 20% timeout/crash/invalid), for `elo`, a draw-heavy run
  (over 50% draws leaves few decisive outcomes to rate from), the measured
  effect being smaller than the CI's own half-width (plausibly noise around
  zero), or, in unpaired mode, one `id` being repeated 3+ times among 10+
  id-tagged trials (a sign the same test case was logged multiple times
  rather than run that many independent times - silent when every `id`
  appears exactly twice, the common case of forgetting `--paired-by-id`).
* **`data_quality`** - the same flags as `warnings`, as booleans
  (`tiny_sample`, `high_failure_rate`, `draw_heavy`, `effect_within_noise_floor`,
  `low_id_diversity`) rather than strings, for a machine consumer that wants
  to branch on a flag instead of parsing prose. Added alongside `warnings`,
  not a replacement - both are always present.

See [`docs/metrics.md`](docs/metrics.md) for the full detail on every method
above, including assumptions and known failure modes.

## SPRT

`veridict sprt` is a separate mode from `compare`: instead of an effect
size and a confidence interval checked against a threshold, it accumulates
a log-likelihood ratio and stops as soon as the evidence crosses one of two
boundaries derived from `--alpha`/`--beta`. `pass` means "confident the
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
  `pentanomial_counts` (the 5-bucket breakdown), `raw_trial_count`, and
  `paired_count`.

See [`docs/metrics.md`](docs/metrics.md) for the full mechanics of all
three variants, including the BayesElo/logistic-Elo unit conversion.

`sprt` also accepts `--failure-policy` (see [Metrics](#metrics) for the exact
`report-only`/`exclude`/`loss` semantics) - applies identically across all three
`--sprt-variant` choices, including `pentanomial`: a `loss`-synthesized outcome nets
against its pair partner the same way any other outcome would.

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

**`sprt --sprt-variant pentanomial` is the one exception to "an id used once
is an ordinary unpaired sample":** it keeps the pair's full 5-value score
instead of netting it (see [SPRT](#sprt)), which has no meaning for a lone
game, so it always requires `--paired-by-id` and rejects any id that
doesn't appear exactly twice - a lone id is a hard error here, not treated
as an unpaired sample the way it is everywhere else.

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
* **`mean-diff` CI** - percentile or BCa (bias-corrected and accelerated) bootstrap, both from
  Efron & Tibshirani, *An Introduction to the Bootstrap* (1993, ch. 14).
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
  held fixed). `mean-diff` is the exception: there's no such closed form for a bootstrap CI, so it
  falls back to an `O(1/sqrt(n))` scaling heuristic with a documented bias (see Report extras).
* **`warnings`** - the 30-trial, 20%-failure-rate, and 50%-draw-rate thresholds are conventional
  rules of thumb, not derived from a specific paper.

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
