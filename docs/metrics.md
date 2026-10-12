# Metrics reference

English | [日本語](metrics_ja.md)

This document defines shipped statistical behavior. Start with the [README](../README.md) for
examples. Unshipped ideas belong in the [research map](research-map.md).

## Decision contract

For a reported effect interval `[L, U]`:

- `pass` when `L >= pass_above`;
- `fail` when `U <= fail_below`;
- `inconclusive` otherwise.

`--min-effect x` means `pass_above = x` and `fail_below = -x`. Use `metric=value` thresholds in a
multi-metric run when units differ. Input or configuration errors exit with code `3`; they are not
statistical verdicts.

`promotion` also includes evidence-validity checks. A breached validity cap can demote a
statistical pass to `inconclusive`, but cannot create a pass or a fail.

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
`+/-1600` Elo). Elo summarizes the observed result population; it does not establish strength
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
prevents promotion.

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

`sprt` compares simple hypotheses with boundaries:

```text
upper = ln((1 - beta) / alpha)
lower = ln(beta / (1 - alpha))
```

Both rates must be in `(0, 1)` and `alpha + beta` must be less than `1`; otherwise the lower
boundary would meet or exceed the upper boundary and the configuration is rejected.

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
constrain supported paired modes.

Use `verdict`, `promotion`, and `decision_*` for the decision. Generic `available_*`, `analyzed_*`,
`stopping_*`, and `ignored_*_after_stop` fields describe the replay. For schema-v1 compatibility,
unprefixed Wald/trinomial counts and `llr` still describe the full input; do not reconstruct the
decision from them.

Library callers should prefer `sprt::run_with_options(records, config, options)` with
`SprtRunOptions`. Its default is Wald, unpaired, `FailurePolicy::ReportOnly`, incomplete IDs
tolerated, and no paired-ID limits. The original positional `sprt::run` remains available as a
compatibility wrapper and uses the same validation path.

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
recorded opaque identifiers or hashes. It does not open artifacts, recompute hashes, or prove
benchmark quality.

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
  wide scales. Warnings do not change the verdict.

JSON Schemas in [`schemas/`](../schemas/) are the machine-readable field contracts.

## References

- Wilson, E. B. (1927). “Probable Inference, the Law of Succession, and Statistical Inference.”
- Clopper, C. J.; Pearson, E. S. (1934). “The Use of Confidence or Fiducial Limits Illustrated in
  the Case of the Binomial.”
- Efron, B.; Tibshirani, R. J. (1993). *An Introduction to the Bootstrap*.
- Wald, A. (1945). “Sequential Tests of Statistical Hypotheses.”
- Elo, A. (1978). *The Rating of Chessplayers, Past and Present*.
- Bradley, R. A.; Terry, M. E. (1952). “Rank Analysis of Incomplete Block Designs: I.”
- Hunter, D. R. (2004). “MM Algorithms for Generalized Bradley-Terry Models.”
