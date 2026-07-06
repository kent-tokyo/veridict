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

**Established statistic**, two variants (`--sprt-variant`):

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

Neither variant is a heuristic - both are the referenced sequential test, evaluated exactly against
the observed counts. What *is* a design choice is which variant runs by default (`wald`, since it
needs no nuisance-parameter estimation) and that veridict doesn't (yet) implement the further
pentanomial extension some chess-engine testers use (see `docs/research-map.md`).

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
  producing a usable result.
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
