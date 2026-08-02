# Research map

English | [日本語](research-map_ja.md)

Candidates we've considered, why they aren't shipped (yet or ever), and what would change the
decision. This is a "deferred, not rejected" list, not a roadmap - a future request to revisit any
of these is legitimate, not a re-litigation of a closed question. See `docs/metrics.md` for what
*is* implemented and its statistical basis.

## Statistical methods considered but not shipped

### BCa bootstrap for `quantile-diff`

**What it is:** the same bias-corrected-and-accelerated bootstrap `mean-diff`/`matrix` already ship
(`--bootstrap-method bca`), applied to a quantile instead of a mean.

**Why not yet:** implemented (`stats::bootstrap::bootstrap_quantile_diff_ci_bca`) but rejected as a
config error at the CLI (`VeridictError::IncompatibleBootstrapMethod`) rather than shipped enabled.
The sample quantile is a non-smooth statistic - the empirical quantile function is a step function
- so BCa's jackknife acceleration term has no solid asymptotic footing the way it does for the
mean, an established statistical caveat rather than a hunch. `tests/calibration/
quantile_coverage.rs` measures this directly: at p95/n=30 on skewed data, BCa's coverage (0.7910)
was statistically indistinguishable from plain percentile's (0.7940) - no measured benefit to
justify the extra complexity and jackknife cost of unlocking it.

**What would change this:** calibration evidence across more quantiles/sample sizes/populations
showing BCa's correction actually helps for a quantile in at least some regime (e.g. central
quantiles, or larger n), enough to justify a narrower unlock than "always allowed" - or a concrete
report that percentile/basic's coverage is inadequate in practice and BCa is worth the risk anyway.

### `power`/`matrix`/`plan` support for `quantile-diff`

**What it is:** `power --metric quantile-diff` (pre-experiment sample-size estimate for a quantile
gate) and `matrix`/`plan` support for `quantile-diff` as a pairwise comparison metric.

**Why not yet:** `power --metric mean-diff`'s closed-form calculation (see `docs/metrics.md`) relies
on the sample mean's asymptotic normality (`Normal(assume_effect, assume_sd^2/n)`) - a sample
quantile's asymptotic variance instead depends on the population density *at* that quantile
(`Var(quantile_hat) ≈ q(1-q) / (n * f(x_q)^2)`), which isn't estimable from a single assumed
standard deviation the way `mean-diff`'s is; it needs either a density estimate or a distributional
assumption `mean-diff`'s design deliberately avoids requiring. A genuinely different, harder
problem, not a mirror of `mean-diff`'s existing constructor. `matrix`/`plan` support is scoped out
for the same reason `--failure-policy` and multiple-comparison correction weren't extended to
`matrix` in earlier rounds - no concrete workflow has asked for it yet, and `matrix`'s own verdict
concept is itself still an open design question (see "Matrix verdict semantics" below).

**What would change this:** a concrete request for a pre-experiment sample-size estimate for a
quantile-based gate (most plausibly a latency/p95 regression-testing workflow), with enough detail
to settle whether a density-estimate or distributional-assumption approach fits the request better.

### `power --metric relative-diff`

**What it is:** `power --metric mean-diff`'s closed-form pre-experiment calculation (see
`docs/metrics.md`), applied to `relative-diff` instead of `mean-diff`.

**Why not yet:** structurally close to `mean-diff`'s existing `--assume-sd`/`--pilot` design, but
not a mechanical copy - `--assume-sd` would need to mean the standard deviation of *relative*
observations (`(candidate - baseline) / baseline`), not absolute ones, so a value someone already
has on hand for `mean-diff` power planning isn't reusable as-is for `relative-diff`; `--pilot` would
need to relative-transform each pilot record (`(candidate - baseline) / baseline`) before estimating
a standard deviation from the transformed sample, not the raw one; and that transform needs the same
`baseline > 0` validation `compare --metric relative-diff` enforces, applied to pilot data too (a
pilot file with a zero/negative baseline can't silently produce a usable SD estimate any more than a
real run can). None of this is hard, but it's real design surface that hasn't been independently
audited yet - `veridict power --metric relative-diff` currently returns a clear
`VeridictError::PowerUnsupportedForRelativeDiff` rather than quietly reusing `mean-diff`'s
constructor and getting the units wrong.

**What would change this:** a concrete request for pre-experiment sample-size planning against a
`relative-diff` gate, at which point the above three points become the actual design spec rather
than a list of open questions.

### Subset-only `relative-diff` (effect size restricted to non-tied cases)

**What it is:** a second, `relative-diff`-only diagnostic number - or possibly its own bootstrap
CI - computed over just the non-tied subset of ingested records (`candidate != baseline`), rather
than the full pooled sample `relative-diff` already reports. The motivating case: a change that
only affects a subset of test cases (e.g. one code path among several) contributes an exact
`diff = 0` for every untouched case, which pulls the pooled `mean(relative_diff_i)` toward zero and
makes a correctly-scoped, conservative change look weaker than it actually is among the cases it
touches. `compare --metric relative-diff`'s `data_quality.diluted_by_ties`/`tied_count` (see
`docs/metrics.md`) *detects* this dilution today - counts how many records tied exactly - but
doesn't correct for it; this entry is the deferred "and report the target-subset number too" half.

**Why not yet:** this directly borders `docs/metrics.md`'s "Method-selection discipline: pick the
metric before looking at results" principle - restricting to `diff != 0` records is a
data-dependent filter, even though it's arguably a legitimate distinction rather than result-driven
cherry-picking: the *full pooled* `relative-diff` answers an intent-to-treat-style question ("what
was the average proportional effect across everything tested, including cases the change never
touched"), while a *non-tied-subset* number answers a per-protocol-style question ("what was the
average proportional effect among cases the change actually affected"). Both are legitimate,
well-defined questions - but which records land in "non-tied" is only known after the run, unlike
`relative-diff` vs `mean-diff` itself (a metric choice made before seeing any data). That framing
needs to be written down and justified with the same rigor this project already gives
`scale_diagnostics` (computed from baselines alone, deliberately never conditioned on the observed
effect) before shipping a second effect size that *is* conditioned on the observed per-record diffs
- a category this project has not shipped before.

Open questions, not yet answered:

- **Diagnostic-only vs. a real second effect size.** `scale_diagnostics` is the existing precedent
  for "an extra number that's advisory/transparency-only and never touches `verdict`/CI" - a
  subset-only *count* or *point estimate* could follow that shape. A subset-only number with its
  *own* bootstrap CI (and potentially its own `verdict`) is a materially bigger claim - it would
  need the same design rigor `relative-diff` itself got as an independent `MetricConfig`/
  `MetricKind` (see `docs/metrics.md`'s "why this is an independent metric" section), not a
  mechanical reuse of the existing bootstrap machinery on a filtered sample.
- **CLI surface.** An always-computed report field (cheap, but changes every `relative-diff`
  report's shape) vs. an opt-in flag (e.g. something in the shape of `--exclude-ties`) that a
  caller requests explicitly - and what either would be named.
- **Denominator/counting-point question, for the effect-size case specifically.** `tied_count`
  (shipped) settled this for a simple *count*: at ingest, before `--paired-by-id` netting (so two
  opposite nonzero ratios netting to `0.0` under pairing is never mistaken for a genuine exact
  match). A subset-only *effect size* recomputation has its own, harder implications under
  `--paired-by-id` that a count doesn't: does filtering happen before or after netting, and does
  the bootstrap resample the filtered raw records or the filtered netted pairs? Not settled by the
  count-only design that shipped.

**What would change this:** someone doing that design pass, or a concrete follow-up request
specific enough to settle the diagnostic-vs-effect-size question above - e.g. a report where
`diluted_by_ties` fired and the pooled `relative-diff` alone wasn't enough to judge whether a
narrowly-scoped change should ship.

### Log-ratio metric (`log(candidate / baseline)`)

**What it is:** `log(candidate / baseline)`, bootstrapped the same way `relative-diff` bootstraps
`(candidate - baseline) / baseline` - a different effect size for the same "proportional change"
question, with a symmetry property `relative-diff` doesn't have: `log(c/b) = -log(b/c)`, so swapping
which arm is baseline and which is candidate exactly flips the sign, and a `+X` log-ratio followed by
a `-X` log-ratio returns exactly to the start (unlike `relative-diff`, where a +100% increase
followed by a -50% decrease returns to the original value - see `docs/metrics.md`'s `relative-diff`
section). For positive, multiplicatively-varying data, this symmetry is often the more natural
effect size, and it's the standard choice in fields (e.g. log-normal modeling) where ratios compound
multiplicatively.

**Why not yet:** deliberately not implemented alongside `relative-diff` this round, on purpose, not
by oversight - it's a genuinely different effect size (different units: log-ratio, not a percentage;
different center: `0` means "no change" for both, but `+0.05` means different things in each), and
shipping it in the same round as `relative-diff` risked blurring the "these are two distinct,
independently-justified metrics" line this project cares about (see `AGENTS.md`'s "avoid clever
one-liners" and this project's general preference for one well-understood metric shipped and
documented over two half-differentiated ones landing together). It shares `relative-diff`'s
`baseline > 0` requirement (and, less obviously, needs `candidate > 0` too - `log` of a non-positive
candidate is undefined, a constraint `relative-diff` doesn't have since it only divides, never takes
a log), so it isn't a drop-in swap even for data where both would otherwise apply.

**What would change this:** a concrete request for a symmetric proportional-change metric - most
plausibly from a workflow already using log-ratios elsewhere (e.g. log-normal latency/throughput
modeling) that wants `compare`'s pass/fail gate to speak the same units it already reports in.

### Wilson continuity correction (`winrate`/`sign-test`/`elo`)

**What it is:** a small-sample correction to the Wilson score interval that widens it slightly,
trading some conservatism for coverage accuracy at small n.

**Why not yet:** this was one of two accuracy investigations started under an earlier "improve
accuracy" priority; both stalled on transient tooling errors before returning a usable design, and
were superseded when a different priority (repo growth/discoverability) took over. Unlike the BCa
bootstrap (the other stalled investigation, since shipped for both `mean-diff` and `matrix`),
this one needs real care before resuming: the exact continuity-correction formula must be verified
against an authoritative source first, because `wilson_ci` is shared by three metrics
(`winrate`/`sign-test`/`elo`) - a wrong coefficient here would silently corrupt every confidence
interval built on top of it, not just one metric's.

**What would change this:** a concrete accuracy complaint at small n, or someone doing the formula
verification work up front.

### Normalized-Elo pentanomial, and the Siegmund discrete-time bound correction

**What it is:** `--sprt-variant pentanomial` (shipped) ports Fishtest's *logistic-Elo*
`LLR_logistic` (expectation-constrained multinomial MLE). Fishtest's newer default is
*normalized-Elo* (`LLR_normalized`, a `t`-value-constrained MLE with its own iterative solve,
scaled so a test's expected duration is invariant to draw rate/opening book) - a real refinement,
not needed for a statistically valid pentanomial test, just a different (and moderately more
complex) parameterization. Separately, Fishtest applies a Siegmund discrete-time correction on top
of the base LLR, tightening the bounds slightly for sequential tests that only check at discrete
intervals rather than continuously - `trinomial` doesn't apply this either, so `pentanomial`
matches existing precedent by not applying it, not by an oversight.

**Why not yet:** both are incremental accuracy refinements on an already-valid, already-shipped
test, not correctness fixes - see `docs/metrics.md`'s `sprt` section for the shipped pentanomial's
own math and the specific claim (within-pair correlation, not just draw-awareness) it's judged on.

**What would change this:** a concrete report that logistic-Elo pentanomial bounds behave
noticeably differently across draw rates/opening books in practice, which is exactly the problem
normalized Elo exists to solve.

### Power analysis / required-sample-size subcommand

**What it is:** given a desired effect size and confidence level, compute how many trials are
needed *before* running an experiment (the inverse of what `estimated_additional_trials` already
does reactively after an inconclusive result).

**Now covered, for `matrix`/tournament comparisons, plain `compare` runs, and `sprt`.** `veridict
plan` (see `docs/metrics.md`'s `plan` section) does this for the `matrix`/tournament-comparison
case - given `--min-elo`, it estimates additional trials needed per pair, ranked most-uncertain
first. `veridict power` (see `docs/metrics.md`'s `power` section) does the equivalent for a plain
two-way `compare --metric winrate/sign-test/elo` run: given `--min-effect` (the pass bar) and
`--assume-effect` (the true effect being powered for - must exceed `--min-effect`, since power
evaluated with the two equal is undefined-in-practice, see that section), it estimates trials
needed for a target power, computed exactly against the real Wilson/Clopper-Pearson/Jeffreys CI
functions this project already ships, not a textbook approximation. `veridict power --sprt` (see
`docs/metrics.md`'s `power --sprt` section) covers `sprt` itself: given `--elo0`/`--elo1`/
`--alpha`/`--beta` (the same inputs `sprt` takes), it reports the *expected* number of trials to a
decision under each hypothesis via Wald's classical Average Sample Number (ASN) approximation -
`E[N|H] ≈ [alpha'(H)*ln(A) + (1-alpha'(H))*ln(B)] / E[Z|H]`, where `alpha'(H)` is the probability
of stopping at the *upper* boundary `ln(A) = ln((1-beta)/alpha)` under hypothesis `H` - source:
Wald (1947), *Sequential Analysis*; this pairing was backwards in an earlier draft of this
project's own proposal, which would have produced a negative expected sample size under H1,
corrected before implementation. Structurally different from the CI-crossing-probability mode (no
`--target-power` - alpha/beta already fix the guaranteed error rates), so it's a separate `--sprt`
flag on the same subcommand rather than a `--metric` value. Wald's ASN is a known approximation
(ignores "overshoot" - the LLR's real excess past a boundary at the moment of stopping); the real
bias is measured empirically, not just cited, in `tests/calibration/sprt_asn_calibration.rs`
(about 1-2% at elo0=0/elo1=20/alpha=beta=0.05 - small at this gap, not claimed universal).

**`compare --metric mean-diff`'s equivalent has since shipped too**, via `--assume-sd`/`--pilot
FILE` (see `docs/metrics.md`'s `power --metric mean-diff` section) - a closed-form normal
approximation given an assumed or pilot-estimated standard deviation of the paired difference,
not a search (mean-diff has no closed-form CI-width-at-n function to search against, unlike
winrate/sign-test/elo). Still separate and deferred: budget-constrained allocation across a fixed
trial budget for `plan` (see the entry below), which neither `plan` nor `power` attempts.
Multiple-comparison correction for multi-metric `compare` runs (see that entry below) is a related
but distinct concern from power analysis - it's since shipped for `compare`, though `matrix`/`sprt`
versions of it haven't.

### Draw-aware `power --metric elo`

**What it is:** `power`'s exact search models every trial as a pure win/not-win `Binomial(n, p1)`
draw. For `winrate`/`sign-test` this exactly matches `compare`'s own math (both metrics discard
draws before computing their CI). For `elo`, it doesn't: `compare --metric elo` computes its score
over *all* trials including draws (`(candidate_wins + 0.5*draws) / (wins+losses+draws)`, draws as
half a win, in the denominator), so `power --metric elo`'s `estimated_trials` undercounts the real
game count needed whenever the true draw rate is nonzero - documented as a known caveat in
`docs/metrics.md`'s `power` section (treat it as a lower bound for draw-heavy candidates), not
silently wrong, but not modeled either.

**Why not yet:** modeling this properly needs a trinomial (win/draw/loss) sampling distribution
under an assumed draw rate, not just an assumed effect size - a new required input
(`--assume-draw-rate` or similar) with no natural default, and a genuinely different (three-outcome)
exact summation than the binomial one `power` ships with. Scoped out of this round the same way
`mean-diff`/`sprt` support was: a real, structurally different piece of work, not a quick addition.

**What would change this:** a concrete report that `power --metric elo`'s estimate is misleading
enough in practice (e.g. a draw-heavy shogi/chess engine test needing meaningfully more real games
than the tool suggested) to justify the added `--assume-draw-rate` input and trinomial search.

### Budget-constrained match allocation for `plan`

**What it is:** given a fixed total trial budget (`--budget N`) and/or an explicit goal
(`--goal identify-best`), recommend the *optimal set* of matches to run - a genuine constrained-
allocation/optimization problem, not just ranking every pair independently by uncertainty (what
`plan` ships today).

**Why not yet:** no real algorithm for either exists anywhere in this codebase or its dependencies.
An earlier, broader design for `plan` included both flags; both were dropped before implementation
specifically because building a speculative optimizer with no concrete request behind it is exactly
the over-engineering this project avoids (see `AGENTS.md`). `plan`'s shipped `--min-elo`-ranked list
already satisfies the motivating "what should I test next" use case without one.

**What would change this:** a concrete workflow where ranking pairs independently (today's `plan`)
demonstrably produces worse recommendations than a real joint allocator would, with enough detail to
shape what "optimal" should mean here (fewest total trials to a target confidence across all pairs?
fastest to identify the single best candidate? something else?).

### Multiple-comparison correction for multi-metric runs

**What it is:** when a `compare` run requests several `--metric` flags together, each gets its own
independent verdict at the stated confidence level - running several tests without correction
(e.g. Bonferroni/Holm) inflates the false-positive rate across that *family of individual
per-metric verdicts*, read as simultaneous claims. This is deliberately not the same target as
`verdict::aggregate`'s combined result: aggregating with "every metric must pass" is itself an
intersection-union rule over `alpha/2`-level tests, which is already no more likely to false-pass
than a single metric, with no correction needed (Berger 1982) - correction exists for whatever, if
anything, reads an individual metric's own verdict on its own.

**Now covered, with the target/mechanism split fully reflected in the report shape.**
`--claim-correction bonferroni`/`holm` (`--correction` kept as a deprecated alias for one release;
see `docs/metrics.md`'s `--claim-correction` section) keeps the family's one-sided false-pass rate
at or below what a single, uncorrected metric already has today (`alpha/2` at the default 95%
confidence). Both share one `achieved_alpha` binary search (against the same real
Wilson/Clopper-Pearson/Jeffreys CI functions `compare` already uses) via the standard CI-test
duality; Bonferroni applies a uniform `alpha/family_size` budget, Holm step-down sorts by achieved
significance and stops rejecting at the first failure (Holm 1979). Crucially, correction never
mutates `Report.verdict`/`promotion` or `MultiReport.verdict`/`promotion` (the deployment gate,
still computed from unadjusted per-metric verdicts via `verdict::aggregate`, unchanged) - it
populates a separate `Report.family_adjusted_verdict`/`family_adjusted_promotion` and
`MultiReport.simultaneous_claims_promotion` (`promoted` only if every report's
`family_adjusted_promotion` is too). Processing order is a real correctness dependency:
`verdict::apply_failure_caps`/`apply_failure_caps_to_multi` run *before*
`correction::apply_correction`/`apply_correction_to_multi`, so a report already forced to
`Inconclusive` for a technical failure never counts as a legitimate claim. `mean-diff`/
`quantile-diff`/`relative-diff` and `--cluster-by-id` are rejected outright as a configuration error
(exit code 3) rather than silently left uncorrected while still counting toward `family_size`
(`metrics::lacks_closed_form_ci` - none of the three bootstrap-CI metrics has a closed-form CI to
correct against; a `--cluster-by-id` report's cluster bootstrap CI can't be reconstructed from
`successes`/`paired_count` either, and a naive i.i.d. fallback would read as more significant than
it truly is under positive intra-cluster correlation - leniency in exactly the direction this
project's bias forbids).

**Still not yet:** `matrix`'s all-pairs correction (see the "matrix verdict semantics" entry below
- it needs its own verdict concept designed first, not a mechanical extension of what shipped for
`compare`) and `sprt`'s own multiplicity question (running several simultaneous SPRTs - a separate,
harder, unstarted question). Also still deferred: Benjamini-Hochberg/FDR (a different, less
conservative family-error target than FWER), finer-grained correction "families" (e.g. correcting
across candidates in a broader campaign, not just across metrics within one `compare` run),
sequential/repeated-looks warnings (checking an accumulating result multiple times before it's
final is itself a multiplicity risk this round doesn't address), and a cluster-aware/
bootstrap-aware `achieved_alpha` (which would let `mean-diff`/`quantile-diff`/`relative-diff`/
`--cluster-by-id` join a `--claim-correction` family instead of being rejected outright).

### Matrix verdict semantics

**What it is:** `matrix` (and `plan`, which shares its input) is entirely report-only today - no
`Verdict`/`Thresholds`/`decide()` concept exists anywhere in the module (`Command::Matrix`'s own
doc says "no single pass/fail verdict applies to a whole matrix"). Extending
multiple-comparison correction to matrix's all-pairs case - the natural next step after
`compare`'s multi-metric correction above - needs a real verdict concept for matrix designed from
scratch first, not a mechanical reuse of `compare`'s. Open questions, not yet answered:

- Does matrix gain a per-cell pass/fail verdict, a whole-matrix verdict, or does it stay
  report-only forever (with correction, if it ever exists, applied only to some derived summary
  rather than to individual cells)?
- If cells get a pass bar, what does it look like - a `--min-elo`-style symmetric threshold per
  pair, or an asymmetric `--pass-above`/`--fail-below` pair like `compare`'s?
- Every pairwise comparison has a direction (row beats column, or vice versa) - does a per-cell
  verdict need to be direction-aware in a way `compare`'s single candidate-vs-baseline verdict
  never had to be?
- Does the "all-pairs" correction family mean every cell in the matrix, every cell touching a
  given candidate, or something else - and does that choice interact with `plan`'s own
  most-uncertain-first ranking?
- How do disconnected or fragile cells (see `matrix`'s general-graph mode and Bradley-Terry
  convergence handling) participate in a family-wise correction - excluded, included with a
  wider default uncertainty, or something else?
- Should correction here reuse `compare`'s `achieved_alpha`/Bonferroni/Holm machinery as-is, or
  does matrix's fundamentally different shape (an `n`-by-`n` grid of comparisons instead of one
  candidate vs. one baseline) call for a different construction entirely?

**Why not yet:** a genuinely separate design question from `compare`'s multi-metric correction,
not a follow-on implementation task - resolving the above needs a real design pass (a design memo
or a dedicated `docs/matrix-verdict-design.md`), not a `/plan` invocation that assumes the answers.

**What would change this:** someone doing that design pass, or a concrete workflow that makes one
of the above questions no longer open (e.g. a real request specifically for a per-cell pass bar,
which would settle the first two questions at once).

### `--failure-policy` for `matrix`

**What it is:** `compare`/`sprt` support `--failure-policy report-only`/`exclude`/`loss` (see
`docs/metrics.md`); `matrix` doesn't - every candidate's failures are still tallied and reported
per-candidate, but always behave as `report-only`.

**Why not yet:** `matrix` has no verdict for a failure policy to influence (it's report-only by
design - see the README's "Comparison matrix" section), unlike `compare`/`sprt` where `loss`
changes which side of a threshold/LLR boundary a run lands on. No concrete workflow has asked for
`matrix`'s per-candidate Elo estimates themselves to treat a failure as a loss yet.

**What would change this:** a concrete request to have a candidate's crash/timeout rate pull its
own Elo estimate down in a matrix run, not just get reported alongside it.

### `--cluster-by-id` for `mean-diff`/`sign-test`/`quantile-diff`/`relative-diff`

**What it is:** `--cluster-by-id` (see `docs/metrics.md`) ships for `winrate`/`elo` - a cluster
bootstrap CI over id-grouped outcome tallies, correctly widening the interval when many trials
share a common source of correlation (the same opening/testcase replayed several times).
`mean-diff`/`sign-test`/`quantile-diff`/`relative-diff` don't get it yet.

**Why not yet:** `winrate`/`elo` both collect through `OutcomeCollector`, which already tracks
per-id groups for `--paired-by-id`'s netting - extending that same grouping to feed a cluster
bootstrap instead was a natural reuse. `mean-diff`/`sign-test`/`quantile-diff`/`relative-diff`
collect through `DiffCollector`/`SignCounts` instead, which discard per-id structure once a value
is netted or tallied; giving them real cluster support needs those collectors to retain cluster
structure through to resampling (which `mean-diff`/`quantile-diff`/`relative-diff` already do for
their own *non*-clustered bootstrap, so it's a plausible extension, not a new mechanism) - separate
wiring per collector, not a mechanical copy of `OutcomeCollector`'s change.

**What would change this:** a concrete workflow needing a cluster-robust CI for paired numeric
data (e.g. per-request latencies clustered by which endpoint/deployment produced them) - most
directly implementable for `mean-diff`/`quantile-diff`/`relative-diff` (already bootstrap-based)
before `sign-test` (closed-form Wilson on a sign proportion, same harder-conversion shape
`winrate`/`elo` themselves needed).

### Checkpoint/training lineage tracking

**What it is:** an idea raised alongside `--cluster-by-id`/the validity axis (a broader proposal
for linking `veridict`'s verdicts to upstream training/checkpoint provenance - candidate parent,
training recipe, dataset hash, selector rule, checkpoint-selection reason, weight/binary hash -
and rendering a lineage tree from failed candidates back to their source).

**Update (2026-07-26): the opaque-passthrough half of this shipped, as `verify-run`.** A concrete
downstream pipeline (multiple tools around `veridict`, coordinating a shared "experiment
envelope" of opaque identifiers - `experiment_id`/`candidate_id`/`baseline_id`/`lineage_id`/
`dataset_sha256`/`binary_sha256`/`weight_sha256`/seeds/`schema_version`/`validity`) needed exactly
what the "What would change this" paragraph below described: a way for `veridict` to carry those
identifiers through untouched and check them for consistency, without understanding what any of
them mean. `veridict verify-run manifest.toml games.jsonl` is that - see the README's "Verify
run" section and `schemas/experiment-envelope.schema.json`. It only ever compares caller-supplied
opaque strings for equality/no-drift between a declared `manifest.toml` and the actual
`games.jsonl`; it never opens, hashes, or interprets an actual binary/weight/corpus file, and it
does not store or track lineage across runs itself.

**What's still out of scope, unchanged:** lineage *storage* across many runs (an
experiment-database concern) and a lineage *tree* display (a dashboard concern) remain explicitly
one-way-dependency integrations `veridict` itself must not become - `verify-run` checks one run's
internal consistency, it doesn't track history across runs. Original reasoning kept below for
context.

**Why not shipped (as a lineage-tracking *system*, still true):** this is squarely the
"Deliberately out of scope" list below, not a "not yet" item - lineage storage/tracking is an
experiment-database concern, and a lineage *tree* display is a dashboard concern; both are
explicitly one-way-dependency integrations `veridict` itself must not become. `veridict` judges
results; it doesn't track how they were produced.

**What would change this (historical - already acted on above):** a concrete downstream tool that
already tracks checkpoints/experiments and needs `veridict`'s JSON report to carry an opaque
identifier through untouched for that tool to join on.

### Trinomial/pentanomial policies for `time_sensitive`

**What it is:** `time_sensitive` (see the README's "Time-sensitive testing" section) ships v1
scoped to Bernoulli simple-vs-simple only - a fixed null `p0`/alternative `p1`, draws advance the
reward-schedule clock but carry no information (see `time_sensitive`'s module doc). Extending the
same time-sensitive-reward idea to `sprt`'s trinomial (draw-rate-aware) or pentanomial
(paired-game) models - the actual shape of Sekirei's chess/shogi draw-heavy data - is a real,
separate piece of work.

**Why not yet:** the Bellman/EDO math in this round is derived specifically for a two-outcome
(success/failure) transition weighted by `p1` (see `time_sensitive::grid`'s recursion and
`time_sensitive::edo`'s moment equation). A trinomial/pentanomial version needs a draw-rate
nuisance parameter folded into that same transition, which changes the state space, the moment
equation EDO solves, and the exact-enumeration/martingale-identity proof this round's tests rely
on - not a mechanical copy of `stats::trinomial_sprt`/`stats::pentanomial_sprt`'s own
draw-modeling onto the new grid.

**What would change this:** a concrete request to apply time-sensitive rewards to draw-heavy
data (the Sekirei pipeline is the motivating case), once the Bernoulli version has had real usage
to validate the reward-schedule/policy design choices against.

### Composite hypotheses and online `p1` estimation for `time_sensitive`

**What it is:** `time_sensitive` takes `p0`/`p1` as fixed, caller-supplied constants (simple-vs-
simple). A composite alternative (a range or prior over `p1` rather than one fixed value) or
online estimation of the true alternative from the data being collected are both real extensions
the underlying paper's framework doesn't rule out.

**Why not yet:** both change what "the alternative" means to the Bellman/EDO recursion
mid-stream, which interacts with the anytime-validity argument in a way this round's tests don't
cover (`time_sensitive`'s validity proof holds for *any* action sequence chosen independently of
knowledge of the true `p0`/`p1` under test - updating the alternative from the same data the
wealth process is betting on needs its own, separate justification, not an assumed extension of
the fixed-`p1` case). No concrete design exists yet.

**What would change this:** a concrete workflow where a fixed `p1` guess is known to be
unreliable enough that adapting it mid-run would meaningfully change outcomes, with enough detail
to shape whether a composite-prior or an online-estimation approach fits better.

### `--elo0`/`--elo1` input for `time-sensitive`

**What it is:** `sprt --elo0`/`--elo1` (logistic Elo, via `stats::sprt::score_from_elo`) is a
natural-looking input `time-sensitive --p0`/`--p1` could plausibly also accept.

**Why not yet:** `score_from_elo` returns an Elo-implied *expected score*, where a draw counts as
half a win - not the decisive-observation-conditional success probability `time_sensitive`'s
`p0`/`p1` are defined as (see `time_sensitive::e_variable::BernoulliHypotheses`'s doc). Wiring
`--elo0`/`--elo1` straight through `score_from_elo` would silently mix those two definitions on
any input with a nonzero draw rate. A correct version needs a draw-rate input to convert between
the two, which doesn't exist yet - the same real blocker "Draw-aware `power --metric elo`" above
describes for a different subcommand.

**What would change this:** a concrete request for Elo-scale input to `time-sensitive`, paired
with a decision on where the draw-rate parameter that conversion needs comes from (a new
`--assume-draw-rate`-style flag, most plausibly, mirroring the `power --metric elo` entry above).

### Boundary-aware capped EDO, matrix/multi-metric support, and other `time_sensitive` refinements

**What it is:** several smaller, independent extensions to `time_sensitive`, none started: a
boundary-aware correction to EDO's stationary approximation near the wealth threshold (the paper's
own EDO derivation is a stationary/interior approximation - see `time_sensitive::edo`'s module
doc - and doesn't correct for the boundary effect close to `1/alpha`); comparing several
candidates against one baseline (`matrix`'s shape) under a time-sensitive reward; running several
simultaneous time-sensitive claims with multiple-comparison correction (the same open question
`sprt`'s own multiplicity has, see "Multiple-comparison correction for multi-metric runs" above);
and reinforcement-learning-based policies as an alternative to the grid/closed-form approximations
this round ships.

**Why not yet:** none has a concrete workflow asking for it yet, and each is a genuinely separate
design question (a boundary correction changes EDO's own derivation; matrix support needs
`time_sensitive`'s per-candidate shape designed the way "Matrix verdict semantics" above is still
open for `compare`; multi-claim correction needs the same family-wise design work as `sprt`'s).
Building any of them speculatively, with no concrete request behind it, is exactly the
over-engineering this project avoids (see `AGENTS.md`).

**What would change this:** a concrete workflow for each - most plausibly, real usage of the
Bernoulli v1 surfacing which of these actually matters in practice, rather than guessing up front.

### Sekirei preset schedules (400/3200-game gates) for `time_sensitive`

**What it is:** the Sekirei pipeline's own gate design uses specific game-count checkpoints (e.g.
400/1600/3200) as reward-schedule breakpoints - `time_sensitive`'s `--reward-schedule` JSON format
(see the README) already expresses exactly this shape.

**Why not yet, and why probably never as a core feature:** this is caller-side configuration, not
a core-library concern - `veridict` stays domain-agnostic (see `AGENTS.md`'s "no game/engine-
specific presets baked into core APIs" rule, the same reason `verify-run` never hashes an actual
binary/weight/corpus file). A Sekirei-specific preset belongs in a Sekirei-side config file or
wrapper script that calls `veridict time-sensitive --reward-schedule sekirei-gate.json`, not in
`veridict` itself.

**What would change this:** nothing changes this by design, short of a genuinely domain-agnostic
generalization of "a common shape of tiered reward schedule" that several unrelated callers would
plausibly want - unlikely, since `--reward-schedule` already covers the general case.

## Deliberately out of scope

veridict judges results; it does not produce them. These are not "not yet" items - they're
intentionally left to separate projects that depend on veridict, keeping the dependency direction
one-way (integrations depend on veridict, veridict never depends on them):

- Web dashboard
- Experiment database
- Cloud service
- Distributed workers
- Domain-specific match runners (e.g. running a chess engine or an LLM to produce results)
- LLM-judge implementation
- OCR metric implementation
- Chess/shogi protocol support
- Plotting-heavy UI

If your workflow needs one of these, build it as a separate tool that feeds JSONL/CSV into
veridict - that's the intended integration point, not a missing feature.
