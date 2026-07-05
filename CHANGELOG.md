# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

`veridict` is a domain-agnostic statistical decision gate: feed it paired candidate-vs-baseline
results (JSONL or CSV) and it returns `pass`/`fail`/`inconclusive`, never a false pass dressed up
as a pass - see [`docs/metrics.md`](docs/metrics.md) for the statistical basis of every number it
reports and [`docs/research-map.md`](docs/research-map.md) for what's deliberately out of scope.

## [Unreleased]

Everything below has landed on `master` but is not yet in a published crates.io release - `0.1.0`
on crates.io predates all of it (confirmed against the published build on docs.rs). No version
number has been decided yet; `0.2.0` would be the natural next version under semver pre-1.0
(crates.io does not allow republishing an existing version regardless).

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
