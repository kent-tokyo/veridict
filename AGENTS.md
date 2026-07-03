# AGENTS.md

## Project: veridict

`veridict` is a small, domain-agnostic evaluation gate for deciding whether a candidate implementation is actually better than a baseline.

It is not a benchmark runner, not an experiment tracker, and not a domain-specific evaluator.
It is the statistical decision layer that consumes evaluation results and returns a clear verdict:

* `pass`
* `fail`
* `inconclusive`

The name means: **verify results and return a verdict**.

## Core Mission

Build a lightweight Rust library and CLI that can compare two systems, models, algorithms, prompts, engines, or configurations from structured result files.

Examples:

* game engine A vs game engine B
* OCR model A vs OCR model B
* extraction pipeline A vs extraction pipeline B
* LLM prompt A vs LLM prompt B
* ranking algorithm A vs ranking algorithm B
* optimization method A vs optimization method B
* baseline release vs candidate release

The library must be useful outside any single domain.

## Design Principles

### 1. Domain-agnostic first

Do not bake in chess, shogi, OCR, LLM, chemistry, or benchmark-specific assumptions.

Allowed concepts:

* trial
* sample
* pair
* baseline
* candidate
* score
* win/loss/draw
* status
* timeout
* crash
* invalid output
* confidence interval
* statistical decision

Avoid domain-specific names such as `game`, `move`, `engine`, `prompt`, `document`, or `molecule` in core APIs unless they are only used in examples.

### 2. Small core, useful CLI

The Rust library should expose the statistical and reporting core.

The CLI should be a thin wrapper around the library.

Preferred shape:

```text
veridict-core      # library logic
veridict-cli       # command line interface, optional if workspace is used
```

A single crate is acceptable at the beginning if it keeps the API clean.

### 3. JSONL in, verdict out

The first stable input format should be JSONL.

Each line should represent one observation or paired comparison.

Support these broad result types:

```json
{"id":"case-001","baseline":0.81,"candidate":0.84}
{"id":"case-002","result":"candidate_win"}
{"id":"case-003","result":"draw"}
{"id":"case-004","baseline_status":"ok","candidate_status":"timeout"}
{"id":"case-005","baseline_status":"ok","candidate_status":"invalid"}
```

The exact schema may evolve, but keep it simple, documented, and stable.

### 4. CI-friendly behavior

The CLI must be suitable for GitHub Actions and local regression checks.

Exit codes should be deterministic:

```text
0 = pass
1 = fail
2 = inconclusive
3 = invalid input or configuration error
```

Never make CI users parse prose to determine success.

### 5. Explain the verdict

Every verdict must include enough information to understand why it happened.

Machine-readable JSON report is required.

Markdown report is strongly preferred.

Minimum report fields:

```json
{
  "verdict": "pass",
  "metric": "winrate",
  "baseline_count": 100,
  "candidate_count": 100,
  "paired_count": 100,
  "effect": 0.06,
  "confidence": 0.95,
  "ci_low": 0.02,
  "ci_high": 0.10,
  "timeouts": 1,
  "crashes": 0,
  "invalid": 0
}
```

### 6. Be conservative by default

If the data is insufficient, noisy, malformed, or statistically unclear, return `inconclusive`.

Do not overclaim improvements.

A false pass is worse than an inconclusive result.

## Initial Feature Set

### MVP

Implement the smallest useful version first.

Required:

* JSONL input
* paired numeric comparison
* win/loss/draw comparison
* timeout/crash/invalid status counting
* summary JSON output
* pass/fail/inconclusive verdict
* clear exit codes
* tests for edge cases

Recommended first statistical methods:

* mean difference
* win rate
* Wilson confidence interval for proportions
* bootstrap confidence interval for paired numeric data
* sign test for paired comparisons

Do not implement too many methods before the input model and report format are stable.

### Next Features

Add after MVP is stable:

* Elo difference from win/loss/draw
* Bradley-Terry model
* SPRT
* paired bootstrap
* regression thresholds
* Markdown report
* CSV input
* multiple metrics in one run
* grouped reports by tag
* failure classification summary
* comparison matrix for more than two candidates

## CLI Sketch

The CLI should feel simple.

Example:

```bash
veridict compare results.jsonl \
  --metric winrate \
  --min-effect 0.02 \
  --confidence 0.95
```

Numeric paired comparison:

```bash
veridict compare scores.jsonl \
  --metric mean-diff \
  --min-effect 0.01 \
  --confidence 0.95
```

Regression gate:

```bash
veridict compare results.jsonl \
  --metric winrate \
  --fail-below -0.01 \
  --pass-above 0.02 \
  --confidence 0.95 \
  --report-json report.json \
  --report-md report.md
```

SPRT, once implemented:

```bash
veridict sprt results.jsonl \
  --elo0 0 \
  --elo1 10 \
  --alpha 0.05 \
  --beta 0.05
```

## Data Model Guidelines

Prefer explicit structured types.

Suggested core enums:

```rust
pub enum Verdict {
    Pass,
    Fail,
    Inconclusive,
}

pub enum TrialStatus {
    Ok,
    Timeout,
    Crash,
    Invalid,
}

pub enum Outcome {
    BaselineWin,
    CandidateWin,
    Draw,
}

pub enum MetricKind {
    WinRate,
    MeanDiff,
    Elo,
    SignTest,
    BootstrapMeanDiff,
}
```

Keep serialization stable with `serde`.

Avoid exposing internal statistical implementation details in public types unless necessary.

## Statistical Correctness

Statistical code must be boring, explicit, and tested.

Rules:

* Avoid clever one-liners in statistical code.
* Prefer readable formulas.
* Include tests with known examples.
* Include edge-case tests:

  * zero samples
  * all wins
  * all losses
  * all draws
  * NaN input
  * missing IDs
  * duplicate paired IDs
  * candidate crashes
  * baseline crashes
  * invalid outputs
  * extremely small sample sizes
* Document assumptions for each method.
* Never silently ignore invalid data unless the user explicitly requested it.

Floating point behavior should be deterministic enough for CI.

Use tolerances in tests where appropriate.

## Rust Guidelines

Use stable Rust.

Prefer:

* `serde`
* `serde_json`
* `clap`
* `thiserror`
* `anyhow` only in binaries, not core library APIs
* `rand` only when needed for bootstrap, with deterministic seed support
* `statrs` only if it meaningfully reduces risk; avoid pulling in heavy dependencies casually

Public APIs should return typed errors.

Core library should not print to stdout or stderr.

CLI handles user-facing messages.

## Error Handling

Bad input should produce a clear error and exit code `3`.

Examples:

* invalid JSON
* missing required fields
* incompatible metric and input schema
* duplicate pair IDs when uniqueness is required
* empty input
* unsupported metric
* invalid confidence level
* invalid threshold

Do not panic on user input.

Panics are acceptable only for impossible internal states and should be avoided.

## Performance

The library should handle large JSONL files efficiently.

Target:

* streaming parse where practical
* avoid loading unnecessary fields
* deterministic memory usage for simple metrics
* allow bootstrap sample count to be configured
* avoid excessive dependencies

MVP does not need distributed execution.

## Reporting

Reports should be honest and compact.

JSON report is for machines.

Markdown report is for humans.

A Markdown report should include:

* verdict
* metric
* effect size
* confidence interval
* sample counts
* timeout/crash/invalid counts
* threshold configuration
* reason for pass/fail/inconclusive
* warnings

Example:

```markdown
# Veridict Report

Verdict: pass

Candidate improved win rate by 3.4 percentage points.
95% CI: +1.2 pp to +5.7 pp.
Threshold for pass: +2.0 pp.

Status counts:
- timeout: 1
- crash: 0
- invalid: 0
```

## Non-goals

Do not build these in the core project initially:

* web dashboard
* experiment database
* cloud service
* distributed workers
* domain-specific match runners
* LLM judge implementation
* OCR metric implementation
* chess/shogi protocol support
* plotting-heavy UI

These can be separate integrations later.

`veridict` should judge results.
It should not own every way of producing results.

## Relationship to Other Projects

`veridict` may be used by:

* Sekirei for engine strength regression
* OCR projects for accuracy regression
* SDS extraction tools for structured output quality
* chemistry tools for candidate ranking quality
* LLM workflows for prompt/model comparison
* optimization projects for algorithm comparison

However, `veridict` must not depend on any of these projects.

Keep the dependency direction one-way:

```text
sekirei / ocr / sds / llm tools
        depend on
veridict
```

Never make `veridict` depend on them.

## Development Workflow

Every change should preserve:

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
```

Add tests for every statistical method and every CLI decision branch.

When changing output schema, update examples and tests.

Breaking schema changes must be intentional and documented.

## Suggested Repository Layout

```text
.
├── AGENTS.md
├── README.md
├── Cargo.toml
├── crates
│   ├── veridict
│   │   └── src
│   │       ├── lib.rs
│   │       ├── input.rs
│   │       ├── metrics.rs
│   │       ├── verdict.rs
│   │       ├── report.rs
│   │       ├── stats
│   │       │   ├── mod.rs
│   │       │   ├── wilson.rs
│   │       │   ├── bootstrap.rs
│   │       │   ├── sign_test.rs
│   │       │   └── elo.rs
│   │       └── error.rs
│   └── veridict-cli
│       └── src
│           └── main.rs
├── examples
│   ├── winloss.jsonl
│   ├── paired_scores.jsonl
│   └── status_failures.jsonl
└── tests
    ├── cli_compare.rs
    └── fixtures
```

A simpler single-crate layout is acceptable for the first commit.

## Implementation Order

### Phase 1: MVP

1. Create Rust crate and CLI.
2. Define input record model.
3. Parse JSONL.
4. Implement win/loss/draw summary.
5. Implement paired numeric mean difference.
6. Implement Wilson CI.
7. Implement paired bootstrap CI.
8. Implement verdict thresholds.
9. Emit JSON report.
10. Add CLI exit codes.
11. Add examples and README.

### Phase 2: Better Gates

1. Add sign test.
2. Add Markdown reports.
3. Add CSV input.
4. Add failure classification.
5. Add multi-metric reports.
6. Add deterministic bootstrap seed.

### Phase 3: Engine/Ranking Support

1. Add Elo difference.
2. Add Bradley-Terry.
3. Add SPRT.
4. Add paired opening/testcase support.
5. Add comparison matrix.

## Quality Bar

A feature is not done until:

* it has tests
* it has at least one fixture
* it has documented assumptions
* JSON output is stable
* invalid input behavior is tested
* CLI exit behavior is tested

## Tone of the Project

Be small, sharp, and trustworthy.

The goal is not to produce impressive statistics jargon.
The goal is to help developers make better decisions without fooling themselves.

When uncertain, say `inconclusive`.
