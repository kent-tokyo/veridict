# Metrics reference

English | [日本語](metrics_ja.md)

This is the unabridged version of the README's "Statistical basis" section: for each number
veridict reports, the method behind it, its assumptions, its known failure modes, and whether it's
an established statistic or a project-specific design choice. See `docs/research-map.md` for
methods considered but not shipped, and what's deliberately out of scope.

Full bibliographic entries for every citation below are in the README's "Statistical basis" →
References section (not duplicated here).

## `winrate` / `sign-test` confidence interval

**Established statistic**, three interchangeable methods (`--ci-method`):

- **`wilson` (default)** - Wilson score interval (Wilson 1927). `winrate` computes it over
  decisive (non-draw) trials only: `p_hat = candidate_wins / (candidate_wins + baseline_wins)`;
  draws count toward "used" records but are excluded from the proportion, matching standard
  paired-match testing practice. `sign-test` runs the identical interval over the proportion of
  paired records where the candidate beat the baseline (ties excluded). Both are then centered on
  0 (deviation from a 50/50 split) so they compose directly with `--min-effect`/`--pass-above`/
  `--fail-below`.
- **`exact`** - Clopper-Pearson exact binomial interval (Clopper & Pearson 1934), derived directly
  from the Binomial-Beta relationship. Holds its nominal coverage guarantee exactly at any sample
  size, at the cost of usually being wider than Wilson's. Only defined for a true integer-count
  binomial - rejected for `elo`/`mean-diff` (`VeridictError::IncompatibleCiMethod`), not silently
  approximated.
- **`jeffreys`** - Bayesian credible interval using the non-informative Jeffreys prior
  Beta(0.5, 0.5) (Jeffreys 1946; the boundary-clamping convention used here, and the general
  "Interval Estimation for a Binomial Proportion" comparison of the three methods, is from Brown,
  Cai & DasGupta (2001)). Same integer-count restriction as `exact`, for the same reason.

**Failure mode / ordering to know:** for interior `p` the textbook ordering holds - Wilson
tightest, Jeffreys in the middle, Clopper-Pearson widest - but this is *not* universal. At the
boundary (all-wins or all-losses), Jeffreys' prior contributes real probability mass near the edge
that neither Wilson's normal approximation nor Clopper-Pearson's worst-case guarantee gets to use,
so Jeffreys can end up narrower than *both* there. Don't assume Clopper-Pearson is always the
widest just because it's the "exact" one.

## `mean-diff` bootstrap confidence interval

**Established statistic**, three methods (`--bootstrap-method`), all from Efron & Tibshirani,
*An Introduction to the Bootstrap* (1993, ch. 14):

- **`percentile` (default)** - the original bootstrap CI: resample `candidate - baseline` pairs
  with replacement, take the `alpha/2`/`1 - alpha/2` percentiles of the resampled means.
- **`basic`** (a.k.a. reflected/reverse-percentile) - reflects the percentile interval around the
  original sample's point estimate: `(2*effect - perc_hi, 2*effect - perc_lo)`. No bias-correction
  of its own. **Known limitation:** on skewed data it reflects the *same* skew that made the
  percentile interval biased in the first place, so it can move the bounds in the opposite
  qualitative direction from BCa's correction - it's the "obvious" fix for percentile's bias
  problem, but a naive one.
- **`bca`** - bias-corrected and accelerated. Adjusts *which* percentiles are read using a
  bias-correction `z0` (fraction of bootstrap resamples below the original estimate, converted via
  the inverse normal CDF) and an acceleration `a` (from a jackknife over the original sample,
  O(n)). When `z0 = 0` and `a = 0` this reduces exactly to the plain percentile bounds - useful to
  know when checking whether a BCa result looks wrong.

`--resamples` controls the resample count; `--seed` controls the RNG seed (fixed by default, so
output is bit-identical across runs of the same input - every resample gets its own independently
seeded RNG stream, verified invariant to worker/thread count).

## `elo`

**Established statistic with one documented approximation.** Score rate
`score = (candidate_wins + 0.5 * draws) / n`, converted via the standard logistic Elo model
(the widely-used variant of Elo's original rating system, Elo 1978). The confidence interval reuses
Wilson's score interval on that score rate, treating each trial as a plain Bernoulli draw.

**Known, deliberate approximation:** this overstates variance relative to the true trinomial
(win/draw/loss) distribution, since a draw's half-point outcome carries less variance than a coin
flip would. This is a deliberately conservative (too-wide, never too-narrow) choice - consistent
with the project's "false pass is worse than inconclusive" bias - and the same tradeoff already
accepted for `sign-test`. Doesn't support `--ci-method exact`/`jeffreys`: both require a true
integer-count binomial, and Elo's win rate is fractional (a draw is half a win).

**A separate source of noise, and how to cancel it:** the approximation above is about *modeling*
variance correctly once the trials are fixed. It says nothing about *testcase-selection* variance
- independently sampling which starting position or which side plays which role adds noise that
has nothing to do with the candidate's actual strength. Engine-style paired testing (same starting
position played twice, roles/sides swapped) is the standard way to cancel that bias, and `elo`
already supports it for free via [`--paired-by-id`](../README.md#paired-testcases): assign the
same `id` to both games of a position-pair (independent of which side won which color) so the pair
nets to one observation - by total points, `win=1`/`draw=0.5`/`loss=0` - instead of two independent
ones. This is a different lever from `--ci-method`/`--bootstrap-method`: it reduces the variance
that goes *into* the estimate, rather than changing how the interval is computed from it.

## `sprt`

**Established statistic**, three variants (`--sprt-variant`):

- **`wald` (default)** - the classic two-outcome sequential probability ratio test (Wald 1945).
  Accumulates a log-likelihood ratio over *decisive* (non-draw) trials only and stops as soon as it
  crosses one of two boundaries derived from `--alpha`/`--beta` (the test's actual guaranteed false
  positive/negative rates, not tunable report thresholds). Draws carry no information about which
  Elo hypothesis is true under this model and are excluded from the LLR entirely - matching the
  same "decisive games only" convention `winrate`/`sign-test` already use.
- **`trinomial`** - a draw-aware generalized LLR test in the BayesElo parameterization historically
  used by chess-engine testing tools (Fishtest's `LLRlegacy`). Estimates the draw rate as a
  nuisance parameter (`drawelo`) from the pooled win/draw/loss counts, then evaluates both
  hypotheses at that shared estimate - this is what lets it converge faster than the plain Wald
  test on draw-heavy data, without the caller needing to supply a separate draw-rate assumption.
  **Units are BayesElo, not logistic Elo** - the two scales only coincide when `drawelo == 0`
  (concretely: at `drawelo = 200`, a BayesElo gap of 10 corresponds to a logistic-Elo gap of only
  about 7.3) - which is why the CLI exposes this through separate `--belo0`/`--belo1` flags rather
  than reinterpreting `--elo0`/`--elo1`. At zero draws this reduces exactly to the Wald variant's
  LLR (the algebra collapses exactly, not just in a limit).
- **`pentanomial`** - a generalized LLR test over *paired* games (same opening, colors swapped),
  ported from Fishtest's `LLR_logistic` (expectation-constrained multinomial MLE / "exponential
  tilting" of the empirical pair-outcome distribution to a hypothesized mean score). **Always
  requires `--paired-by-id`**: two records sharing an `id` are combined into one of 5 outcome
  categories by their pair's combined candidate score (`0`/`0.5`/`1`/`1.5`/`2` - candidate points
  summed over the pair's two games), instead of netted down to a single win/loss/draw the way
  `--paired-by-id` works for `winrate`/`elo`. An id that doesn't appear exactly twice is a hard
  error, not silently treated as an unpaired sample - a 5-value pair score has no meaning for a
  lone game. Hypotheses are `--elo0`/`--elo1`, the same logistic Elo scale as `wald` - this model
  has no `drawelo`-style nuisance parameter to make BayesElo meaningful.

  **Why this isn't just `trinomial` on twice as many games:** a pentanomial pair's entire
  statistical value comes from the *negative correlation* between its two games. Concretely, model
  each pair as sharing a per-pair "opening bias" `b` that shifts one game's candidate-win
  probability up by `b` and the other's down by `b` (colors swapped, so whichever side gets the
  opening's favorable color benefits in that one game and is disadvantaged in the other): the
  pair's *combined* score has expectation `2p` regardless of `b` (the `+b`/`-b` terms cancel), so
  the bias contributes zero variance to the pair total - while each individual game's own
  *marginal* variance (averaged over the bias distribution) is inflated by the bias spread on top
  of ordinary sampling variance. Running `trinomial`/`wald` on the ungrouped games sees that
  inflated per-game variance directly; `pentanomial`'s per-pair scoring cancels it, which is what
  lets it converge in *fewer pairs* than `trinomial` needs *games* on real paired data - not merely
  "the same information, batched differently." An earlier design for this variant modeled a pair's
  outcome as the convolution of two *independent* single-game draws instead: that sets the
  covariance to exactly zero by construction and throws away this entire effect, which is why it
  was rejected in favor of the approach actually shipped (treating each pair's outcome as one draw
  from a free 5-category multinomial, whose *shape* is estimated from the empirical pair-outcome
  frequencies directly - preserving whatever real correlation the data has - with only the *mean*
  constrained per hypothesis).

  The report adds `sprt_variant` (present for every variant), and, only for `pentanomial`:
  `pentanomial_counts` (the 5-bucket breakdown the LLR was computed from), `raw_trial_count`
  (total input records before pairing), and `paired_count` (number of complete pairs). The
  existing `candidate_wins`/`baseline_wins`/`draws` fields stay populated too, netted from the same
  5 buckets by the standard "paired game" convention (`>1` net candidate win, `<1` net baseline
  win, exactly `1` net draw) for compatibility with tooling that only understands the 3-outcome
  shape.

  **Not (yet) implemented:** Fishtest's newer *normalized-Elo* pentanomial (`LLR_normalized`,
  a `t`-value-constrained MLE with its own iterative solve) and the Siegmund discrete-time bound
  correction some engine-testing tools apply on top of the base LLR - both real refinements, not
  needed to get a statistically valid pentanomial test, deferred rather than rejected (see
  `docs/research-map.md`).

Neither `wald` nor `trinomial` is a heuristic, and `pentanomial` isn't either - all three are the
referenced sequential test, evaluated exactly (mod the numerical `secular`-equation solve
`pentanomial` needs, itself a standard empirical-likelihood tilting, not an approximation) against
the observed counts. What *is* a design choice is which variant runs by default (`wald`, since it
needs no nuisance-parameter estimation).

## `--failure-policy`

**This project's own design choice, not a citation-backed method** - it controls whether a failed
trial (`baseline_status`/`candidate_status` other than `ok`) affects the *computation*, not just
whether it's *reported* (`failure_breakdown` tallies every failure regardless of this flag). Only
meaningful for outcome-based metrics (`winrate`/`elo`) and `sprt`; `mean-diff`/`sign-test` have no
win/loss/draw outcome for a failed numeric trial to become, so requesting `exclude`/`loss` with
either is a config error (`IncompatibleFailurePolicy`), not an arbitrary numeric penalty invented
for the occasion.

- **`report-only`** (default) - exactly the behavior that existed before this flag did: a status-
  only record (no `result`) already contributes nothing to any outcome-based metric, so this is a
  no-op in the common case. The one place it's *not* a no-op: a record carrying both a failure
  status and a `result` still has that `result` counted.
- **`exclude`** - a failed side's `result` is never counted, even when present alongside the
  status. This is where it actually diverges from `report-only`, and only in that same mixed-
  field case - worth calling out explicitly so it isn't mistaken for a broader behavior change on
  ordinary data.
- **`loss`** - the failed side's outcome is synthesized: candidate failed -> `baseline_win`,
  baseline failed -> `candidate_win`, both sides failed -> `draw` (a symmetric "wash", not an
  arbitrary tie-break). This synthesized outcome *overrides* any literal `result` on the same
  record - trusting the execution-level failure signal over a same-record `result` is the
  conservative choice, consistent with this project's "a false pass is worse than an inconclusive
  result" bias (a `candidate_win` result next to a `candidate_status: crash` must never silently
  win out over the crash).

Applies identically under `--paired-by-id`: the resolved outcome (literal or synthesized) is what
gets fed into the pairing/netting logic, so a `loss`-synthesized failure inside a pair nets
against its partner exactly the way any other outcome would - `--sprt-variant pentanomial`
included, where a failure's synthesized outcome becomes one of the pair's two games going into the
5-value bucket.

## `matrix`'s general-graph mode

**Established statistic.** Once real candidate-vs-candidate games make the observed graph
topologically more than a star (not every game touches the shared baseline), `matrix` fits a
general Bradley-Terry paired-comparison model (Bradley & Terry 1952) via the Zermelo (1929)/
Hunter (2004) Minorization-Maximization fixed-point iteration, computed independently per strongly
connected component.

**Existence condition (not a numerical nicety - a real mathematical requirement):** a finite
maximum-likelihood solution requires the "who scored any points against whom" graph to be strongly
connected (Ford 1957). If some nonempty proper subset of competitors never lost or drew against
anyone outside it, that subset's ratings diverge to infinity relative to the rest - there is no
finite MLE for that pair, not merely an uncertain one. `matrix` computes strongly connected
components first and fits each independently, rather than inferring disconnection from the solver
failing to converge - a genuinely divergent component's relative change per iteration can decay
like `1/n` (shrinking every step, never crossing a fixed threshold), so no choice of iteration cap
reliably tells "slowly converging" apart from "slowly diverging" by iteration behavior alone.

**Confidence intervals** on general-graph pairwise Elo differences support the same
`--bootstrap-method percentile|basic|bca` as `mean-diff`. BCa's acceleration term needs a
jackknife, but the underlying data is aggregate per-edge win/loss/draw tallies, not raw per-game
records, so a literal "leave one game out" isn't directly possible. Games within one edge's outcome
category are exchangeable, so the true per-game jackknife collapses to at most 3 distinct weighted
replicates per edge (drop-one-a-win, drop-one-b-win, drop-one-draw) rather than a naive (and
wrong-magnitude) leave-one-*edge*-out. On star-graph topology, `matrix` instead uses the closed
form directly (no iterative solver, no bootstrap needed - a star graph's Bradley-Terry MLE has no
shared terms to solve jointly).

## `plan`

**This project's own design choice, layered on already-established statistics** - `veridict plan`
takes `matrix`'s exact input and output (see above) and, for each cell, estimates how many
additional trials would narrow that pair's Elo-difference CI to `--min-elo` half-width. It
deliberately does not reuse `verdict::estimate_additional_trials` directly: that function is keyed
on a verdict/threshold crossing (`Inconclusive` + `Thresholds`), which has no equivalent here -
`plan` has no verdict at all, just a target half-width to narrow toward. Its own small function
mirrors that function's two-branch shape instead:

- **Exact Wilson-CI binary search** - for a `baseline`-vs-one-candidate cell (the common star-graph
  case), the cell's CI *is* that one candidate's own Wilson CI (see `matrix`'s star-graph closed
  form above), so the same real, already-tested `wilson_ci_from_proportion` function is
  recomputed at a hypothetical `n`, holding the point estimate fixed, and binary-searched to the
  target half-width - not an approximation, real math against the same formula the report shows.
- **`O(1/sqrt(n))` CLT-scaling fallback** - for every other cell shape: a star-graph
  candidate-vs-candidate cell (its CI is a `hypot` of two Wilson margins, not a single proportion
  with a closed form) and every general-graph cell (a real bootstrap CI has no CI-width-at-n
  function at all). This is the exact model `mean-diff` already uses for the same reason, with the
  same known bias documented under `estimated_additional_trials` below - `n` here is the bottleneck
  side's own trial count (`min` of the two competitors' `paired_count`), since narrowing a pair's CI
  is gated by whichever side has fewer trials.

A `disconnected` pair, or a `direct`/`inferred` cell too fragile under resampling for a reliable CI
(see `matrix`'s general-graph doc above), gets no estimate at all (`null`, with an explanatory
`note`) rather than a number computed from a CI that doesn't exist yet - narrowing a nonexistent CI
isn't a "how many more trials" question, it's a "you need a connecting game first" one.

**Deliberately dropped from an earlier, broader idea:** a `--budget N`/`--goal identify-best`
constrained-allocation solver (recommend an optimal *set* of matches under a fixed trial budget,
rather than ranking every pair independently). No real algorithm for that exists anywhere in this
codebase or its dependencies - see `docs/research-map.md` for what would need to exist before that
ships.

## `power`

**Established statistic (statistical power), computed exactly against this project's own real CI
functions rather than a textbook formula.** `veridict power` estimates how many trials
`compare --metric winrate/sign-test/elo` would need for a `--target-power` probability of reaching
a passing verdict *before running any of them* - the pre-experiment counterpart to
`estimated_additional_trials` below, which only ever answers the same question *after* some trials
already ran.

**Why `--min-effect` and `--assume-effect` are both required, and why they must differ.**
`compare`'s real decision rule (`verdict::decide`) is: pass iff a CI's lower bound clears
`pass_above`. If power were evaluated with the *true* effect set equal to that same pass bar, the
computed number would be the interval's own miscoverage at the boundary it was built against
(`≈ 1 - confidence`) - flat, and it never climbs toward a target power no matter how large `n`
gets, because a CI's lower bound crossing the *exact* true value it's centered near is fundamentally
a coverage-guarantee question, not a sample-size question. A real power calculation needs two
distinct values: `--min-effect` (the pass bar - identical meaning to `compare --min-effect`/
`--pass-above`) and a strictly larger `--assume-effect` (the true effect actually being powered
for). `power` rejects `--assume-effect <= --min-effect` as a hard error rather than silently
returning a number that looks meaningful but isn't - the standard distinction, present in every
real power analysis, between "the smallest effect worth caring about" and "the effect you actually
expect or hope for."

**The exact calculation:**

```
power(n) = sum_{k=0}^{n} Binomial_pmf(n, p1, k) * [CI_lower(k, n, confidence) >= p0]
```

`p0`/`p1` are `--min-effect`/`--assume-effect` converted to proportions (`0.5 + effect` for
`winrate`/`sign-test`; `stats::sprt::score_from_elo(effect)` for `elo` - the same named
logistic-Elo transform `sprt`'s own hypothesis handling uses, reused directly rather than
re-derived a third time). `CI_lower` is whichever of `wilson`/`exact`/`jeffreys` `--ci-method`
selects (`elo` accepts only `wilson`, the same restriction `compare --metric elo` itself has) -
real, already-tested functions, the same rationale `estimated_additional_trials` already gives for
searching against real CI math instead of an approximation. The smallest qualifying `n` is found
via a fast normal-approximation seed, refined by an exact search against the formula above - never
the normal approximation itself as the reported answer.

**`estimated_trials` counts decisive trials, and for `elo` specifically that's a real, undocumented-
until-now gap against draw-heavy testing.** The `Binomial(n, p1)` model above draws every trial as
either a "success" or not - there is no draw outcome in it. For `winrate`/`sign-test` this is
already exactly what `compare` itself does: both metrics discard draws before computing their own
CI (`winrate.rs`'s `finish` explicitly drops the draw count), so `n` there genuinely means decisive
trials, and `power`'s model matches `compare`'s own. For `elo`, `compare --metric elo` computes its
score as `(candidate_wins + 0.5 * draws) / (wins + losses + draws)` - draws count as half a win and
sit inside the denominator - which `power`'s pure win/not-win model does not represent. In a
draw-heavy testcase (the common case in engine testing - the whole reason `--sprt-variant
pentanomial`/`trinomial` exist), the real `compare --metric elo` run will need more total games than
`veridict power --metric elo`'s `estimated_trials` says, since some fraction of those games resolve
as draws rather than decisive results. Treat `power --metric elo`'s number as a lower bound on total
games for a draw-heavy candidate, not the real game count - a draw-aware `elo` power model is a
deferred extension (see `docs/research-map.md`), not something this round attempts.

**The "sawtooth" caveat.** Exact power for a discrete CI method (Wilson, and especially
Clopper-Pearson/Jeffreys) is not perfectly monotonic in `n` - a documented property of exact
discrete methods, not a bug (Chernick, M.R. & Liu, C.Y. (2002), "The Saw-Toothed Behavior of Power
Versus Sample Size and Software Solutions: Single Binomial Proportion Using Exact Methods," *The
American Statistician* 56(2):149-155). `estimated_trials` is confirmed to hold `>= target_power`
across a window of subsequent `n`, not just at the single point a naive search might land on -
`achieved_power` in the report is the real exact power at `estimated_trials`, and can overshoot
`target_power` by a nontrivial margin for exactly this reason. `tests/calibration/
power_calibration.rs` verifies this empirically via Monte Carlo simulation, not just by trusting
the derivation.

**Why `--paired-by-id` doesn't change the number.** Pairing reduces testcase/opening variance in
practice (see "Paired testcases" and the `pentanomial` SPRT section above), but the *actual*
reduction depends on the data's within-pair correlation, which doesn't exist yet before any trial
has run - there's nothing to measure it from. `power` accepts the flag and adds a caveat to the
report's `notes` rather than silently applying an invented correction factor: treat the reported
number as a conservative (unpaired) upper bound when pairing is planned.

**Why `mean-diff` isn't supported.** No closed-form CI-width-at-n function exists for a bootstrap
CI without real resampled data (the same reason `estimated_additional_trials` falls back to an
`O(1/sqrt(n))` approximation for it post-hoc) - but pre-experiment there is no fallback available
either, since there's no existing sample to approximate a variance from at all. Rejected at the CLI
flag-parsing level (a distinct `--metric` enum from `compare`'s own), not silently ignored.

**Why the output is a design estimate, not a guarantee.** `estimated_trials` assumes the true
effect is *exactly* `--assume-effect`. A smaller real effect needs more trials than this number
says, not fewer - not a corner case, the entire reason `--assume-effect` is a required, separate,
user-supplied assumption rather than something this tool infers from `--min-effect` alone.
Consistent with this project's "a false pass is worse than an inconclusive result" bias: `power` is
a design aid for choosing how much data to collect, never a substitute for the real confidence
interval `compare` computes from whatever data is actually observed.

### `power --sprt`

**A structurally different question, not a variant of the search above.** Wald's SPRT guarantees
its `alpha`/`beta` error rates by construction, regardless of `n` - there's no "target power" to
search a sample size for. What's useful instead is the *expected* number of trials to a decision
(Wald's own term: "Average Sample Number", ASN) under each hypothesis, given `--elo0`/`--elo1`/
`--alpha`/`--beta` - the same inputs `veridict sprt --sprt-variant wald` itself takes
(`SprtConfig::new` is reused directly for validation, so a bad `elo0 >= elo1` etc. here produces
the exact same error `sprt` itself would).

**The formula** (Wald's classical approximation - source: Wald (1947), *Sequential Analysis*):

```
E[N | H] ≈ [alpha'(H) * ln(A) + (1 - alpha'(H)) * ln(B)] / E[Z | H]
```

`ln(A)`/`ln(B)` are the same stopping boundaries `stats::sprt::bounds` computes for `sprt::run`'s
real Wald loop - reused directly, not re-derived. `alpha'(H)` is the probability of stopping at the
*upper* boundary under hypothesis `H`: `alpha` under H0, `1 - beta` under H1 (this pairing was
backwards in an earlier draft of this feature's own proposal - it would produce a negative expected
sample size under H1; corrected here and cited in `docs/research-map.md`). `E[Z | H]` is the
expected per-trial log-likelihood-ratio increment under `H`, computed via `stats::sprt::llr_delta`
- the same function that accumulates the real LLR in `sprt::run` itself.

**`expected_trials_under_h0`/`expected_trials_under_h1` are the two optimistic endpoints, not the
expected sample size for an unknown candidate.** A Wald SPRT's expected sample size is unimodal in
the true strength and *peaks between the two hypotheses*, not at either one - so a candidate whose
true strength lies somewhere between `elo0` and `elo1` (the common case: that uncertainty is
precisely why SPRT is being run) needs substantially more trials than either endpoint reports. This
isn't a small correction like the overshoot bias below - `tests/calibration/sprt_asn_calibration.rs`
measures the gap directly: at elo0=0/elo1=20/alpha=beta=0.05, the empirical mean at the midpoint
(true strength = 10 Elo) ran about 1.6x either endpoint's number. Budget above
`expected_trials_under_h0`/`expected_trials_under_h1`, not at them, whenever the candidate's true
strength is genuinely uncertain - which is the normal case, not an edge case.

**A known, honest approximation, quantified rather than just cited.** Wald's ASN formula ignores
"overshoot" - the LLR's real excess past a boundary at the moment a discrete process actually
crosses it, versus landing exactly on the boundary the formula assumes. A real run typically needs
somewhat more trials than this number in practice. `tests/calibration/sprt_asn_calibration.rs`
measures this empirically via Monte Carlo rather than leaving it as an unquantified caveat: at
elo0=0/elo1=20/alpha=beta=0.05, the real simulated mean ran about 1-2% higher than the formula's
prediction (both under H0 and under H1) - small at this elo gap, not claimed to hold at every
elo0/elo1/alpha/beta combination. This is the same underlying gap already listed in
`docs/research-map.md`'s deferred "Siegmund discrete-time bound correction" entry, now actually
surfaced by name in this context rather than left purely theoretical.

**Also not modeled: draws.** Like `power --metric elo`'s own draws gap (above), this counts
*decisive* trials only, matching `--sprt-variant wald` itself (draws don't move the LLR at all,
same "decisive games only" convention `winrate`/`sign-test` already use). A draw-heavy testcase
needs more real games than `expected_trials_under_h0`/`expected_trials_under_h1` says - use
`--sprt-variant trinomial`/`pentanomial` for draw-heavy testing, and treat this number as a
decisive-trials estimate, not a total-games one.

## `pass` / `fail` / `inconclusive`

**Not a citation-backed result - this project's own conservative design choice.** The gate
compares the confidence interval, not the point estimate, against the thresholds: `pass` requires
the CI's pessimistic (lower) bound to clear `--pass-above`; `fail` requires the CI's optimistic
(upper) bound to be at or below `--fail-below`. Anything else, including zero usable trials, is
`inconclusive`. Comparing a CI against a threshold is a standard decision rule, but *which*
threshold to use, and the "false pass is worse than inconclusive" bias behind picking the
pessimistic/optimistic bound rather than the point estimate, are veridict's own design decisions,
not a theorem.

## `estimated_additional_trials`

**Mixed: exact for three metrics, a heuristic for one.** This is a rough estimate of how many
*additional* trials would likely turn an `inconclusive` result decisive, assuming the effect size
itself doesn't move.

- For **`winrate`/`sign-test`/`elo`**, this binary-searches the real, already-tested CI function
  the report itself uses (`wilson`/`jeffreys`/`exact`, per `--ci-method`), holding the point
  estimate fixed - not an approximation, an exact search against real, already-verified math.
- For **`mean-diff`**, there is no closed-form "CI width at a hypothetical n" function for a
  bootstrap CI without real resampled data, so it falls back to the `O(1/sqrt(n))` CLT-scaling
  model instead. This has a documented, quantified bias: verified within ~1.5% of an actual re-run
  for a clean 4x sample-size jump at moderate n, but a real ~18% *under*-estimate at n=100, because
  e.g. Wilson's CI also shrinks via an `O(z^2/n)` recentering term the simple `1/sqrt(n)` model
  doesn't capture. Treat `mean-diff`'s number as "roughly this many, plausibly more," not a
  guarantee.

Returns `null` when there's nothing meaningful to suggest: the verdict is already `pass`/`fail`,
there are zero paired trials, or the effect sits *inside* the pass/fail threshold band (the "dead
zone") - shrinking the CI around a point estimate already in the dead zone can never cross either
boundary no matter how much data is added; only a genuinely different effect size resolves that
case, not more data alone.

## `warnings`

**Not citation-backed - conventional rules of thumb**, computed independently of `verdict` (a
warning never changes `pass`/`fail`/`inconclusive`):

- **Tiny sample** - under 30 paired trials, the conventional threshold below which CI methods are
  considered unreliable.
- **High failure rate** - over 20% of trials failed to execute (timeout/crash/invalid) rather than
  producing a usable result. Under `--failure-policy loss`, a failure's synthesized outcome is
  counted both as a failure and as a trial in this rate's denominator, which can under-report the
  true rate near the 20% boundary (see the `ponytail:` comment on `collect_data_quality` in
  `src/lib.rs`) - a known, narrow gap, not a silent one.
- **Draw-heavy** (`elo` only) - over 50% of trials were draws, leaving few decisive outcomes to
  estimate Elo from. (Not yet extended to `winrate`/`sign-test`, which discard their tie/draw count
  before it reaches the shared `MetricOutput` struct - a real gap, not a silent omission; see
  `docs/research-map.md`.)
- **Effect within noise floor** - the measured effect is smaller than the CI's own half-width
  (could plausibly be noise around zero), guarded by *not* also tiny-sample (a wide CI from a tiny
  sample already gets its own warning; flagging both would double-count the same underlying cause).
- **Low id diversity** - one `id` repeated 3 or more times among at least 10 id-tagged trials,
  unpaired mode only. `compare_one`'s CI treats every trial as independent; a heavily repeated `id`
  suggests the same underlying test case was logged multiple times rather than actually run that
  many independent times, which would make the CI narrower than the data really supports. Silent
  when every `id` appears exactly twice (the common, innocent case of forgetting
  `--paired-by-id` on genuinely paired data - flagging that would be noise, not signal) and silent
  entirely under `--paired-by-id` (repeated ids mean something different there - see
  [paired testcases](../README.md#paired-testcases)). This only catches literal `id` collisions -
  it says nothing about near-duplicate trials that don't share an `id`.

None of these thresholds (30, 20%, 50%, 3-of-10) come from a specific paper - they're the kind of
rule of thumb a careful practitioner would apply by hand, made automatic.
