# Research map

English | [日本語](research-map_ja.md)

This file records plausible features that are not shipped. It is not a schedule. Implemented
behavior belongs in [`metrics.md`](metrics.md).

Each candidate needs a concrete workflow, a defined statistical claim, calibration evidence where
appropriate, and a license-compatible implementation path. Until then, the current conservative
behavior remains intentional.

## Numeric metrics

### BCa for `quantile-diff`

BCa is available internally but rejected by the CLI for quantiles. Sample quantiles are non-smooth,
and current p95 calibration did not improve coverage over percentile bootstrap. Reconsider only if
broader calibration identifies a regime with a repeatable benefit.

### Power, matrix, and plan for `quantile-diff`

Quantile variance depends on density near the target quantile. It cannot reuse the mean-difference
power model with only a standard deviation. Reconsider for a concrete tail-latency workflow that
can choose between a density estimate and a distributional assumption. Matrix and plan support
also wait for clear matrix verdict semantics.

### `power --metric relative-diff`

This needs an assumed standard deviation of per-record relative changes, or a pilot file transformed
with `(candidate - baseline) / baseline`. Pilot validation must enforce `baseline > 0`. Implement
when a caller needs planning in proportional units.

### Subset-only relative effects

The current report identifies exact ties but keeps them in the pooled effect. Reporting an effect
only for changed cases would condition on observed outcomes and raises unresolved questions about
pairing, resampling, and whether the value is diagnostic or confirmatory. Implement only after
those semantics are fixed for a real diluted-by-ties case.

### Log-ratio metric

`log(candidate / baseline)` is symmetric when candidate and baseline are swapped, but it has
different units and requires both values to be positive. Add it only for a workflow already making
claims in log-ratio units.

### Cluster bootstrap for numeric metrics

`--cluster-by-id` currently supports `winrate` and `elo`. Extending it to numeric metrics requires
rules for pairing, cluster count, degenerate clusters, and bootstrap units. Priority order, if
activated: `mean-diff`, `relative-diff`, `quantile-diff`, then `sign-test`. Require a correlated
fixture and coverage calibration against independent-row intervals.

## Binomial and Elo refinements

### Wilson continuity correction

A continuity correction may improve small-sample coverage but affects shared code for `winrate`,
`sign-test`, and `elo`. Do not add it without an authoritative formula, calibration, and a measured
benefit over the existing exact and Jeffreys choices.

### Normalized-Elo pentanomial and Siegmund correction

The shipped pentanomial test uses a logistic-Elo generalized LLR. Normalized Elo and the Siegmund
discrete-time bound correction are refinements, not fixes for an invalid test. Reconsider if tests
show meaningful sensitivity to draw rate, opening selection, or discrete boundary checks.

### Draw-aware `power --metric elo`

Current Elo power planning uses a binomial model and is a lower bound when draws are common. A
draw-aware version needs an explicit assumed draw rate and a trinomial search. Add it when a
draw-heavy workflow demonstrates a material planning error.

## Matrix and planning

### Matrix verdict semantics

Matrix output is descriptive. An overall verdict needs a declared claim: all candidates differ,
one candidate is best, a designated candidate clears every rival, or something else. Define the
claim before adding thresholds, promotion, or matrix-wide multiple-comparison correction.

### Failure policy for matrix

Mapping a failed run to a head-to-head loss is not always valid. Extend `--failure-policy` only
when a workflow can define which competitor was responsible and how ambiguous dual failures are
handled.

### Correction with bootstrap or clustered intervals

Claim correction currently requires reconstructible binomial intervals. Bootstrap metrics and
`--cluster-by-id` are rejected. Extend correction only with a method that uses the actual resampling
unit and preserves the family-wise error guarantee; never substitute an independent-row interval.

### Budget-constrained allocation

`plan` ranks uncertain pairs independently. Optimizing a fixed budget requires an objective such as
“identify the best” and a tested allocation algorithm. Do not add `--budget` until both are defined.

## Run provenance

### Checkpoint and training lineage

`verify-run` validates an experiment envelope but does not model training lineage. A future generic
extension could verify opaque parent artifacts, data hashes, and configuration hashes. It must not
become a framework-specific experiment database.

## Time-sensitive testing

### Trinomial and pentanomial policies

The shipped command is Bernoulli. Draw-aware or paired policies need a state space, e-process, and
reward interpretation for three or five outcomes. Add them only with a concrete time-sensitive
workflow and calibration.

### Composite hypotheses and online alternatives

Estimating `p1` from the same stream changes the validity argument. A mixture or confidence-sequence
construction is required; plugging in the current estimate is not acceptable. Reconsider when
simple hypotheses are too restrictive in practice.

### Elo inputs and policy refinements

Elo flags would only be a parameter conversion for Bernoulli data unless draw handling also changes.
Boundary-aware EDO, capped policies, matrix support, and preset schedules remain deferred until the
base command has real usage evidence. Domain-specific schedules belong in integrations, not core.

## Deliberately out of scope

- Benchmark runners, match runners, domain-specific judges, and metric producers.
- Experiment databases, dashboards, cloud services, and distributed workers.
- Prediction-set construction, local-risk models, and threshold-selection systems.
- Causal discovery, intervention selection, and adaptive experiment control.
- Domain presets whose meaning depends on a particular engine, model, or dataset.

External tools may emit generic paired scores, outcomes, statuses, or run envelopes for `veridict`
to judge. The dependency must remain one-way.
