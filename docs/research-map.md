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
(e.g. Bonferroni/Holm) inflates the overall false-positive rate across the combined result.

**Now covered, for `compare`'s multi-metric family.** `--correction bonferroni`/`holm` (see
`docs/metrics.md`'s `--correction` section) keeps the family's one-sided false-pass rate at or
below what a single, uncorrected metric already has today (`alpha/2` at the default 95%
confidence) - not the nominal `alpha` itself, which would let a "corrected" family tolerate a
*higher* false-pass rate than a single uncorrected metric already does. Both share one
`achieved_alpha` binary search (against the same real Wilson/Clopper-Pearson/Jeffreys CI functions
`compare` already uses) via the standard CI-test duality; Bonferroni applies a uniform
`alpha/family_size` budget, Holm step-down sorts by achieved significance and stops rejecting at
the first failure (Holm 1979). Correction only ever downgrades an unadjusted `pass` to
`inconclusive` (widening a CI can't newly satisfy the `fail` condition for a report that already
passed) - `verdict::aggregate`'s "any fail sinks the whole run, else any inconclusive, else pass"
logic itself is unchanged; correction only ever changes *which* individual verdicts feed into it,
never how they combine. `mean-diff` can't be individually corrected (no closed-form CI at a
hypothetical confidence for a bootstrap interval) but still counts toward `family_size`.

**Still not yet:** `matrix`'s all-pairs correction (see the new "matrix verdict semantics" entry
below - it needs its own verdict concept designed first, not a mechanical extension of what
shipped for `compare`) and `sprt`'s own multiplicity question (running several simultaneous SPRTs
- a separate, harder, unstarted question). Also still deferred: Benjamini-Hochberg/FDR (a
different, less conservative family-error target than FWER), finer-grained correction "families"
(e.g. correcting across candidates in a broader campaign, not just across metrics within one
`compare` run), and sequential/repeated-looks warnings (checking an accumulating result multiple
times before it's final is itself a multiplicity risk this round doesn't address).

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
