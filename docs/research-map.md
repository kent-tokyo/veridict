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

### Pentanomial SPRT

**What it is:** a further generalization of the sequential test beyond the trinomial (draw-aware)
variant veridict already ships - modeling paired games (e.g. same opening played with colors
swapped) as one of five outcomes instead of three, which some chess-engine testing tools use to
reduce variance further on paired test setups.

**Why not yet:** the trinomial variant already solves the problem that originally motivated
looking at this (slow convergence on draw-heavy data); pentanomial's value is specifically for
paired-game test designs, which veridict doesn't have separate first-class support for outside
`--paired-by-id`'s existing win/loss/draw netting.

**What would change this:** a concrete workflow that pairs games in a way `--paired-by-id`'s
current netting doesn't already handle well.

### Power analysis / required-sample-size subcommand

**What it is:** given a desired effect size and confidence level, compute how many trials are
needed *before* running an experiment (the inverse of what `estimated_additional_trials` already
does reactively after an inconclusive result).

**Why not yet:** no concrete request has shaped what the output should look like yet - this is a
"P2 backlog" idea, not an in-progress design.

### Multiple-comparison correction for multi-metric runs

**What it is:** when a `compare` run requests several `--metric` flags together, each gets its own
independent verdict at the stated confidence level - running several tests without correction
(e.g. Bonferroni/Holm) inflates the overall false-positive rate across the combined result.

**Why not yet:** same as power analysis - a real idea, not yet a concrete design, and it interacts
with `verdict::aggregate`'s existing "any fail sinks the whole run" logic in a way that needs
thinking through (correction changes *which* individual verdicts are significant, not how they
combine).

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
