# Product scope

English | [日本語](product-scope_ja.md)

`veridict` is an independent Rust library and CLI for deciding whether a candidate meets declared
adoption criteria against a baseline, using externally collected evaluation results. It reports
`pass`, `fail`, or `inconclusive`, with the effect, uncertainty, assumptions, and technical failures
needed to explain the decision. The caller owns the adoption action.

This document defines product direction and feature boundaries. Current behavior and limitations
are defined in [metrics.md](metrics.md); a product objective is not a guarantee that every current
implementation already meets it.

## The primary workflow is a candidate adoption decision

A developer changes an implementation, model, algorithm, or configuration and needs to decide
whether to adopt it. The relevant evidence may be paired numeric scores, win/loss/draw outcomes,
or execution failures. Some evaluations have a fixed sample size; others collect observations in
order until a sequential rule reaches a decision.

The product should make this workflow reproducible and usable in CI. Success means traceable
decisions, explicit reasons to collect more evidence, and fewer unsupported adoptions. Method
count and unrelated statistical breadth are not success measures.

## Responsibilities stay at the result-to-decision boundary

| Responsibility | What belongs in veridict |
|---|---|
| Evaluation input | JSONL/CSV parsing, schema validation, explicit score/outcome/status contracts |
| Evidence structure | Pair and cluster semantics, independent observation units, ordering and completeness checks |
| Statistical decisions | Fixed-sample intervals and thresholds; sequential tests with declared hypotheses and stopping rules |
| Technical failures | Timeout, crash, and invalid-output counts, explicit failure policies and validity limits |
| Explanation and reproducibility | Stable JSON, Markdown summaries, seeds, assumptions, decision-prefix information and supplied identifiers |
| Supporting analysis | Evidence-size estimates, descriptive candidate comparisons and suggestions for collecting more evidence |

`compare` and `sprt` are the central decision paths. `verify-run` checks the supplied run structure
and declared identifiers; it does not authenticate artifacts or prove that a runner obeyed its
configuration. `power`, `matrix`, and `plan` support the decision workflow. Existing experimental
`time-sensitive` testing remains bounded by its documented model and should not drive expansion
into experiment orchestration.

## Real workflows guide generic interfaces

Development priorities come from repeatable adoption decisions. Features express their
statistical meaning through generic trials, pairs, scores, outcomes, statuses, hypotheses, or
opaque identifiers. Paired outcomes, draws, Elo, and sequential stopping remain in scope when
their assumptions are explicit.

Consumers produce their own metrics and records, either using the library or invoking the CLI.
Domain adapters, runner settings, model access, and application-specific adoption policies stay
with those consumers. The dependency direction is always:

```text
external evaluation tools -> veridict
```

`veridict` must build, test, and run without any consuming project's source tree, artifacts,
services, or credentials. Public documentation describes reusable workflows and contracts;
private consumer names and internal prioritization context stay outside published materials.
Existing serialized field names remain compatible unless a separately documented schema change
is necessary.

## Correct decisions take priority over additional methods

1. Fix incorrect passes, non-finite calculations, invalid configuration handling, and misleading
   uncertainty before adding methods. Small samples and degenerate resampling distributions need
   explicit decision behavior, backed by boundary-case and error-rate checks.
2. Make paired and sequential decisions reliable: preserve the actual evidence unit, the first
   eligible stopping event, failure semantics, and consistency of the supplied run records.
3. Improve reproducibility, integration, reports, and performance on these workflows. Measure
   improvements with fixed inputs, configurations, and versions.
4. Add a statistical method only for a demonstrated decision need, with defined assumptions,
   calibrated behavior, and a manageable API and maintenance cost. The [research map](research-map.md)
   records candidates rather than a commitment to implement them.

Evidence requirements depend on the method and data structure. A universal sample-count cutoff
is not a substitute for checking whether an interval or stopping rule supports the claimed decision.

## Work owned by other tools

The core product does not run benchmarks or matches, implement application protocols, train
models, generate domain metrics, or act as an LLM judge. It does not manage experiment databases,
dashboards, cloud services, distributed execution, artifact deployment, or adaptive experiment
control. It does not select favorable thresholds after seeing results or certify general
superiority outside the supplied evaluation conditions.

Integrations may perform these tasks and supply generic observations to `veridict`. A proposed
feature belongs here only when it improves the result-to-decision contract without taking over
those responsibilities.
