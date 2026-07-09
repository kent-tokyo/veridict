# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

`veridict` is a domain-agnostic statistical decision gate: feed it paired candidate-vs-baseline
results (JSONL or CSV) and it returns `pass`/`fail`/`inconclusive`, never a false pass dressed up
as a pass - see [`docs/metrics.md`](docs/metrics.md) for the statistical basis of every number it
reports and [`docs/research-map.md`](docs/research-map.md) for what's deliberately out of scope.

## [0.10.0] - 2026-07-10

Purely additive - no breaking changes since `0.9.0`.

### Added

- `power --metric mean-diff`, given `--assume-sd <f64>` or `--pilot FILE` (real paired
  baseline/candidate data to estimate a sample standard deviation from). Unlike
  winrate/sign-test/elo's exact binomial search, this is a closed-form calculation - mean-diff has
  no closed-form CI-width-at-n function to search against, so given an assumed/estimated standard
  deviation of the paired difference, `n = ceil(((z_conf + z_power) * assume_sd / (assume_effect -
  min_effect))^2)` (`z_conf`/`z_power` via the existing `inverse_normal_cdf`, `achieved_power` via
  `statrs::distribution::Normal`, the same pattern `stats::bootstrap`'s BCa already uses). `z_conf`
  is deliberately the *two-sided* confidence quantile, matching how `compare`'s own CI is built and
  read one-sidedly - the same correctness point that already shaped `power`'s two-effect-value
  design and `--correction`'s `alpha/2` family target, verified consistent here rather than
  re-derived. This is a normal approximation of the real bootstrap decision rule, not an exact
  search against it - `tests/calibration/power_mean_diff_calibration.rs` measures the real gap
  empirically. `--pilot`'s standard deviation is estimated via the same `DiffCollector`
  `compare --metric mean-diff` itself uses (including `--paired-by-id` netting), new
  `stats::bootstrap::sample_variance`/`sample_sd` helpers, and a clear error rather than a silent
  `NaN`/`0` for too-small or zero-variance pilot data. New `PowerReport` fields
  `assume_sd`/`sd_source`, omitted (not `null`) for every other metric - existing
  winrate/sign-test/elo/`--sprt` output is byte-identical.

## [0.9.0] - 2026-07-10

Purely additive - no breaking changes since `0.8.0`.

### Added

- `compare --correction none|bonferroni|holm`: multiple-comparison correction across a
  multi-`--metric` family, controlling the family's one-sided false-pass rate at or below what a
  single, uncorrected metric already has today (`alpha/2` at the default 95% confidence) - not the
  nominal `alpha` itself, which would let a "corrected" family tolerate a *higher* false-pass rate
  than a single uncorrected metric already does (see `docs/metrics.md`'s `--correction` section).
  Default `none` - today's existing behavior, byte-identical, unless opted into. Bonferroni applies
  a uniform `alpha/family_size` significance budget; Holm (recommended) step-down sorts by achieved
  significance and stops rejecting at the first failure, uniformly more powerful than Bonferroni
  for the same guarantee. Both share one `achieved_alpha` binary search (via the standard CI-test
  duality, against the same real Wilson/Clopper-Pearson/Jeffreys functions `compare` already uses)
  rather than separate math. Correction can only downgrade an unadjusted `pass` to `inconclusive`,
  never invent a `fail` - a direct consequence of widening a CI, not a special case. `mean-diff`
  can't be individually corrected (no closed-form CI at a hypothetical confidence for a bootstrap
  interval) but still counts toward `family_size`. New `Report` fields, all omitted (not `null`)
  unless `--correction` is active: `correction_method`, `family_size`, `achieved_alpha`,
  `adjusted_alpha_threshold`, `unadjusted_verdict` (`verdict` itself becomes the adjusted value).
  `matrix`'s all-pairs correction is out of scope for this round - see `docs/research-map.md`'s new
  "matrix verdict semantics" entry for the open design questions.

## [0.8.0] - 2026-07-09

Purely additive - no breaking changes since `0.7.0`.

### Added

- `veridict power --sprt`: estimates the expected number of trials to an SPRT decision under each
  hypothesis (Wald's classical Average Sample Number approximation), given `--elo0`/`--elo1`/
  `--alpha`/`--beta` - the same inputs `veridict sprt --sprt-variant wald` itself takes
  (`SprtConfig::new` reused directly, so a bad `elo0 >= elo1` produces the exact same error `sprt`
  itself would). Structurally different from `power`'s existing CI-crossing-probability mode -
  Wald's alpha/beta already fix the guaranteed error rates, so there's no target power to search a
  sample size for - so this is a new `--sprt` flag mutually exclusive with
  `--metric`/`--min-effect`/`--assume-effect`/`--confidence`/`--target-power`/`--ci-method`, not a
  new `--metric` value. The formula's `alpha'(H)`/boundary pairing was corrected before
  implementation (an earlier draft had it backwards, which would have produced a negative expected
  sample size under H1 - see `docs/research-map.md`). Two caveats measured empirically rather than
  left as cited theory (`tests/calibration/sprt_asn_calibration.rs`): Wald's ASN ignores
  "overshoot" (real runs need ~1-2% more trials than the formula predicts), and more significantly,
  `expected_trials_under_h0`/`expected_trials_under_h1` are the two optimistic endpoints, not the
  expected sample size for a candidate of unknown strength - ASN peaks between the two hypotheses,
  ~1.6x either endpoint at the same config. New `schemas/power-sprt-report.schema.json`. See
  `docs/metrics.md`'s new `power --sprt` section.

## [0.7.0] - 2026-07-09

Purely additive - no breaking changes since `0.6.0`.

### Added

- `veridict power`: estimates how many trials `compare --metric winrate/sign-test/elo` would need
  for a target probability (power) of reaching a passing verdict, before running any of them -
  report-only, no input file. Requires both `--min-effect` (the pass bar, same meaning as
  `compare`) and `--assume-effect` (the true effect being powered for, must exceed `--min-effect`)
  - evaluating power with the two equal only recovers the CI's own miscoverage at that boundary,
  not a useful number; `power` rejects that combination as a hard error rather than returning a
  misleading one. `estimated_trials` is found by an exact search against the real `wilson`/
  `exact`/`jeffreys` CI functions `compare` itself uses (`elo` accepts only `wilson`), not a
  textbook approximation - verified empirically via a new Monte Carlo calibration test
  (`tests/calibration/power_calibration.rs`). `mean-diff` and `sprt`'s own expected-sample-size are
  out of scope this round (structurally different, deferred - see `docs/research-map.md`). New
  `schemas/power-report.schema.json`. See `docs/metrics.md`'s new `power` section for the full
  reasoning, including why two effect values are required and the discrete "sawtooth"
  non-monotonicity caveat (Chernick & Liu 2002).

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
