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

## `quantile-diff` bootstrap confidence interval

**Established statistic, generalized from `mean-diff`'s mean to an arbitrary quantile** (`--quantile
Q`, default `0.5` = the median) of `candidate - baseline`. Useful where the mean isn't the number
that matters - a p95/p99 latency regression gate, for instance, where a handful of outliers
shouldn't move the verdict but a shift in the *typical worst case* should. Same input (paired
numeric records, `DiffCollector`), same `--resamples`/`--seed` semantics as `mean-diff`.

**Quantile convention:** type-7 linear interpolation (R's default `type=7`, also NumPy's
`percentile` default) - the least-surprising choice among several published conventions.
`--quantile` must be strictly inside `(0, 1)`; `0` or `1` (the sample min/max) is rejected as
`VeridictError::InvalidQuantile` rather than silently accepted, since the bootstrap distribution of
a sample extreme doesn't converge the way an interior quantile's does.

**Two of `mean-diff`'s three bootstrap methods (`--bootstrap-method`):**

- **`percentile` (default)** - resample `candidate - baseline` pairs with replacement, take the
  `alpha/2`/`1 - alpha/2` percentiles of the resampled quantiles.
- **`basic`** (reflected) - same reflection-around-the-point-estimate construction as `mean-diff`'s.

**`bca` is implemented but rejected as a config error** (`VeridictError::IncompatibleBootstrapMethod`),
not silently unavailable. The sample quantile is a non-smooth statistic (the empirical quantile
function is a step function), so BCa's jackknife acceleration term has no solid asymptotic footing
the way it does for the mean - an established statistical caveat, not a hunch. `tests/calibration/
quantile_coverage.rs` measures this directly rather than leaving it purely theoretical: at p95/n=30
on skewed data, BCa's measured coverage (0.7910) was statistically indistinguishable from plain
percentile's (0.7940) - no evidence the correction helps here the way it measurably does for
`mean-diff` (see `tests/calibration/bootstrap_coverage.rs`). The gate stays until calibration
evidence justifies lifting it, in either direction.

**Tail quantiles need real sample size, and the report says so.** A `q`-th quantile at small `n`
has only `n * min(q, 1-q)` expected observations in the thinner tail - p95 at n=30 has roughly 1.5,
p99 at n=30 has roughly 0.3 (a case `tests/calibration/quantile_coverage.rs` documents as
genuinely degenerate, not a bug: measured coverage there was 0.2670 against a 0.95 nominal target).
`data_quality.thin_quantile_tail` fires when that expected count drops below 10 (the same shape as
the binomial `np >= 10` rule of thumb), independent of the separate `tiny_sample` flag - a sample
large enough to clear `tiny_sample`'s `n >= 30` floor can still trip this one at an extreme `q`
(e.g. n=100 at q=0.95 has only ~5 expected tail observations).

**Limitation, not a bug: one quantile per `compare` invocation.** `--quantile` is a single
per-invocation flag, like `--bootstrap-method`; there's no way to request `quantile-diff` at two
different quantiles in one run. Run `compare` twice (once per quantile) if you need both a p50 and
a p95 gate, for instance.

**Not supported: `power --metric quantile-diff` and `matrix`/`plan`.** Power needs an
order-statistic asymptotic variance (or a density estimate at the quantile) - a separate research
problem from `mean-diff`'s closed-form power, not a mirror of it (see `docs/research-map.md`).
`estimated_additional_trials`/`--correction` treat `quantile-diff` exactly like `mean-diff`: the
`O(1/sqrt(n))` CLT-scaling fallback for the former (no closed-form CI-width-at-n function for
either's bootstrap CI), and exclusion from individual correction while still counting toward
`family_size` for the latter (no closed-form CI-at-a-hypothetical-confidence function either).

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

## `--max-timeouts` / `--max-crashes` / `--max-invalid` (`validity`)

**This project's own design choice, not a citation-backed method** - hard, zero-tolerance-style
caps on `compare`/`sprt`'s existing `timeouts`/`crashes`/`invalid` counts, orthogonal to
`--failure-policy`. `--failure-policy` controls whether a failure changes *which outcome a trial
contributes* to the computation; these caps control whether the run is trustworthy enough to read
a `verdict` off of *at all*. Breaching a cap sets `validity: invalid`, forces `verdict:
inconclusive` (overwriting whatever `verdict::decide`/the LLR boundary check actually produced),
and clears `estimated_additional_trials` (more trials can't fix a technical-failure problem) -
applied as a final pass over an already-built report (`verdict::apply_failure_caps`/
`sprt::apply_failure_caps`), the same "mutate a finished report" shape `--correction` already
uses for a different cross-cutting concern.

Deliberately an absolute count, not a rate: `data_quality.high_failure_rate` (over 20% of trials)
already covers "this run's failure *rate* looks unusually high," advisory-only, never changing
`verdict`. A cap is for the opposite case - a single technical failure that must matter regardless
of how many thousands of clean trials surround it (`--max-crashes 0`), which no rate threshold can
express: at n=10,000, even 5 crashes is a 0.05% rate, far under any reasonable "high failure rate"
bar. The concrete failure mode this closes: under `--failure-policy loss`, enough crashes can tip
a numeric `winrate`/`elo` verdict to `fail` (or, in principle, `pass`) even though the real cause
was infrastructure, not candidate strength - `--max-crashes 0` catches that before it ever reaches
the report, rather than requiring a human to separately notice `crashes > 0` next to a confident-
looking `fail`.

Each cap is independently optional (`None`/unset is uncapped - existing behavior for a run that
never passes any `--max-*` flag), and all three apply identically to `sprt`, across every
`--sprt-variant` (a `loss`-synthesized failure counts toward the same `timeouts`/`crashes`/
`invalid` totals `sprt`'s report already carries, regardless of variant). For a `compare
--metric`-family run, `validity`/`promotion` are computed per report but the underlying
`timeouts`/`crashes`/`invalid` counts are identical across every metric in one run (one shared
scan over the input, see `metrics::compute_many`) - the overall `MultiReport.validity` is
`invalid` if *any* member report's is.

See [Validity, strength, and promotion](../README.md#validity-strength-and-promotion) in the
README for the field semantics (`validity`/`verdict`/`promotion` as three separate axes) and a
worked example.

## `--cluster-by-id` (winrate/elo)

**Established statistic - cluster bootstrap, a standard nonparametric technique (see Field &
Welsh 2007, "Bootstrapping Clustered Data"; Cameron, Gelbach & Miller 2008 for the cluster-robust
inference literature this generalizes), applied here rather than derived from scratch.** Every CI
`compare` ships treats each record as an independent trial. That assumption breaks when many
records share a common source of correlation - the same opening replayed several times, the same
underlying testcase logged repeatedly - and the *un*-clustered CI is then too narrow: it counts
correlated repeats as if they were independent evidence. `--cluster-by-id` treats every id (an id
used once is its own singleton cluster) as one resampling unit instead: each bootstrap replicate
draws whole clusters with replacement - not individual records - and recomputes the metric
statistic (winrate's proportion, elo's score) from the pooled resampled records, the same
`stats::sprt`-independent Wilson-vs-bootstrap distinction `matrix`'s general-graph mode already
draws for its own Elo CIs (bootstrap when the naive closed form's independence assumption doesn't
hold).

**Structurally different from `--paired-by-id`, not a stricter version of it.** Pairing *nets* an
id's exactly-two records into one observation (see "Paired testcases" above); clustering *keeps*
every record in an id group as its own resampling unit; the two describe incompatible treatments
of a repeated id and are mutually exclusive (`ClusterByIdConflictsWithPairedById`).

**`effective_sample_size`/`design_effect` come from the same bootstrap as the CI, not a separately
computed intra-class correlation.** `design_effect = Var(cluster bootstrap) / Var(i.i.d.
bootstrap)` - both variances estimated from the identical pooled data via the same resampling
family (`stats::bootstrap::cluster_bootstrap_ci`/`iid_bootstrap_outcome_draws`), so the two numbers
are directly comparable rather than mixing a closed-form binomial variance with a bootstrap one
(which could disagree at small n for reasons that have nothing to do with clustering - the same
internal-consistency discipline `estimate_additional_trials` already follows by searching the
exact CI function a report displays, not a different approximation of it).
`effective_sample_size = paired_count / design_effect` is the standard Kish (1965) deflation of a
naive sample size under clustering - "how many truly independent trials this clustered data is
actually worth." `design_effect` near 1.0 means little measurable clustering effect (a small
`--cluster-by-id` CI difference from the unclustered case is expected, not a sign of a bug);
noticeably above 1.0 means the unclustered CI would have been overconfident.

**`cluster_count`/`max_cluster_size` are the plain descriptive stats underneath both** - the
number of distinct clusters (openings/testcases) and the largest single cluster's record count
(e.g. the most-repeated opening) - reported unconditionally alongside the CI, no estimator
involved.

**`low_id_diversity` is reinterpreted the same way `--paired-by-id` already reinterprets it**: a
repeated id is the entire point of clustering, not a sign of a data mistake, so
`records_with_id`/`max_id_count` tracking (and the warning built from it) is skipped entirely
under `--cluster-by-id`, exactly as it already is under `--paired-by-id`.

**`estimated_additional_trials` is `null` under `--cluster-by-id`**, not merely unpopulated: it
would otherwise binary-search wilson/jeffreys/exact against a report whose displayed CI is a
cluster bootstrap, the exact "different approximation of it" the paragraph above says this project
avoids - and the independent unit under clustering is the cluster, not the trial, so `paired_count`
isn't even the right `n` to scale a search from. See `estimated_additional_trials` below.

**Only `winrate`/`elo` this round** (`IncompatibleClusterById` for any other requested metric).
`mean-diff`/`sign-test`/`quantile-diff` are numeric-diff metrics already bootstrapped by record,
not by outcome tally - real cluster support for them needs `DiffCollector` (not `OutcomeCollector`)
to retain cluster structure through to resampling, a genuinely separate piece of wiring, not a
mechanical extension of this round's work. See `docs/research-map.md`.

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

**`mean-diff` is a closed-form calculation, not this search** - see `### power --metric mean-diff`
below.

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

#### `--horizon N`: probability of no decision by a fixed trial cap

**This project's own design choice, not a citation-backed method.** `expected_trials_under_h0`/
`expected_trials_under_h1` answer "how many trials, on average"; `--horizon N` answers a different,
sharper planning question a real gate design needs: "if I stop at `N` trials no matter what, how
often will I still have nothing?" There is no simple closed form for a random walk's boundary-
crossing-time *distribution* at a general drift (only its *mean*, which the ASN formula above
already gives) - so this is deliberately boring Monte Carlo simulation, not a derived formula:
2,000 independent replications (`stats::bootstrap::DEFAULT_SEED`, so the same inputs always give
the same answer), each simulating raw Bernoulli trials against the exact same `stats::sprt::
{bounds, llr_delta}` math `sprt::run`'s real Wald loop decides with, counting how often the
simulated LLR never crosses either boundary within `N` steps.

**Evaluated at the midpoint, not either endpoint - the same worst case
`expected_trials_under_h0`/`expected_trials_under_h1`'s own doc above already establishes.**
`score_from_elo((elo0 + elo1) / 2.0)` is the true win probability simulated from; reporting this at
`elo0`/`elo1` instead would understate the real risk for a candidate of genuinely unknown strength,
for the same reason budgeting at the ASN endpoints understates expected sample size.

**Not a stopping rule.** This is a design aid for choosing the *next* gate's trial budget/cutoff,
exactly like `estimated_additional_trials`/`power`'s other numbers - it never changes how a real
`veridict sprt` run should be stopped, since that run's own `--alpha`/`--beta` boundaries already
fully and correctly determine that regardless of how long it takes.

### `power --metric mean-diff`

**A closed-form calculation, not the search above.** `mean-diff` has no closed-form CI-width-at-n
function without real resampled data (the same reason `estimated_additional_trials` falls back to
an `O(1/sqrt(n))` approximation for it post-hoc) - but pre-experiment there's no fallback
available either, since there's no existing sample to approximate a variance from at all. Given an
assumed standard deviation from the caller (`--assume-sd <f64>`, or estimated from real pilot data
via `--pilot FILE`), modeling the sample mean of `n` diffs as `Normal(assume_effect,
assume_sd^2/n)` - the standard pre-experiment assumption, since there's no real data yet to
bootstrap - makes the power calculation continuous and monotone in `n`, unlike the discrete
binomial case above: there's an exact closed-form solution, so `power --metric mean-diff` doesn't
run a search at all.

**The formula:**

```
z_conf  = inverse_normal_cdf((1 + confidence) / 2)   // two-sided quantile - see below
z_power = inverse_normal_cdf(target_power)
n       = ceil( ((z_conf + z_power) * assume_sd / (assume_effect - min_effect))^2 )
achieved_power = Phi( (assume_effect - min_effect) * sqrt(n) / assume_sd - z_conf )
```

**`z_conf` must be the two-sided quantile - the one correctness subtlety here, and it's the same
one that already shaped this project's design twice before.** `compare`'s own CI is built as an
ordinary *two-sided* `(1-confidence)` interval and then read one-sidedly (only the lower bound
matters for a pass); `wilson_ci_from_proportion` computes its own `z` the same way
(`inverse_normal_cdf(1 - alpha/2)`, i.e. exactly `inverse_normal_cdf((1+confidence)/2)` - 1.96 at
95% confidence, not the one-sided 1.645). Using the one-sided quantile here would compute *fewer*
trials than actually needed - the optimistic, false-pass-prone direction this project exists to
avoid. This is the same "a two-sided CI read one-sidedly only carries half its nominal budget in
that tail" fact that already shaped `power`'s two-effect-value design (above) and
`--correction`'s `alpha/2` family target (see that section) - verified consistent here by
construction, not re-derived from scratch, and confirmed independently before implementation.

**This is a normal approximation of a real bootstrap decision rule, not an exact search against
one - a different kind of estimate from `winrate`/`sign-test`/`elo`'s.** There's no real data
pre-experiment to bootstrap, so a normal model of the paired differences is the standard
assumption; `report.method` says `normal_approximation_closed_form`, not
`exact_binomial_search`, and `report.ci_method` says `"normal"` (a label, not a real `--ci-method`
choice). For skewed real diffs, the bootstrap CI `compare` actually computes and this normal
estimate will diverge - `tests/calibration/power_mean_diff_calibration.rs` measures the real gap
empirically (drawing synthetic normal diffs and running them through the actual bootstrap rule via
`compare_one`, not a simulated approximation of it): at two tested configs, empirical pass rate
tracked `target_power` within ~0.004-0.015, no systematic directional bias the way SPRT's ASN
formula had one.

**`assume_sd` is the standard deviation of the paired *difference* (`candidate - baseline`), not
either arm's own standard deviation.** The classic paired-design mislabeling risk: using an arm's
own SD here understates the true variance for anything but a perfectly correlated pair, silently
corrupting every number downstream. `--pilot FILE` computes this correctly by construction - the
same `(candidate - baseline)` diffs `compare --metric mean-diff` itself computes, via the same
`DiffCollector` (including `--paired-by-id` netting), just without the bootstrap step (there's
nothing to bootstrap-check pre-experiment; only the sample standard deviation of the diffs is
needed).

**`--pilot FILE`'s caveats.** Fewer than 2 usable diffs, or a pilot with zero variance (every diff
identical), is rejected as a clear error rather than producing `NaN`/`0` silently. Fewer than 30
diffs (the same conventional threshold `data_quality.tiny_sample` already uses elsewhere) adds a
note: the sample standard deviation itself is a rougher estimate at that size, and normal
quantiles (used here) slightly underestimate the required `n` relative to a small-sample
`t`-distribution correction, which this round doesn't implement - a documented caveat, not a
silently wrong number.

## `pass` / `fail` / `inconclusive`

**Not a citation-backed result - this project's own conservative design choice.** The gate
compares the confidence interval, not the point estimate, against the thresholds: `pass` requires
the CI's pessimistic (lower) bound to clear `--pass-above`; `fail` requires the CI's optimistic
(upper) bound to be at or below `--fail-below`. Anything else, including zero usable trials, is
`inconclusive`. Comparing a CI against a threshold is a standard decision rule, but *which*
threshold to use, and the "false pass is worse than inconclusive" bias behind picking the
pessimistic/optimistic bound rather than the point estimate, are veridict's own design decisions,
not a theorem.

## `--correction`

**Why this exists.** `compare --metric elo --metric winrate --metric sign-test` runs several
metrics against the same candidate in one call; `verdict::aggregate` combines them (any `fail`
sinks the run, else any `inconclusive` holds it back, else `pass`). But each metric's own
pass/fail decision is made independently at the stated `--confidence` - run enough metrics (or,
across a broader campaign, enough candidates) and the chance that *something* clears its bar by
luck alone climbs. That directly undermines this project's own "a false pass is worse than an
inconclusive result" bias (see above): an uncorrected multi-metric family is more likely to
produce a lucky pass than any single metric run alone would be. `--correction none` (the default)
is exactly today's existing behavior, unchanged - correction is opt-in.

**The family-error target: no worse than today's own single-metric baseline.** `compare`'s pass
rule already reads a *two-sided* `(1-confidence)` CI's *lower* bound as a one-sided pass signal -
a two-sided interval splits its error budget evenly between both tails, so a single, uncorrected
metric today already has a one-sided false-pass rate of `alpha/2` (e.g. 0.025 at the default 95%
confidence), not the nominal `alpha`. The natural, and only defensible, correction target is
therefore "running `m` metrics together is no more dangerous than running one" - keep the
*family's* one-sided false-pass rate at that same `alpha/2`, not the nominal `alpha` itself (which
would let a "corrected" multi-metric family tolerate a *higher* false-pass rate than a single
uncorrected metric already has today). This falls straight out of standard textbook Bonferroni
simultaneous confidence intervals (Dunn 1961; Miller, *Simultaneous Statistical Inference*, 1966):
recompute each test's ordinary, symmetric, two-sided CI at confidence `1 - alpha/m` (the same
`alpha = 1-confidence` the tool already uses, split across the family - no extra factor of
anything) and re-run the same pass/fail rule against it.

**`--correction bonferroni`**: a uniform significance budget `alpha/family_size` for every metric
in the run, regardless of how strong or weak each one's own evidence is.

**`--correction holm`** (recommended over Bonferroni): sorts metrics by their own achieved
significance ascending and steps down, comparing the `k`-th (1-based) most significant result to
`alpha/(family_size-k+1)`, stopping at the first failure. Uniformly more powerful than Bonferroni
for the same family-wise guarantee - it never rejects fewer true passes than Bonferroni would, and
often rejects strictly more (Holm 1979). A report past an early failure in the ordered sequence is
held back regardless of its own significance, because that sequential stop is what gives Holm its
guarantee, not independent per-metric comparisons.

**Correction can only downgrade a pass, never invent a fail.** Widening a CI (lower confidence
budget per test) only ever pushes its lower bound down and its upper bound up. For a report that
already passed (`ci_low >= pass_above`, which by construction means `ci_low > fail_below` too,
since `pass_above > fail_below`), a wider `ci_high` can never newly satisfy `ci_high <= fail_below`.
So correction only ever moves an unadjusted `pass` to `inconclusive` - it never fails a metric that
wasn't already failing, and never touches a metric whose unadjusted verdict was already `fail` or
`inconclusive`. That asymmetry falls straight out of the math above; it isn't a special case.

**`mean-diff`/`quantile-diff` count toward `family_size` but keep their own, unadjusted verdict.**
There is no closed-form CI-at-a-hypothetical-confidence function for either's bootstrap CI without
real resampled data (same reason `estimated_additional_trials`/`power` both special-case them) -
such a report's own pass/fail is left as computed. It still counts toward `family_size`, though:
excluding it would under-count the real multiplicity risk the *other* metrics in the same run are
actually exposed to - the conservative choice.

**Report fields** (all omitted, not present as `null`, unless `--correction` is something other
than the default `none`): `correction_method` (`"bonferroni"`/`"holm"`), `family_size`,
`achieved_alpha` (the smallest one-sided significance at which this report's own CI would still
pass - `null`/omitted for `mean-diff`/`quantile-diff`), `adjusted_alpha_threshold` (the corrected
threshold `achieved_alpha` was actually compared against), and `unadjusted_verdict` (the verdict before
correction - `verdict` itself becomes the *adjusted* value, since that's the field the exit code
and `verdict::aggregate` actually act on).

**A single-metric run degenerates to a no-op.** With `family_size=1`, both Bonferroni's and Holm's
threshold reduce to `alpha/1 = alpha` - exactly the report's own existing, uncorrected pass
condition. `--correction` is accepted and reported on a single-`--metric` run for uniformity, but
never changes its verdict.

**`estimated_additional_trials` stays `null` on a correction-downgraded report.** It's computed
once, before correction runs, from the *unadjusted* verdict - a report downgraded from `pass` to
`inconclusive` by correction still shows `null` there, same as any other `pass`. A consumer that
assumes "inconclusive always has a trials estimate" will see `null` for this new reason too; the
number correction's `inconclusive` would actually need (more trials at the *corrected* confidence,
not the original one) isn't computed this round.

**Out of scope for now**: `matrix`'s all-pairs correction (matrix has no verdict concept to correct
at all today - see `docs/research-map.md`'s "matrix verdict semantics" entry), `sprt`'s own
multiplicity question (running several simultaneous SPRTs), and Benjamini-Hochberg/FDR-style
correction (a different, less conservative family-error target than FWER).

## `estimated_additional_trials`

**Mixed: exact for three metrics, a heuristic for one.** This is a rough estimate of how many
*additional* trials would likely turn an `inconclusive` result decisive, assuming the effect size
itself doesn't move.

- For **`winrate`/`sign-test`/`elo`**, this binary-searches the real, already-tested CI function
  the report itself uses (`wilson`/`jeffreys`/`exact`, per `--ci-method`), holding the point
  estimate fixed - not an approximation, an exact search against real, already-verified math.
- For **`mean-diff`/`quantile-diff`**, there is no closed-form "CI width at a hypothetical n"
  function for a bootstrap CI without real resampled data, so both fall back to the
  `O(1/sqrt(n))` CLT-scaling model instead. This has a documented, quantified bias for
  `mean-diff`: verified within ~1.5% of an actual re-run for a clean 4x sample-size jump at
  moderate n, but a real ~18% *under*-estimate at n=100, because e.g. Wilson's CI also shrinks via
  an `O(z^2/n)` recentering term the simple `1/sqrt(n)` model doesn't capture. `quantile-diff`
  reuses the same model, unverified for its own bootstrap CI. Treat either metric's number as
  "roughly this many, plausibly more," not a guarantee.

Returns `null` when there's nothing meaningful to suggest: the verdict is already `pass`/`fail`,
there are zero paired trials, or the effect sits *inside* the pass/fail threshold band (the "dead
zone") - shrinking the CI around a point estimate already in the dead zone can never cross either
boundary no matter how much data is added; only a genuinely different effect size resolves that
case, not more data alone. Also `null` whenever `--cluster-by-id` was used (see
`--cluster-by-id` above) - none of the binary-searched CI functions describe a cluster bootstrap,
and the independent unit is the cluster, not the trial.

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
- **Thin quantile tail** (`quantile-diff` only) - fewer than 10 expected observations
  (`paired_count * min(q, 1-q)`) in the thinner tail at the requested quantile - see the
  `quantile-diff` section above.

None of these thresholds (30, 20%, 50%, 3-of-10, 10-expected-in-the-tail) come from a specific
paper - they're the kind of rule of thumb a careful practitioner would apply by hand, made
automatic.
