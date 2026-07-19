# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

`veridict` is a domain-agnostic statistical decision gate: feed it paired candidate-vs-baseline
results (JSONL or CSV) and it returns `pass`/`fail`/`inconclusive`, never a false pass dressed up
as a pass - see [`docs/metrics.md`](docs/metrics.md) for the statistical basis of every number it
reports and [`docs/research-map.md`](docs/research-map.md) for what's deliberately out of scope.

## [0.14.0] - 2026-07-20

### Changed

- **Breaking: split the deployment-gate verdict from family-adjusted metric claims, and renamed
  `--correction` to `--claim-correction`.** `--correction`'s statistical target was always a
  family of *simultaneous per-metric claims*, not `verdict::aggregate`'s combined result (an
  intersection-union rule requiring every metric to pass is already at least as conservative as a
  single metric, with no correction needed of its own - see 0.13.0's entry below). But the report
  shape didn't reflect that: `apply_correction` mutated each report's own `verdict` in place, so a
  claim-correction downgrade could still pull the whole run's combined `verdict`/`promotion` down
  as a side effect of sharing one field, not by statistical necessity. Now: `Report.verdict`/
  `promotion` and `MultiReport.verdict`/`promotion` are the deployment gate, always computed from
  *unadjusted* per-metric verdicts and never touched by `--claim-correction`, no matter what's
  passed. Correction instead populates `Report.family_adjusted_verdict`/
  `family_adjusted_promotion` and a new `MultiReport.simultaneous_claims_promotion` (`promoted`
  only if every report's `family_adjusted_promotion` is too) - the field to read if "does every
  metric's own improvement claim individually survive being read as part of the family" matters on
  its own. `Report.unadjusted_verdict` is retained as a deprecated compatibility alias for
  `verdict` (it now always equals `verdict`, since `verdict` is never adjusted) so an existing
  consumer of a corrected report doesn't see a field vanish; new consumers should migrate to
  `verdict` directly, and it's scheduled for removal alongside a future `REPORT_SCHEMA_VERSION`
  bump, not this round. `--correction` is kept as a deprecated alias for one release (prints a
  warning to stderr; the two flags are mutually exclusive). See
  [`docs/metrics.md`](docs/metrics.md)'s `--claim-correction` section.
- **Breaking: `--claim-correction`/`--correction` now reject a family containing `--metric
  mean-diff`/`quantile-diff` as a configuration error (exit code 3)**, instead of silently leaving
  such a report uncorrected while still counting it toward `family_size` - the same "reject
  outright rather than leave an incomplete guarantee" call 0.13.0 made for `--cluster-by-id`,
  applied consistently to the other gap in the same family-wise claim. Bootstrap-aware correction
  is a separate, deferred piece of work.
- **Correctness dependency, now explicit: failure caps run before claim correction.**
  `verdict::apply_failure_caps`/`apply_failure_caps_to_multi` finalize `validity`/`verdict`/
  `promotion` *before* `correction::apply_correction`/`apply_correction_to_multi` ever runs, so a
  report already forced to `Inconclusive` for a technical failure can never count as a legitimate
  statistical claim just because its raw numeric verdict looked clean.

## [0.13.0] - 2026-07-20

### Fixed

- **`--correction` combined with `--cluster-by-id` no longer silently miscorrects.**
  `achieved_alpha` (the binary search `--correction bonferroni`/`holm` both use) recomputes a
  report's CI at a hypothetical confidence from `successes`/`paired_count` alone - the closed-form
  Wilson/Jeffreys/Exact math `compare` uses when *not* clustering. A `--cluster-by-id` report's
  actual, displayed CI comes from a cluster bootstrap instead, which that reconstruction can't see:
  under positive intra-cluster correlation (the usual case, and the whole reason `--cluster-by-id`
  widens the CI to begin with), the i.i.d. reconstruction comes out *narrower* than the true
  cluster-robust CI, so a report could read as more significant than it really is - correction
  could then under-downgrade a pass it should have caught, leniency in exactly the direction this
  project's own "false pass worse than inconclusive" bias forbids. `compare --correction
  bonferroni|holm --cluster-by-id` is now rejected outright as a configuration error (exit code 3,
  `VeridictError::CorrectionConflictsWithClusterById`) rather than silently applying a mismatched
  correction. `--correction none` (the default) is unaffected. Cluster-aware correction is a
  separate, deferred piece of work - see `docs/research-map.md`.

### Changed

- **Breaking:** `correction::apply_correction` now returns `Result<(), VeridictError>` instead of
  `()`, to surface the new `CorrectionConflictsWithClusterById` error to library callers, not just
  the CLI. Any caller must now handle (or `?`-propagate) the `Result`.
- Docs (`README.md`/`README_ja.md`, `docs/metrics.md`/`docs/metrics_ja.md`,
  `docs/research-map.md`) now distinguish `--correction`'s statistical target (a family of
  *simultaneous per-metric* claims - `verdict::aggregate`'s combined result doesn't need it, since
  requiring every metric to pass is already at least as conservative as a single metric,
  correction or not) from what it currently touches (the combined verdict still moves today, as a
  side effect of correction mutating each report's `verdict` in place before re-aggregation).
  Separating a deployment-oriented aggregate gate from a family-adjusted simultaneous-claims result
  is planned as an independent follow-up - see `docs/research-map.md`'s new entry.

## [0.12.0] - 2026-07-19

### Added

- **`validity`/`promotion` report fields, plus `--max-timeouts`/`--max-crashes`/`--max-invalid`
  hard failure caps** (`compare` and `sprt`). `validity` (`valid`/`invalid`) is a new axis
  separate from `verdict`: whether this run's data is trustworthy enough to read a verdict off of
  at all, independent of what that verdict says. Breaching a configured cap forces `verdict` back
  to `inconclusive` (never a possibly-misleading `pass`/`fail` - concretely, under
  `--failure-policy loss`, enough crashes can otherwise tip a numeric verdict to `fail` even
  though the real cause was infrastructure, not candidate strength) and rewrites `reason` to say
  why. `promotion` (`promoted`/`not_promoted`) collapses `validity`+`verdict` into the one field a
  deployment pipeline should actually gate on: `promoted` only when both are clean. Unlike
  `data_quality.high_failure_rate` (a rate-based, purely advisory warning), these caps are
  absolute counts - zero-tolerance-style gates (`--max-crashes 0`) that matter regardless of how
  many clean trials surround a single technical failure. Applied as a final pass over an
  already-built report (`verdict::apply_failure_caps`/`apply_failure_caps_to_multi`,
  `sprt::apply_failure_caps`), the same "mutate a finished report" shape `--correction` already
  uses. Unset (the default): uncapped, existing behavior, byte-identical JSON. See
  [Validity, strength, and promotion](README.md#validity-strength-and-promotion) and
  `docs/metrics.md`'s `--max-timeouts`/`--max-crashes`/`--max-invalid` section.
- **`power --sprt --horizon N`**: a Monte Carlo estimate (`probability_no_decision_by_horizon`,
  2,000 replications, fixed seed) of how often a real `veridict sprt` run still won't have reached
  a decision after `N` decisive trials, evaluated at the realistic worst-case true strength
  (halfway between `--elo0`/`--elo1`, not either endpoint - the same peak
  `expected_trials_under_h0`/`expected_trials_under_h1`'s own doc already establishes). A planning
  number for the next gate's trial budget/cutoff, not a stopping rule - a real run's own
  `--alpha`/`--beta` boundaries already fully determine when it stops. Omitted entirely (not
  null) unless `--horizon` is given. See `docs/metrics.md`'s `power --sprt` section.
- **`compare --cluster-by-id`** (`--metric winrate`/`--metric elo` only): a cluster bootstrap CI
  for records sharing a common source of correlation (the same opening/testcase replayed several
  times), instead of a single pair. Structurally different from `--paired-by-id` (nets exactly two
  records into one observation) - clustering keeps every record but resamples whole id-groups with
  replacement instead of individual records, correctly widening the CI when trials aren't truly
  independent; mutually exclusive with `--paired-by-id`. Adds `cluster_count`/`max_cluster_size`
  (plain descriptive stats) and `effective_sample_size`/`design_effect` (Kish 1965 - both derived
  from the *same* cluster-vs-i.i.d. bootstrap comparison, not a separately computed ICC, so the
  numbers can't silently disagree with the CI they describe) to the report.
  `mean-diff`/`sign-test`/`quantile-diff` cluster support is deferred (see
  `docs/research-map.md`) - those metrics bootstrap by individual record today, not by outcome
  tally, so real support needs separate collector wiring. See
  [Clustered testcases](README.md#clustered-testcases) and `docs/metrics.md`'s `--cluster-by-id`
  section, including a worked example where the naive per-game CI reads as a confident `pass` and
  the cluster-aware one correctly reads `inconclusive` on identical data. `estimated_additional_trials`
  is always `null` under `--cluster-by-id`, even when `inconclusive` - it would otherwise
  binary-search wilson/jeffreys/exact against a report whose displayed CI is a cluster bootstrap,
  and `paired_count` isn't the right `n` to scale from when the independent unit is the cluster.

### Changed

- **Breaking**: `compare_one`/`compare_many` (and the lower-level `metrics::compute`/
  `compute_many`) each gained a new trailing `cluster_by_id: bool` parameter - this only affects
  direct library callers, not the CLI or any JSON schema (every new report field above is
  additive; `schema_version` stays `1`).

## [0.11.0] - 2026-07-12

Purely additive - no breaking changes since `0.10.0`.

### Added

- `compare --metric quantile-diff --quantile Q` (default `0.5`, the median): a bootstrap
  confidence interval on an arbitrary quantile of `candidate - baseline`, generalizing
  `mean-diff` from the mean to a quantile - useful where a p95/p99 latency-style regression
  matters more than the average. Type-7 linear interpolation (R's/NumPy's default quantile
  convention); `--quantile` must be strictly inside `(0, 1)`. Shares `mean-diff`'s
  `--resamples`/`--seed`/`DiffCollector` machinery and `--paired-by-id` netting.
  `--bootstrap-method percentile`/`basic` are supported; `bca` is implemented but rejected as a
  config error (`IncompatibleBootstrapMethod`) - the sample quantile is a non-smooth statistic,
  so BCa's jackknife acceleration has no solid asymptotic footing the way it does for the mean,
  confirmed (not just theorized) by `tests/calibration/quantile_coverage.rs`'s new coverage
  simulations. `estimated_additional_trials`/`--correction` treat it exactly like `mean-diff`
  (no closed-form CI-at-n/CI-at-confidence function for either's bootstrap CI). New
  `data_quality.thin_quantile_tail` warning fires when the requested quantile's tail has fewer
  than 10 expected observations. `power`/`matrix`/`plan` support is deferred (see
  `docs/research-map.md`) - a genuinely different, harder problem than mirroring `mean-diff`'s.
- A weekly (plus PR-triggered on `src/stats/**`/`tests/calibration/**` changes, plus
  on-demand) `calibration.yml` CI workflow now actually runs the `#[ignore]`d bootstrap/quantile
  coverage-calibration tests, which previously only ran when someone happened to invoke them
  locally.

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
