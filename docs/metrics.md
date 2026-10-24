# Metrics reference

English | [日本語](metrics_ja.md)

This document defines behavior in the current source tree. The latest published release is
`0.19.6`; its exact contract remains in the
[v0.19.6 tag](https://github.com/kent-tokyo/veridict/tree/v0.19.6). Start with the
[README](../README.md) for examples; unimplemented ideas belong in the [research map](research-map.md).

## Decision contract

For a reported effect interval `[L, U]`:

- `pass` when `L >= pass_above`;
- `fail` when `U <= fail_below`;
- `inconclusive` otherwise.

`--min-effect x` means `pass_above = x` and `fail_below = -x`. Use `metric=value` thresholds in a
multi-metric run when units differ. Input or configuration errors exit with code `3`; they are not
statistical verdicts.

`promotion` also includes evidence-validity checks. A breached validity cap forces
`validity=invalid`, `verdict=inconclusive`, and `promotion=not_promoted`, regardless of the
uncapped verdict. It cannot create a pass or a fail.

## Fixed-sample metrics

| Metric | Observation | Effect | Interval |
|---|---|---|---|
| `winrate` | decisive outcome | candidate win rate minus `0.5` | Wilson, exact, or Jeffreys |
| `sign-test` | non-tied paired score | positive-sign rate minus `0.5` | Wilson, exact, or Jeffreys |
| `mean-diff` | paired score | mean `candidate - baseline` | percentile, basic, or BCa bootstrap |
| `quantile-diff` | paired score | selected quantile of paired differences | percentile or basic bootstrap |
| `relative-diff` | paired score with `baseline > 0` | mean per-record proportional change | percentile, basic, or BCa bootstrap |
| `elo` | win/draw/loss score | logistic Elo of mean score | transformed score interval |
| `paired-elo` | complete two-trial pair | logistic Elo of mean pair score | pair-level percentile bootstrap |

### Metric-specific rules

- `winrate` excludes draws; `sign-test` excludes exact ties. `--ci-method` selects `wilson`
  (default), Clopper-Pearson `exact`, or `jeffreys`. No decisive observation gives
  `inconclusive`.
- `mean-diff` resamples paired differences, never the two columns independently.
- Numeric inputs and every derived difference, effect, and interval bound must remain finite.
  An unrepresentable result is an input/configuration error (exit code `3`), never a `pass` with
  JSON `null` fields. Rescale the input values and thresholds without changing their meaning.
- `quantile-diff` defaults to the median; specify `--quantile` in automation. BCa is rejected
  because current calibration does not justify it for a non-smooth sample quantile.
- `relative-diff` averages `(candidate - baseline) / baseline`. `tied_count` and
  `data_quality.diluted_by_ties` are diagnostics and do not change the verdict.
- Bootstrap metrics use `--resamples` and a reproducible `--seed`. `percentile` is the default;
  `basic` reflects its endpoints around the estimate; BCa adds bias and acceleration correction.
- `mean-diff` may report a baseline-scale warning. Choose absolute or proportional effects before
  inspecting outcomes; the warning is advisory.

### Elo

Candidate wins score `1`, draws `0.5`, and baseline wins `0`. For mean score `p`:

```text
Elo = 400 * log10(p / (1 - p))
```

Scores are clamped to `[0.0001, 0.9999]` before conversion, so shutouts remain finite (about
`±1600` Elo). Elo summarizes the observed result population; it does not establish strength
outside that population.

`paired-elo` requires `--paired-by-id`, a non-empty ID on every record, and exactly two usable
outcomes per ID. It retains combined scores `{0, 0.5, 1, 1.5, 2}`, reports them in
`pair_score_counts`, divides by two before Elo conversion, and bootstraps whole pairs. Missing IDs,
singletons, triples, and incomplete outcomes are input errors.

`paired-elo` is fixed-sample analysis. It does not replace pentanomial SPRT. Existing
`elo --paired-by-id` still reduces each pair to a net win/draw/loss.

## Pairing, clustering, and failures

`--paired-by-id` combines records according to the selected metric. Normally one record remains a
singleton, two records become one ID-level observation, and three or more are invalid. Outcome
metrics net the two scores; numeric metrics average the two differences. `paired-elo` is the strict
complete-pair exception described above.

`--cluster-by-id` instead keeps all rows and bootstraps whole ID clusters. It supports only
`winrate` and `elo`, needs enough distinct clusters, and is incompatible with `--paired-by-id`.

Statuses are `ok`, `timeout`, `crash`, and `invalid`.

| `--failure-policy` | Outcome treatment |
|---|---|
| `report-only` | Count failures and use a literal result when present |
| `exclude` | Exclude an outcome if either side failed |
| `loss` | Convert a one-sided failure to a loss; convert dual failure to a draw |

Failure counts are never hidden. `exclude` and `loss` apply only to outcome metrics and sequential
tests. `--max-timeouts`, `--max-crashes`, and `--max-invalid` are validity caps; breaching one
forces invalid validity and an inconclusive verdict. Reports preserve counts and the breach reason,
not every configured cap value.

## Multiple metrics

Repeat `--metric` to evaluate several claims. Overall verdict precedence is
`fail > inconclusive > pass`.

`--claim-correction` adds family-adjusted fields without replacing ordinary `verdict` or
`promotion`:

- `none`: no family adjustment;
- `bonferroni`: equal alpha allocation;
- `holm`: ordered Holm adjustment.

Families containing bootstrap metrics or `--cluster-by-id` are rejected because their adjusted
intervals cannot be reconstructed from the supported closed-form binomial model.

## Sequential tests

### Decision rule and variants

`sprt` compares simple hypotheses with boundaries:

```text
upper = ln((1 - beta) / alpha)
lower = ln(beta / (1 - alpha))
```

Both rates must be in `(0, 1)` and `alpha + beta` must be less than `1`; otherwise the lower
boundary would meet or exceed the upper boundary and the configuration is rejected.
Hypothesis values must map to representable probabilities, and both bounds must remain finite.
Any non-finite LLR or draw-rate intermediate is a configuration error (exit code `3`), not a
verdict with JSON `null` numeric fields.

The upper boundary gives `pass`, the lower boundary gives `fail`, and no crossing gives
`inconclusive`.

| Variant | Data and hypotheses |
|---|---|
| `wald` | Decisive outcomes; logistic Elo via `--elo0` and `--elo1` |
| `trinomial` | Win/draw/loss generalized LLR; BayesElo via `--belo0` and `--belo1` |
| `pentanomial` | Complete pair scores `{0, 0.5, 1, 1.5, 2}`; logistic Elo and `--paired-by-id` |

All variants replay input order and retain the first eligible crossing. Wald and trinomial evaluate
each usable trial, or each completed net pair with pairing; pentanomial evaluates each completed
pair. Records after stopping do not change the decision prefix but are still validated and counted
for failure caps. `--min-paired-ids`, `--max-paired-ids`, and `--require-complete-pairs` further
constrain supported paired modes; paired-ID limits must be at least `1`.

### Report fields and input replay

Use `verdict`, `promotion`, and `decision_*` for the decision. Generic `available_*`, `analyzed_*`,
`stopping_*`, and `ignored_*_after_stop` fields describe the replay. For schema-v1 compatibility,
unprefixed Wald/trinomial counts and `llr` still describe the full input; do not reconstruct the
decision from them.

SPRT reports also expose the advisory `data_quality.low_id_diversity` flag and a matching
`warnings` entry when one ID occurs at least three times among at least ten ID-tagged records in
unpaired mode. The signal depends only on ID structure and never changes the LLR, verdict,
validity, or promotion. It stays silent when every ID appears exactly twice and whenever
`--paired-by-id` is active, because repeated IDs have pairing semantics there.

### Library API

Library callers should prefer `sprt::run_with_options(records, config, options)` with
`SprtRunOptions`. Defaults are Wald, unpaired, `FailurePolicy::ReportOnly`, incomplete IDs allowed,
and no paired-ID limits. The positional `sprt::run` remains as a compatibility wrapper and uses
the same validation path.

### Checkpoints and resume

Long-running producers can add records through `sprt::SprtAccumulator` and serialize a
`SprtCheckpoint` returned by `snapshot()`. Resume with
`SprtAccumulator::from_checkpoint(checkpoint, config, options, expected_position)`, using a
separately persisted producer cursor for `expected_position`.

Resume rejects changes to the checkpoint version, hypotheses, options, record count, last line,
or prefix fingerprint. `push` line numbers must increase in input order. Checkpoints preserve the
first crossing, incomplete pairs, and failure counts. The fingerprint detects stream mix-ups; it
does not authenticate data. Failure caps are not checkpoint options, so apply the same
`FailureCaps` to batch and resumed reports for CLI-equivalent validity and promotion.

## Supporting commands

### `matrix` and `plan`

`matrix` fits Bradley-Terry strengths within each connected component and reports pairwise
logistic-Elo estimates. Cross-component cells are `disconnected` with null estimates. The output
is descriptive and has no overall verdict.

`plan` ranks comparisons by additional evidence needed for a required `--min-elo`. It is not an
optimizer, scheduler, or trial runner.

### `power`

`power` estimates evidence needs before collection:

- exact interval search for `winrate` and `sign-test`;
- binomial score search for `elo`, which is a lower bound when draws are common;
- normal approximation for `mean-diff`, using `--assume-sd` or `--pilot`;
- Wald average-sample-number approximation with `--sprt`.

`paired-elo`, `relative-diff`, and `quantile-diff` power are not implemented. Power outputs are
planning estimates, not verdicts or sample-size guarantees.

### `verify-run`

`verify-run` validates pairing, order, contamination, and consistency between manifest values and
recorded identifiers or hashes, treating their contents as opaque. It does not open artifacts,
recompute hashes, or prove benchmark quality.

### `time-sensitive`

`time-sensitive` is an experimental, one-sided, anytime-valid Bernoulli simple-vs-simple test with
a finite reward horizon. Policies are `bellman`, `edo`, and `gro`; rewards are `hard-deadline`,
`exponential`, or a custom schedule. Draws advance time without changing wealth. The command does
not support Elo, draw-aware hypotheses, or online composite alternatives.

## Report interpretation

- `estimated_additional_trials` is an advisory projection for an inconclusive fixed-sample result.
  Bootstrap metrics use an `O(1/sqrt(n))` interval-width heuristic. It may be `null`.
- `inconclusive_kind` is `directional` when the interval excludes zero, `noise` when it spans zero,
  and `null` when no ordinary interval judgment exists.
- `warnings` expose interpretation risks such as small samples, failures, draws, sparse tails, or
  wide scales. They normally do not change the verdict.

### Small-sample floor

A fixed-sample report with fewer than two independent analysis units is forced to `inconclusive`.
One observation cannot estimate empirical uncertainty. For `paired-elo`, the unit is a completed
pair; for other paired modes, it is an ID group or an ID-less observation; for clustering, it is a
cluster. Samples below 30 still carry a warning. Two units permit evaluation but do not guarantee
adequate small-sample error rates.

### Schema versions

JSON Schemas in [`schemas/`](../schemas/) cover the listed record and report formats. The
experimental `time-sensitive` report has no schema yet.

Current SPRT output uses schema v2, where `warnings` and `data_quality` are required. The archived
[`sprt-report-v1.schema.json`](../schemas/sprt-report-v1.schema.json) captures the earlier strict
shape. Published `0.19.5` reports kept version number 1 while emitting the two new fields, so they
do not satisfy that archived schema. Current code has no v1 output mode. For historical output,
pin the producer version and select the schema by `schema_version`.

## References

- Wilson, E. B. (1927). “Probable Inference, the Law of Succession, and Statistical Inference.”
- Clopper, C. J.; Pearson, E. S. (1934). “The Use of Confidence or Fiducial Limits Illustrated in
  the Case of the Binomial.”
- Efron, B.; Tibshirani, R. J. (1993). *An Introduction to the Bootstrap*.
- Wald, A. (1945). “Sequential Tests of Statistical Hypotheses.”
- Elo, A. (1978). *The Rating of Chessplayers, Past and Present*.
- Bradley, R. A.; Terry, M. E. (1952). “Rank Analysis of Incomplete Block Designs: I.”
- Hunter, D. R. (2004). “MM Algorithms for Generalized Bradley-Terry Models.”
