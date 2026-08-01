# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

`veridict` is a domain-agnostic statistical decision gate: feed it paired candidate-vs-baseline
results (JSONL or CSV) and it returns `pass`/`fail`/`inconclusive`, never a false pass dressed up
as a pass - see [`docs/metrics.md`](docs/metrics.md) for the statistical basis of every number it
reports and [`docs/research-map.md`](docs/research-map.md) for what's deliberately out of scope.

## [Unreleased]

### Changed

- `README.md`/`README_ja.md`: dropped the crates.io downloads and GitHub-stars badges added in
  0.17.0 - on a crate this new, both render as near-zero counts that undercut credibility rather
  than build it. `docs.rs` stays (it's a build/link badge, not a vanity count).

## [0.17.0] - 2026-08-01

### Added

- **`--metric relative-diff`**: bootstrap confidence interval on `(candidate - baseline) /
  baseline` - proportional change relative to the baseline, for benchmarks where problem size or
  raw score varies a lot across cases and an absolute `mean-diff` would let large-scale cases
  dominate the variance. Independent `MetricConfig`/`MetricKind` variant, not a `mean-diff`
  modifier flag: different effect-size units (a ratio, not raw input units), a different baseline
  constraint (`baseline > 0`, required - rejected as `VeridictError::RelativeDiffRequiresPositiveBaseline`
  otherwise), different Markdown rendering (`+6.3%`, not winrate's `+5.0 pp`), and independently
  auditable power/claim-correction support. Supports all three `--bootstrap-method` variants
  (`percentile`/`basic`/`bca`), `--paired-by-id` (relative-transforms each record first, then nets
  same-id pairs by averaging the two ratios - not the ratio of averaged baseline/candidate), and
  the same `--resamples`/`--seed` reproducibility guarantee as `mean-diff`. Not supported this
  round: `veridict power --metric relative-diff` (clear `VeridictError::PowerUnsupportedForRelativeDiff`
  rather than a silent reuse of `mean-diff`'s assumed-SD design) and `--claim-correction` (rejected
  the same way `mean-diff`/`quantile-diff` already are - no closed-form CI to correct against) -
  see `docs/research-map.md` for what's deferred and why.
- **`mean-diff` scale-mismatch diagnostic**: `data_quality.wide_baseline_scale` (advisory, never
  changes `verdict`) fires when `mean-diff`'s baselines are all positive and span at least
  `WIDE_BASELINE_SCALE_ORDERS` (1.0, i.e. roughly 10x) orders of magnitude - a sign the absolute
  difference may be dominated by the largest-scale cases, with a warning suggesting
  `--metric relative-diff` as an alternative to consider *before* confirmatory analysis (switching
  metrics after inspecting a verdict is flagged as exploratory, not a recommendation to switch). A
  different warning fires when the wide-scale baselines include a zero/negative value (where
  `relative-diff` isn't well-defined either). Computed from baseline values alone - never
  candidate, effect, CI, or verdict - so it can't be, even accidentally, a function of the observed
  result. The underlying numbers (`positive_baseline_count`, `non_positive_baseline_count`,
  `min_positive_baseline`, `max_positive_baseline`, `raw_orders_of_magnitude`,
  `robust_orders_of_magnitude`) are exposed as `Report.scale_diagnostics`, for both `mean-diff` and
  `relative-diff` (the latter never sets `wide_baseline_scale` on itself, but still reports the
  numbers for transparency). At 20+ positive baselines, a p95/p05-based robust span becomes the
  primary signal instead of the raw min/max span, so one extreme outlier can't single-handedly
  trigger the warning.

### Changed

- `Report`/`schemas/compare-report.schema.json` gain the additive `data_quality.wide_baseline_scale`
  field (always present, `false` for every metric other than `mean-diff`) and the optional
  `scale_diagnostics` field (`mean-diff`/`relative-diff` only). `REPORT_SCHEMA_VERSION` stays `1`
  per this project's existing "additive changes don't bump it" policy - existing consumers reading
  known fields are unaffected; `compare_correction_none`/`compare_claim_correction_holm` golden
  fixtures were regenerated to reflect the new `data_quality` key (the only diff).
- `Cargo.toml`: added a `documentation` field pointing at docs.rs, and reworded `description` to
  name "A/B and hypothesis testing" explicitly (crates.io/GitHub search discoverability only, no
  code change). `README.md`/`README_ja.md` gained `docs.rs`, crates.io downloads, and GitHub-stars
  badges alongside the existing CI/version/license ones.

## [0.16.1] - 2026-07-27

### Changed

- `Cargo.toml`: added `mathematics`, `science`, and `algorithms` to `categories` (crates.io
  discoverability only, no code change) - `command-line-utilities`/`development-tools::testing`
  alone undersold the statistical/algorithmic core (SPRT, Bradley-Terry MM, bootstrap resampling,
  Bellman value iteration). `science` matches the convention `statrs` (a direct dependency) itself
  uses; `algorithms` matches `rand`'s. `visualization`/`simulation`/`command-line-interface` were
  considered and rejected as not actually describing what this crate does.

## [0.16.0] - 2026-07-27

### Added

- **`veridict time-sensitive`**: time-sensitive anytime-valid testing, an experimental Bernoulli
  specialization of a betting-policy framework that maximizes expected reward under a schedule
  favoring an *early* rejection (a deadline, or a decaying value of waiting), while keeping the
  exact same alpha-level type-I error guarantee `sprt` has, for any betting choice. Independent
  addition, not a replacement: `sprt`/`compare`/`power`'s behavior, JSON, and public API are
  unchanged.

  This implementation is independently derived from the mathematical framework in E. Clerico,
  T. Wegel, I. Azangulov, and P. Rebeschini, "Time-sensitive anytime-valid testing,"
  arXiv:2605.06521v1, 2026 (CC BY 4.0) - see `src/time_sensitive/mod.rs`'s module doc and the
  README's "Time-sensitive testing" section for the full attribution. The EDO policy in
  particular is an independently derived Bernoulli specialization based on the paper's moment
  characterization: a first reading of the paper's general-case formula was internally
  inconsistent with its own stated limits, so the closed form actually shipped here was
  re-derived by hand from the moment equation and checked against both boundary conditions
  (`eta -> 0` recovers GRO; `eta` in `(0,1)` bets more aggressively than GRO), not transcribed
  from the paper's notation verbatim. It is not claimed to reproduce every notation or
  implementation detail of the paper.

  **Scope is deliberately narrow: Bernoulli simple-vs-simple only** (a fixed null `p0` and
  alternative `p1`, `0 < p0 < p1 < 1`, one-sided rejection). Three policies (`--policy`), with
  different, explicitly-labeled guarantees:
  - `gro` (growth-rate-optimal) - the classical anytime-valid baseline, betting-equivalent to
    (but implemented independently of) `sprt`'s own Wald log-likelihood-ratio walk. Ignores the
    reward schedule; the large-`time_scale` limit of `edo`.
  - `bellman` - a numerical approximation of the time-sensitive-optimal policy on a finite
    `(action_grid_size, wealth_grid_size)` grid, for any reward schedule. Reported as
    `bellman_grid_approximation`, never claimed optimal - a grid approximation, not a proof of
    optimality on the continuous action/wealth space.
  - `edo` (exponential-decay-optimal) - a closed-form *stationary* approximation, valid only for
    `--reward exponential`. Reported as `edo_stationary_approximation`, never claimed
    Bellman-optimal.

  Anytime-validity (the alpha-level guarantee) is exact regardless of which policy is chosen or
  how coarse its grid/approximation is: every policy reduces to picking an action in `(0,1)` each
  trial and multiplying wealth by a Bernoulli e-variable that is valid under H0 for *every*
  action, not just the "correct" one - approximation error can only ever cost optimality, never
  validity. This separation is proved exactly (not simulated) for small horizons via exhaustive
  `2^T`-path enumeration, and checked at realistic scale (`T=400`) via Monte Carlo calibration.
  Optimality (which policy actually maximizes reward) does depend on the configured alternative
  `p1` matching reality and on the numerical approximation settings (`--action-grid-size`/
  `--wealth-grid-size`) - a separate claim from anytime-validity, documented as such in every
  report's `notes`.

  Reward schedules: `--reward hard-deadline --deadline N` (reward 1 up to trial `N`, 0 after),
  `--reward exponential --time-scale T --horizon N` (`R(t) = exp(-t/T)`, truncated at `N`), or a
  custom `--reward-schedule FILE.json` (sparse trial-count breakpoints, expanded internally; the
  tail value must be `0.0`). `Draw` results advance the trial count and the reward-schedule clock
  without moving the wealth process - a draw is a consumed trial, not a free one, and `p0`/`p1`
  are decisive-observation-conditional probabilities, the same convention `sprt --sprt-variant
  wald` uses.

  Reuses `Outcome`/`FailurePolicy`/`FailureCaps`/`Verdict`/`Validity`/`Promotion` unchanged - no
  parallel types. `--failure-policy`/`--max-timeouts`/`--max-crashes`/`--max-invalid` work exactly
  as they do for `sprt`. Exit codes are `0` (pass) / `2` (inconclusive) / `3` (config/input error)
  - never `1`: there is no two-sided fail in this model, so a horizon reached without crossing is
  never read as a statistical fail.

  **Known limitation, not addressed this round:** `planned_expected_reward_under_p1` is computed
  under an all-decisive idealization (the value recursion has no draw-rate input), so on a
  draw-heavy stream the realized reward will lag this number. Trinomial/pentanomial (draw-aware)
  time-sensitive policies, composite hypotheses, online `p1` estimation, `--elo0`/`--elo1` input,
  and a boundary-aware correction to EDO near the wealth threshold are all out of scope this round
  - see `docs/research-map.md` for what would change each of these.

### Changed

- `CITATION.cff` now references arXiv:2605.06521 (CC BY 4.0) as the mathematical source for
  `time_sensitive`, alongside veridict's own citation metadata.

## [0.15.0] - 2026-07-26

### Added

- **`sprt --sprt-variant pentanomial` now performs genuine sequential replay, not a
  final-aggregate recompute.** `sprt::run` walks completed pairs in the order they complete and
  recomputes the LLR from cumulative bucket counts after every pair, stopping at the first pair
  that decides - the same method Fishtest itself uses for live monitoring. This closes a
  look-ahead gap in a single static input file: a boundary crossed mid-file and drifted back
  within bounds by the file's end used to be reported as `inconclusive` (a naive final-aggregate
  check can't tell the two apart); it is now correctly `pass`/`fail`, decided at the true stopping
  pair. `llr`/`pentanomial_counts`/`candidate_wins`/`baseline_wins`/`draws` now reflect only the
  pairs analyzed up to that point, never the full input. New report fields:
  `available_paired_count` (total pairs present in the input), `stopping_pair_count`/
  `stopping_reason` (where and why the walk stopped, one of `upper_bound_crossed`/
  `lower_bound_crossed`/`max_paired_ids_reached`/`insufficient_data`), and
  `ignored_pairs_after_stop` (pairs present in the input but completed after the stopping point).
  `paired_count` now means pairs actually analyzed (may be less than `available_paired_count`).
  **Known limitation, not addressed this round:** `wald`/`trinomial` still compute their LLR from
  the final aggregate only, and carry the same theoretical look-ahead gap (their per-trial LLR
  delta is a fixed constant, which only means the *final total* is order-independent, not that a
  final check is equivalent to true sequential stopping) - upgrade them the same way if this is
  ever reported as an issue for those variants.
- **`sprt --min-paired-ids`/`--max-paired-ids`** (`--sprt-variant pentanomial` only, folded into
  the sequential walk above): `--min-paired-ids` (must be `>= 1`) means the walk never evaluates a
  boundary crossing before this many pairs have completed, so a crossing seen on too little data
  can never decide the verdict once the minimum is reached - the decision is always the LLR at
  the pair the walk actually stops on. `--max-paired-ids` (must be `>= --min-paired-ids` when both
  are given) stops the walk with `inconclusive` if reached without a crossing; later pairs, even
  if present in the input, never affect the verdict. Strictly conservative relative to the
  nominal `--alpha`/`--beta`, never less so (see `docs/metrics.md`). Both echoed on the report as
  `min_paired_ids`/`max_paired_ids`.
- **`sprt --require-complete-pairs`** (requires `--paired-by-id`): extends `pentanomial`'s
  existing unconditional "every id appears exactly twice" pairing to `wald`/`trinomial`, which
  otherwise tolerate a lone id as an ordinary unpaired sample. A documented no-op for
  `pentanomial`, which is already this strict regardless of the flag. Echoed as
  `require_complete_pairs`.
- **`veridict verify-run manifest.toml games.jsonl`**: checks that a run's raw data is
  structurally sound *before* `compare`/`sprt`'s statistics are trusted on it. Six checks run
  unconditionally, collecting every violation rather than stopping at the first:
  `pair_completeness` (every id appears exactly twice), `global_index_uniqueness` (a
  caller-declared per-record sequence number never repeats), `role_consistency` (the two records
  sharing an id declare two different `role` values - the domain-agnostic stand-in for
  "color/side reversed within a pair" - and only one `role`-pairing convention is used across the
  run), `schedule_order` (the actual gated pair order matches a declared `manifest.toml`
  schedule), `experiment_contamination` (no record's `experiment_id` disagrees with the
  manifest's - catches a different run's games mixed in), and `environment_consistency`
  (`dataset_sha256`/`binary_sha256`/`weight_sha256`/`config_sha256` each checked independently
  for drift between the manifest's declared value and what records repeat). Self-consistency
  only: every hash/identifier is an opaque caller-supplied string, compared for equality/no-drift
  - `verify-run` never opens, hashes, or interprets an actual binary/weight/corpus/config file.
  A check whose backing field never appears anywhere is honestly reported as skipped
  (`checks_skipped`/`warnings`), never a silent pass or a hard failure. Its own exit-code
  contract, not the usual verdict-based one: `0` (no violation), `1` (one or more violations,
  `validity: "invalid"` - there's no "inconclusive" for a structural invariant), `3` only for a
  genuine parse/config error (malformed manifest, unsupported `manifest_schema_version`,
  malformed `games.jsonl`, empty input, or a manifest declaring nothing to verify) where no
  report can be produced at all. `paired_count` is the formal, *gated* pair count: burn-in pairs
  (either record marked `include_in_gate: false`) are excluded, though they're still checked like
  any other pair by `pair_completeness`/`role_consistency`. `violations` is sorted
  deterministically, then capped at 500 entries so a badly corrupted run can't produce a report
  proportional to the whole input; `violation_count`/`violations_truncated` carry the exact total
  and whether the array above was cut short.
- **`schemas/experiment-envelope.schema.json`**: a standalone, cross-repo contract of 14 opaque
  identifier/hash/seed fields (`experiment_id`, `candidate_id`, `baseline_id`, `lineage_id`,
  `dataset_sha256`, `split_sha256`, `teacher_manifest_sha256`, `binary_sha256`, `weight_sha256`,
  `init_seed`, `split_seed`, `shuffle_seed`, `schema_version`, `validity`), meant to be vendored
  verbatim by other tools around veridict so a downstream tool can join a verdict back to its own
  provenance store without any tool needing to understand another tool's identifiers.
  `schemas/manifest.schema.json`/`schemas/verify-run-record.schema.json`/
  `schemas/verify-run-report.schema.json` document `verify-run`'s own input/output shapes.

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
