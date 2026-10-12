# Research map

English | [日本語](research-map_ja.md)

This file records plausible features that are not shipped. It is not a schedule. Shipped behavior
belongs in [`metrics.md`](metrics.md). Product boundaries and development priorities are defined
in [Product scope](product-scope.md).

Correctness fixes to shipped decisions take priority over these candidates. In particular, false
passes, non-finite calculations, and degenerate confidence intervals are reliability work, not
optional research refinements.

A candidate moves into implementation only when it has a concrete workflow, a defined statistical
claim, suitable calibration evidence, and a license-compatible implementation path. Until then,
the current conservative behavior is intentional.

## Numeric metrics

| Candidate | Why it is deferred | Activation condition |
|---|---|---|
| BCa for `quantile-diff` | Sample quantiles are non-smooth; current p95 calibration did not improve coverage | Repeatable coverage benefit in broader calibration |
| Quantile `power`, `matrix`, and `plan` | Quantile variance needs density or distribution assumptions | A tail-latency workflow that can declare those assumptions |
| `power --metric relative-diff` | Needs relative-change SD or a transformed pilot with `baseline > 0` | A caller needs planning in proportional units |
| Subset-only relative effect | Conditioning on changed cases leaves filtering, pairing, and claim semantics unresolved | A real dilution case with predeclared semantics |
| Log-ratio metric | Different units and strict positivity are not interchangeable with relative difference | A workflow already making claims in log-ratio units |
| Numeric cluster bootstrap | Pairing, cluster count, degeneracy, and resampling units need a contract | Correlated fixture and coverage gain over row-wise intervals |

If numeric clustering is activated, priority is `mean-diff`, `relative-diff`, `quantile-diff`, then
`sign-test`.

## Binomial and Elo

| Candidate | Why it is deferred | Activation condition |
|---|---|---|
| Wilson continuity correction | It changes shared binomial behavior without measured benefit | Authoritative formula plus calibration against exact and Jeffreys intervals |
| Normalized-Elo pentanomial or Siegmund correction | These are refinements, not correctness fixes | Material sensitivity to draw rate, pairing, or discrete boundary checks |
| Draw-aware Elo power | Current binomial plan does not model draws | A draw-heavy workflow with a material planning error and declared draw rate |

## Matrix and planning

| Candidate | Why it is deferred | Activation condition |
|---|---|---|
| Matrix-wide verdict | “Best”, “different”, and “clears every rival” are different claims | A declared claim and multiplicity rule |
| Matrix failure policy | A failed run cannot always be assigned to one competitor | Clear responsibility and dual-failure semantics |
| Correction for bootstrap or clustered intervals | Independent-row reconstruction would use the wrong resampling unit | A method preserving the family-wise guarantee with actual resampling units |
| Budget-constrained allocation | Ranking uncertain pairs is not a budget objective | A defined objective and tested allocation algorithm |

## Provenance and time-sensitive testing

| Candidate | Why it is deferred | Activation condition |
|---|---|---|
| Training/checkpoint lineage | `verify-run` validates an envelope, not an experiment database | A generic need for opaque parent, data, and configuration hashes |
| Trinomial or pentanomial time policies | Three- or five-outcome state, e-process, and reward semantics are undefined | A concrete time-valued workflow and calibration |
| Composite or online alternatives | Estimating `p1` from the same stream changes validity | A mixture or confidence-sequence construction |
| Elo flags and policy presets | Elo-only conversion adds no draw model; domain schedules do not belong in core | Real usage evidence for a generic refinement |

## Out of scope

- Benchmark or match runners, domain-specific judges, and metric producers.
- Experiment databases, dashboards, cloud services, and distributed workers.
- Prediction-set construction, local-risk models, and threshold-selection systems.
- Causal discovery, intervention selection, and adaptive experiment control.
- Presets tied to a particular engine, model, or dataset.

External tools may emit generic scores, outcomes, statuses, or run envelopes for `veridict` to
judge. The dependency remains one-way.
