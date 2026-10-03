# Metrics reference

English | [日本語](metrics_ja.md)

This document defines the statistical contract of the shipped commands. For examples and basic
CLI use, start with the [README](../README.md). For unshipped ideas, see the
[research map](research-map.md).

## Common decision rule

For confidence-interval metrics, let `[L, U]` be the interval for the reported effect.

- `pass` when `L >= pass_above`.
- `fail` when `U <= fail_below`.
- `inconclusive` otherwise.

`--min-effect x` is the symmetric form: `pass_above = x` and `fail_below = -x`. In a multi-metric
run, use `metric=value` thresholds when units differ. Invalid input or configuration is not a
statistical verdict and exits with code `3`.

`promotion` is stricter than `verdict`: validity checks may demote a statistical pass to
`inconclusive`, but cannot create a pass or convert a result to `fail`.

## Comparison metrics

| Metric | Observation | Point estimate | Interval |
|---|---|---|---|
| `winrate` | decisive `result` | candidate win rate minus `0.5` | Wilson, exact, or Jeffreys |
| `sign-test` | paired numeric record | positive-sign rate minus `0.5` | Wilson, exact, or Jeffreys |
| `mean-diff` | `candidate - baseline` | arithmetic mean | percentile, basic, or BCa bootstrap |
| `quantile-diff` | `candidate - baseline` | selected sample quantile | percentile or basic bootstrap |
| `relative-diff` | `(candidate - baseline) / baseline` | arithmetic mean | percentile, basic, or BCa bootstrap |
| `elo` | win/draw/loss score | logistic Elo transform of mean score | transformed score interval |

### `winrate` and `sign-test`

Draws and exact numeric ties are excluded, respectively. Effects and interval endpoints are
centered on zero by subtracting `0.5`; therefore `--min-effect 0.02` means a 52% decisive win or
positive-sign rate. `baseline_count` and `candidate_count` are side-specific decisive counts;
`paired_count` is their sum.

`--ci-method` selects:

- `wilson` (default): Wilson score interval.
- `exact`: Clopper-Pearson interval.
- `jeffreys`: equal-tailed beta posterior interval with Jeffreys prior.

Exact and Jeffreys require integer binomial counts. If no decisive observation remains, the result
is `inconclusive`.

### `mean-diff`

Each record contributes one paired difference. Bootstrap resampling is over those differences, not
over the baseline and candidate columns independently. `--resamples` controls the number of draws;
`--seed` makes the result reproducible.

`percentile` is the default. `basic` reflects percentile endpoints around the point estimate. `bca`
adds bias and acceleration corrections and requires enough non-degenerate data for jackknife terms.

### `quantile-diff`

The metric takes the selected sample quantile of recordwise paired differences. The default is
`0.5` (the median); use `--quantile` explicitly for stable automation.

BCa is rejected for this metric. Sample quantiles are non-smooth, and current calibration did not
show a coverage improvement that justifies exposing the method. Use percentile or basic.

### `relative-diff`

Every usable record must have `baseline > 0`. The reported effect is the mean of per-record ratios,
not the ratio of aggregate means. An effect of `0.03` means a mean relative increase of 3%.

`tied_count` reports exact zero differences. `data_quality.diluted_by_ties` warns when unchanged
records may dilute an effect that only touches a subset. This diagnostic does not alter the metric
or verdict.

### Scale diagnostic

For `mean-diff`, `scale_diagnostics` inspects baseline magnitudes only. A wide-scale warning means
large cases may dominate an absolute mean. It is advisory and never changes the verdict. Choose
`relative-diff` before examining outcomes when proportional change is the intended claim.

### `elo`

Each candidate win scores `1`, draw `0.5`, and baseline win `0`. With score `p`, the point estimate
is the logistic-Elo transform:

```text
Elo = 400 * log10(p / (1 - p))
```

The confidence interval is transformed endpoint-wise. Boundary scores may produce infinite Elo;
JSON represents non-finite endpoints as `null` and adds a warning. Elo here is a result-scale
summary, not proof of strength outside the observed population.

## Pairing and clustering

`--paired-by-id` combines records with the same ID using the metric's pairing rule. One record is a
valid singleton, two become one ID-level observation, and three or more are invalid. Outcome
metrics net the two scores; numeric metrics average the two paired differences. Without pairing,
duplicate IDs are rejected where uniqueness is required.

`--cluster-by-id` is different: it keeps every trial but resamples whole ID clusters for `winrate`
or `elo`. It protects the interval from treating correlated rows as independent. It requires enough
distinct clusters, is incompatible with `--paired-by-id`, and is not supported for numeric metrics.

## Failures and evidence validity

`baseline_status` and `candidate_status` accept `ok`, `timeout`, `crash`, and `invalid`.

| `--failure-policy` | Metric treatment |
|---|---|
| `report-only` | Count failures; use a literal `result` if present |
| `exclude` | Ignore any outcome on a record where either side failed |
| `loss` | Candidate failure becomes a baseline win, baseline failure becomes a candidate win, and dual failure becomes a draw |

The policy does not hide counts. `exclude` and `loss` are accepted only by outcome-based metrics
and sequential tests; numeric metrics reject them as configuration errors.

`--max-timeouts`, `--max-crashes`, and `--max-invalid` define validity caps. A breached cap records
the reason and prevents promotion. In multi-metric runs, validity and promotion are summarized at
the overall level as well as per metric.

## Multiple metrics and claim correction

Repeat `--metric` to evaluate several claims from one input. Overall verdict precedence is
`fail > inconclusive > pass`.

`--claim-correction` controls family-wise confidence:

- `none`: each metric uses the requested confidence independently.
- `bonferroni`: equal alpha allocation across claims.
- `holm`: ordered Holm adjustment; never less powerful than Bonferroni for the same family.

Correction does not replace the ordinary `verdict` or `promotion`. It populates the separate
`family_adjusted_verdict`, `family_adjusted_promotion`, and multi-report
`simultaneous_claims_promotion` fields. Corrected claims can only be preserved or demoted. Families
containing bootstrap metrics or `--cluster-by-id` are rejected because the required adjusted
interval cannot be reconstructed from the closed-form binomial model.

## SPRT

`sprt` compares two simple hypotheses using log-likelihood ratio boundaries:

```text
upper = ln((1 - beta) / alpha)
lower = ln(beta / (1 - alpha))
```

Crossing the upper boundary returns `pass`; crossing the lower boundary returns `fail`; otherwise
the result is `inconclusive`.

### Variants

- `wald`: binary test over decisive outcomes. Hypotheses are logistic Elo (`--elo0`, `--elo1`).
- `trinomial`: win/draw/loss generalized LLR with an estimated draw nuisance parameter. Hypotheses
  are BayesElo (`--belo0`, `--belo1`), not logistic Elo.
- `pentanomial`: generalized LLR over paired two-trial scores `{0, 0.5, 1, 1.5, 2}`. It requires
  `--paired-by-id` and exactly two records per completed pair.

All variants replay the input sequentially and retain the first eligible crossing. Input order is
therefore part of the experiment schedule. Wald and trinomial evaluate after each usable trial, or
after each completed net pair with `--paired-by-id`; pentanomial evaluates after each completed
pair. Later records cannot change the decision LLR or outcome prefix, but they are still parsed and
included in failure counts; a breached failure cap can force the final verdict to `inconclusive`.
`--min-paired-ids` and `--max-paired-ids` constrain pentanomial evaluation. With
`--require-complete-pairs`, incomplete pairs are rejected where supported.

### Report compatibility

The decision prefix is described by `decision_llr`, `decision_candidate_wins`,
`decision_baseline_wins`, `decision_draws`, and the generic `available_*`, `analyzed_*`,
`stopping_observation_*`, and `ignored_*_after_stop` fields.

For schema-v1 compatibility, unprefixed Wald/trinomial `llr` and outcome counts describe the full
input. Pentanomial retains its existing analyzed-prefix meaning. Consume `verdict`, `promotion`,
and `decision_*`; do not reconstruct a decision from legacy `llr`.

## Matrix and plan

`matrix` accepts candidates against a shared baseline or a general graph of head-to-head
`--matches`. It fits Bradley-Terry strengths within each connected component and reports pairwise
logistic-Elo differences and intervals. Cells across disconnected components have
`status=disconnected` and null estimates because those strengths are not comparable. Matrix output
is descriptive and has no overall pass/fail verdict.

`plan` uses the same graph and a required `--min-elo` to rank comparisons by additional evidence
needed. It is a recommendation list, not an optimizer or scheduler.

## Power

`power` estimates evidence needs before data collection.

- `winrate` and `sign-test`: exact search against the selected binomial interval.
- `elo`: binomial score search transformed to Elo. Because draws are not modeled, treat the result
  as a lower bound in draw-heavy settings.
- `mean-diff`: normal approximation using `--assume-sd` or a standard deviation estimated from
  `--pilot` paired differences.
- `--sprt`: Wald average sample number approximation under each hypothesis. It ignores boundary
  overshoot and is therefore an expectation, not a cap. `--horizon` adds a seeded Monte Carlo
  estimate of the probability of still having no decision by that trial count.

`relative-diff` and `quantile-diff` power are not implemented. For proportion metrics,
`--paired-by-id` adds a caveat but cannot estimate correlation before data exist; with a
`mean-diff` pilot it applies the actual pairing rule. Planning inputs must be fixed before the
confirmatory run; power estimates are not verdicts.

## Time-sensitive testing

`time-sensitive` is an experimental, one-sided, anytime-valid Bernoulli simple-vs-simple test.
It compares fixed `p0` and `p1` under a finite budget while optimizing a declared time-value reward.

Policies:

- `bellman`: dynamic-programming policy for the configured reward.
- `edo`: stationary expected-discount approximation, available only with `exponential` reward.
- `gro`: growth-rate-oriented baseline; it ignores the reward schedule.

Rewards are `hard-deadline`, `exponential`, or `--reward-schedule FILE`. Draws advance the trial
clock without changing wealth, so planned reward assumes an all-decisive stream. This command does
not accept Elo hypotheses, draw-aware trinomial hypotheses, or composite online estimation. Its exit codes are
`0` for pass, `2` for no decision, and `3` for invalid input or configuration.

## Additional report fields

### `estimated_additional_trials`

For an inconclusive fixed-sample metric, this estimates how many more observations might be needed
if the current effect persists. Closed forms are used where available; bootstrap metrics use an
`O(1/sqrt(n))` width-scaling heuristic. The field is advisory and may be `null`, including when the
current effect lies inside the threshold dead zone.

### `inconclusive_kind`

- `directional`: the interval excludes zero but does not clear a decision threshold.
- `noise`: the interval still spans zero.
- `null`: the result is not a normal CI judgment, such as zero usable observations or a validity
  cap breach.

### `warnings`

Warnings identify small samples, high failure or draw rates, sparse quantile tails, wide baseline
scales, non-finite Elo endpoints, and other known interpretation risks. They are transparent rules
of thumb and do not modify the verdict.

## References

- Wilson, E. B. (1927). “Probable Inference, the Law of Succession, and Statistical Inference.”
- Clopper, C. J.; Pearson, E. S. (1934). “The Use of Confidence or Fiducial Limits Illustrated in
  the Case of the Binomial.”
- Efron, B.; Tibshirani, R. J. (1993). *An Introduction to the Bootstrap*.
- Wald, A. (1945). “Sequential Tests of Statistical Hypotheses.”
- Elo, A. (1978). *The Rating of Chessplayers, Past and Present*.
- Bradley, R. A.; Terry, M. E. (1952). “Rank Analysis of Incomplete Block Designs: I.”
- Hunter, D. R. (2004). “MM Algorithms for Generalized Bradley-Terry Models.”
