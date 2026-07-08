# Research map

English | [日本語](research-map_ja.md)

Candidates we've considered, why they aren't shipped (yet or ever), and what would change the
decision. This is a "deferred, not rejected" list, not a roadmap - a future request to revisit any
of these is legitimate, not a re-litigation of a closed question. See `docs/metrics.md` for what
*is* implemented and its statistical basis.

## Statistical methods considered but not shipped

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

**Now partly covered:** `veridict plan` (see `docs/metrics.md`'s `plan` section) does this for the
`matrix`/tournament-comparison case - given `--min-elo` (the effect size worth detecting), it
estimates additional trials needed per pair, ranked most-uncertain first. What's still missing:
the same question for a plain two-way `compare`/`sprt` run (no matrix involved) - "how many trials
would I need to detect a `--min-effect` gap before running any of them at all" - and, separately,
budget-constrained allocation across a fixed trial budget (see the entry below), which `plan`
deliberately does not attempt.

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

**Why not yet:** same as power analysis - a real idea, not yet a concrete design, and it interacts
with `verdict::aggregate`'s existing "any fail sinks the whole run" logic in a way that needs
thinking through (correction changes *which* individual verdicts are significant, not how they
combine).

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
