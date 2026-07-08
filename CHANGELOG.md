# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

`veridict` is a domain-agnostic statistical decision gate: feed it paired candidate-vs-baseline
results (JSONL or CSV) and it returns `pass`/`fail`/`inconclusive`, never a false pass dressed up
as a pass - see [`docs/metrics.md`](docs/metrics.md) for the statistical basis of every number it
reports and [`docs/research-map.md`](docs/research-map.md) for what's deliberately out of scope.

## [Unreleased]

## [0.6.0] - 2026-07-08

Purely additive - no breaking changes since `0.5.0`.

### Added

- `veridict plan`: given the same input `matrix` accepts (legacy files and/or `--matches`) plus a
  required `--min-elo <f64>`, recommends which pairs would most benefit from more trials to narrow
  their Elo-difference CI, ranked most-uncertain first. Report-only, like `matrix` (no verdict,
  always exits `0` on success). New `schemas/plan-report.schema.json`. See `docs/metrics.md`'s
  `plan` section for the estimation math (an exact Wilson-CI binary search for a
  baseline-vs-one-candidate cell, the same `O(1/sqrt(n))` CLT-scaling fallback `mean-diff` already
  uses everywhere else) and `docs/research-map.md` for what's deliberately out of scope
  (`--budget`/`--goal identify-best` constrained allocation).

## [0.5.0] - 2026-07-08

### Added

- `--failure-policy report-only|exclude|loss` on `compare --metric winrate`/`--metric elo` and
  `sprt` (all three `--sprt-variant` choices): controls whether a failed trial affects the
  *computation*, not just whether it's reported. `report-only` (default) is unchanged from before
  this flag existed. `exclude`/`loss` are config errors for `--metric mean-diff`/`--metric
  sign-test`, which have no win/loss/draw outcome for a failed numeric trial to become. See
  `docs/metrics.md`'s new `--failure-policy` section for the exact semantics, including how a
  `loss`-synthesized outcome interacts with `--paired-by-id` netting.

### Changed

- **Breaking**: `MetricConfig::new` takes a new `failure_policy: FailurePolicy` parameter;
  `MetricConfig::Elo` changed from a unit variant to `Elo { failure_policy: FailurePolicy }`
  (`WinRate` gained the same field). `sprt::run` takes a new `failure_policy: FailurePolicy`
  parameter. Report JSON shapes are unchanged - this only affects direct library callers, not the
  CLI or any JSON schema.

## [0.4.0] - 2026-07-08

Purely additive - no breaking changes since `0.3.0`.

### Added

- `sprt --sprt-variant pentanomial`: a generalized LLR test over paired games (same opening,
  colors swapped), ported from Fishtest's `LLR_logistic`. Always requires `--paired-by-id`; adds
  `sprt_variant`, `pentanomial_counts`, `raw_trial_count`, and `paired_count` to the SPRT report
  (purely additive - `schema_version` stays `1`). See `docs/metrics.md`'s `sprt` section for why
  this captures within-pair correlation that running `trinomial` on the same games ungrouped
  cannot.

## [0.3.0] - 2026-07-06

Purely additive - no breaking changes since `0.2.0`.

### Added

- `data_quality.low_id_diversity` and a matching `warnings` string: fires when one `id` repeats 3
  or more times among at least 10 id-tagged trials in unpaired mode - a sign the "N independent
  trials" assumption behind the CI doesn't hold (the same underlying test case was likely logged
  multiple times, not run N genuinely separate times). Silent when every `id` appears exactly
  twice (the common, innocent case of forgetting `--paired-by-id`) and silent entirely under
  `--paired-by-id` (repeated ids mean something different there). Purely advisory, like every other
  `data_quality` flag - never affects `verdict`. Prompted by concrete feedback from a downstream
  consumer who'd hit this gap for real; closes it for `winrate`/`elo` specifically (`mean-diff`/
  `sign-test` already hard-reject any duplicate id in unpaired mode, unchanged here).

## [0.2.0] - 2026-07-05

Everything below is new since `0.1.0` on crates.io. `0.2.0` rather than `0.1.1` because of the
breaking `compare_one`/`compare_many` change - a minor bump is allowed for breaking changes under
semver pre-1.0, and crates.io doesn't allow republishing an existing version regardless.

### Changed

- **Breaking**: `compare_one`/`compare_many` (and the lower-level `metrics::compute`/
  `compute_many`) dropped from 9/8 positional parameters to 7/6, in response to concrete feedback
  from an actual downstream library consumer after integrating against the old signature.
  - The `ci_method: CiMethod, bootstrap_method: BootstrapMethod` pair is replaced by a single
    `metric: MetricConfig` parameter - `MetricConfig::WinRate { ci_method }` /
    `SignTest { ci_method }` / `MeanDiff { bootstrap_method }` / `Elo`. Previously, `elo` accepted
    (and silently ignored) both parameters, and `mean-diff` required `ci_method: CiMethod::Wilson`
    to avoid a runtime `IncompatibleCiMethod` error despite never reading it - passing an
    irrelevant parameter for a given metric is now a compile error, not a footgun. Use
    `MetricConfig::new(kind, ci_method, bootstrap_method)` to construct one from flat,
    CLI-flag-shaped inputs (this performs the same validation `compute_many` used to run
    internally on every call, just once, at construction).
  - `records: I where I: IntoIterator<Item = Result<(usize, Record), VeridictError>>` widens to
    `I: IntoIterator where I::Item: IntoRecordResult` - a caller that already has valid,
    already-parsed records in memory can now pass `records.iter().cloned()` directly, instead of
    `.iter().cloned().map(Ok)` just to satisfy the old bound. The streaming-parse use case
    (`Result`-yielding iterators) is unaffected.
  - No JSON/Markdown report format changed - `Report.metric` still serializes as the same plain
    string it always did (`MetricConfig::kind()` recovers the existing `MetricKind` for anything
    report-shaped). This is a Rust-API-only change.

### Added

- `--ci-method jeffreys` for `winrate`/`sign-test`: a Bayesian credible interval using the
  non-informative Jeffreys prior, alongside `wilson` (default) and `exact`.
- `--bootstrap-method basic` for `mean-diff`/`matrix`, alongside `percentile` (default) and `bca`.
- `--sprt-variant trinomial` for `sprt`: a draw-aware generalized LLR test (BayesElo
  parameterization), draw rate estimated as a nuisance parameter (`drawelo`) - converges faster
  than the default `wald` variant on draw-heavy data. Separate `--belo0`/`--belo1` flags (BayesElo
  is a different scale from logistic Elo unless the estimated draw rate is exactly zero).
- `matrix --matches`: named head-to-head records (`{id, a, b, result}`) for direct
  candidate-vs-candidate data, not just each candidate vs. a shared baseline. Once the graph is no
  longer star-shaped, `matrix` fits a general Bradley-Terry model (Zermelo/Hunter MM fixed point)
  instead of the star-graph closed form, with `direct`/`inferred`/`disconnected` cell status and
  real bootstrap CIs on general-graph pairwise Elo differences.
- `schema_version` (currently `1`) and `data_quality` (structured booleans alongside the existing
  string `warnings`) fields on every report.
- Fully parallel bootstrap resampling for `matrix`'s general-graph solver (~3.8x speedup at
  N=100/resamples=2000/10 cores), invariant to worker/thread count.
- `schemas/`: JSON Schema (2020-12) for every report and record type, validated against real CLI
  output.
- `docs/metrics.md`/`metrics_ja.md` (per-metric statistical basis, assumptions, failure modes) and
  `docs/research-map.md`/`research-map_ja.md` (methods considered but not shipped, and what's
  deliberately out of scope).
- `examples/paired_scores.csv` (first CSV example) and
  `examples/chess_engine_draw_heavy.jsonl` (first trinomial-SPRT-oriented example).
- README/README_ja "Statistical basis" section citing the academic source behind each metric.

## [0.1.0] - already on crates.io

The first published release. Covers:

- **`compare`**: `winrate`, `sign-test`, `mean-diff`, and `elo` metrics, each producing an effect
  size and confidence interval checked against `--pass-above`/`--fail-below` (or symmetric
  `--min-effect`).
  - CI methods for `winrate`/`sign-test`: `--ci-method wilson` (default) or `exact`
    (Clopper-Pearson).
  - Bootstrap methods for `mean-diff`: `--bootstrap-method percentile` (default) or `bca`
    (bias-corrected and accelerated). `--resamples`/`--seed` control the bootstrap.
  - Multiple `--metric` flags in one run scan the input once and combine into an aggregated
    verdict (`MultiReport`).
  - Report extras: `estimated_additional_trials` and `warnings` (human-readable data-quality
    flags).
- **`sprt`**: the classic two-outcome Wald sequential probability ratio test over decisive trials.
- **`matrix`**: pairwise comparison across more than two candidates, each measured against a
  shared baseline - closed-form Elo-vs-baseline per candidate, no iterative solver.
- **`--paired-by-id`** (`compare`/`sprt`/`matrix`): nets two records sharing an `id` into one
  observation instead of two independent ones.
- **Input**: JSONL (default) or CSV (`--format csv`, or auto-detected from `.csv`), streaming
  (bounded memory regardless of input size).
- **Reports**: JSON (default) and Markdown (`--report-md`), with a `failure_breakdown` split by
  which side timed out/crashed/produced an invalid result.
- Bilingual README (`README.md`/`README_ja.md`), dual MIT/Apache-2.0 license, CI
  (`fmt`/`clippy -D warnings`/`test --all-features`/`cargo audit` on every push and PR).
