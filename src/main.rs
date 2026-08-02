//! Thin CLI wrapper around the `veridict` library. Owns all stdout/stderr
//! output and exit codes; the library itself never prints.

use std::io::{self, BufRead};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand, ValueEnum};

use veridict::correction::Correction;
use veridict::sprt::{SprtConfig, SprtVariant};
use veridict::stats::bootstrap::{DEFAULT_SEED, sample_sd};
use veridict::verdict::{self, Thresholds};
use veridict::{
    BootstrapMethod, CiMethod, FailureCaps, FailurePolicy, MetricConfig, MetricKind, Verdict,
    VeridictError, input, matrix, power, time_sensitive,
};

#[derive(Parser)]
#[command(
    name = "veridict",
    version,
    about = "Statistical decision gate: is the candidate actually better than the baseline?"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Compare candidate vs baseline results and emit a pass/fail/inconclusive verdict.
    Compare(CompareArgs),
    /// Sequential probability ratio test: accumulate evidence until the candidate is clearly
    /// at least elo1 stronger (pass), clearly at most elo0 stronger (fail), or keep testing.
    Sprt(SprtArgs),
    /// Compare more than two candidates, each measured against the same shared baseline, and
    /// tabulate pairwise Elo differences. Report-only: always exits 0 on success (no single
    /// pass/fail verdict applies to a whole matrix).
    Matrix(MatrixArgs),
    /// Recommend which pairs from a matrix/tournament result would most reduce uncertainty with
    /// more trials, ranked most-uncertain first. Report-only, same input as `matrix`.
    Plan(PlanArgs),
    /// Pre-experiment sample-size estimate: how many trials would `compare` need for a target
    /// probability of reaching a passing verdict. Report-only, a pure calculation from flags -
    /// except `--metric mean-diff --pilot FILE`, which reads real pilot data to estimate a
    /// standard deviation from (the one input file this subcommand takes).
    Power(PowerArgs),
    /// Checks that a run's raw data is structurally sound before its statistics (`compare`/
    /// `sprt`) are trusted: pairing, ordering, contamination, and opaque-identifier consistency
    /// between a declared `manifest.toml` and the actual `games.jsonl`. Not verdict-shaped (no
    /// pass/fail/inconclusive) - its own binary judgment instead: exit 0 if no structural
    /// violation was found, exit 1 if one or more were, exit 3 only for a genuine parse/config
    /// error (malformed manifest, unsupported manifest schema version, malformed games file,
    /// empty input, or a manifest that declares nothing to verify) where no report can be
    /// produced at all.
    VerifyRun(VerifyRunArgs),
    /// Time-sensitive anytime-valid testing (Bernoulli simple-vs-simple only): rather than only
    /// asking whether the candidate is decisively better, choose a betting policy that maximizes
    /// expected reward under a reward-schedule that favors an *early* rejection. Independent of
    /// `sprt` - see `veridict::time_sensitive`'s module doc. One-sided: exit 0 (pass) once wealth
    /// crosses 1/alpha, exit 2 (inconclusive) if the reward schedule's horizon is reached first
    /// (never a statistical fail - there is no lower rejection boundary in this model), exit 3
    /// for a genuine parse/config error.
    TimeSensitive(TimeSensitiveArgs),
}

#[derive(clap::Args)]
struct CompareArgs {
    /// Input file. Use "-" to read from stdin.
    input: PathBuf,

    /// Repeat to run several metrics against the same input and combine
    /// them into one verdict (fail dominates, then inconclusive, then pass).
    #[arg(long = "metric", value_enum, required = true)]
    metrics: Vec<MetricArg>,

    /// Input format. Defaults to sniffing the file extension (.csv vs
    /// everything else); pass explicitly when reading CSV from stdin.
    #[arg(long, value_enum)]
    format: Option<FormatArg>,

    #[arg(long, default_value_t = 0.95)]
    confidence: f64,

    /// Symmetric effect-size threshold: pass if the CI lower bound is >= this, fail if the CI
    /// upper bound is <= -this. A bare number (e.g. `0.005`) applies to every requested metric;
    /// `metric=value` (e.g. `sign-test=0.01`) overrides just that metric - comma-separate or
    /// repeat the flag to mix both, since different metrics' effect sizes live on different
    /// scales (sign-test/winrate: win-rate margin off 0.5; relative-diff: relative ratio;
    /// mean-diff/quantile-diff/elo: raw units) and sharing one number across mixed metrics in a
    /// multi-metric run is only coincidentally correct - a bare default shared across metrics on
    /// different scales prints a stderr warning naming the mismatched metrics. Ignored if
    /// --pass-above/--fail-below are given.
    #[arg(long, value_delimiter = ',', value_parser = parse_min_effect_entry)]
    min_effect: Vec<MinEffectEntry>,

    /// Explicit pass threshold. Requires --fail-below.
    #[arg(long, requires = "fail_below", allow_hyphen_values = true)]
    pass_above: Option<f64>,

    /// Explicit fail threshold. Requires --pass-above.
    #[arg(long, requires = "pass_above", allow_hyphen_values = true)]
    fail_below: Option<f64>,

    /// Bootstrap resample count, used only by --metric mean-diff/quantile-diff/relative-diff.
    #[arg(long, default_value_t = 10_000)]
    resamples: usize,

    /// Bootstrap RNG seed, used only by --metric mean-diff/quantile-diff/relative-diff. Defaults
    /// to a fixed seed, so the same input reproduces bit-identical output in CI.
    #[arg(long)]
    seed: Option<u64>,

    /// Confidence interval method for --metric winrate/sign-test. `exact`
    /// (Clopper-Pearson) and `jeffreys` are only valid for those two metrics;
    /// combining either with --metric elo/mean-diff/relative-diff is a config error, not a
    /// silent fallback.
    #[arg(long, value_enum, default_value = "wilson")]
    ci_method: CiMethodArg,

    /// Bootstrap variant for --metric mean-diff/quantile-diff/relative-diff. `bca` corrects for
    /// bias and skewness (not available for quantile-diff - the sample quantile's jackknife
    /// acceleration term has no solid footing for a non-smooth statistic, a config error rather
    /// than a silent fallback; available for relative-diff, whose sample mean is smooth the same
    /// way mean-diff's is); `basic` reflects the percentile interval around the point estimate
    /// (simpler, no bias-correction of its own); `percentile` stays the default so existing CI
    /// numbers don't silently shift.
    #[arg(long, value_enum, default_value = "percentile")]
    bootstrap_method: BootstrapMethodArg,

    /// Quantile to measure for --metric quantile-diff (e.g. 0.95 for p95); must be in (0, 1).
    /// Defaults to 0.5 (the median) when --metric quantile-diff is used without this flag.
    /// Unused by every other requested metric, so it can be combined with e.g. --metric
    /// mean-diff in the same multi-metric run - but a config error if no --metric
    /// quantile-diff is requested at all.
    #[arg(long)]
    quantile: Option<f64>,

    /// Treat two records sharing an id as one testcase played twice (e.g.
    /// roles swapped to cancel the testcase's own bias) and combine them
    /// into a single net observation instead of two independent ones. An id
    /// used only once is an ordinary unpaired sample; 3+ uses of the same
    /// id is rejected as a data error.
    #[arg(long, conflicts_with = "cluster_by_id")]
    paired_by_id: bool,

    /// Only for --metric winrate/elo: treat every record sharing an id as one correlated
    /// cluster (e.g. the same opening/testcase replayed several times) instead of independent
    /// trials, and switch the CI from the closed-form method to a cluster bootstrap that
    /// resamples whole clusters - correctly widening the interval when trials aren't truly
    /// independent. Adds cluster_count/max_cluster_size/effective_sample_size/design_effect to
    /// the report. Mutually exclusive with --paired-by-id (nets exactly two records per id into
    /// one observation, a different treatment of a repeated id) and any other requested metric
    /// (mean-diff/sign-test/quantile-diff/relative-diff cluster support is deferred, see
    /// docs/research-map.md).
    #[arg(long, conflicts_with = "paired_by_id")]
    cluster_by_id: bool,

    /// How a failed trial affects --metric winrate/elo (mean-diff/sign-test reject anything but
    /// the default as a config error). `report-only` (default): failures are tallied and
    /// reported but never contribute an outcome, same as before this flag existed. `exclude`:
    /// a failed side's `result` (if present) is never tallied as an outcome either - only
    /// diverges from `report-only` when a record carries both a failure status and a `result`.
    /// `loss`: a failed side's outcome is synthesized (candidate failed -> baseline_win,
    /// baseline failed -> candidate_win, both failed -> draw), overriding any literal `result`
    /// on the same record.
    #[arg(long, value_enum, default_value = "report-only")]
    failure_policy: FailurePolicyArg,

    /// Multiple-comparison correction across this run's metric family of *simultaneous per-metric
    /// claims* (relevant whenever more than one --metric is given; a single-metric run is a
    /// harmless no-op) - populates each report's family_adjusted_verdict/family_adjusted_promotion
    /// and, for a multi-metric run, the overall simultaneous_claims_promotion. Does NOT change
    /// verdict/promotion above (the deployment gate already requires every metric to pass, which
    /// needs no correction of its own - see docs/metrics.md's --claim-correction section). `none`
    /// (default): today's existing behavior, unchanged. `bonferroni`: uniform per-metric
    /// significance alpha/family_size. `holm`: step-down, uniformly more powerful than Bonferroni
    /// for the same family-wise guarantee. Rejected as a configuration error for a family that
    /// includes --metric mean-diff/quantile-diff/relative-diff (no closed-form CI to correct) or
    /// was run with --cluster-by-id (correction would reconstruct the wrong CI shape).
    #[arg(long, value_enum, conflicts_with = "correction")]
    claim_correction: Option<CorrectionArg>,

    /// Deprecated alias for --claim-correction, kept for one release. Prints a deprecation
    /// warning to stderr and behaves identically; --claim-correction/--correction are mutually
    /// exclusive.
    #[arg(long, value_enum, conflicts_with = "claim_correction")]
    correction: Option<CorrectionArg>,

    /// Hard cap on candidate+baseline timeout count. Breaching it forces validity=invalid,
    /// verdict=inconclusive, promotion=not_promoted regardless of the metric's own effect/CI -
    /// unlike --failure-policy, this can never be satisfied by more clean trials. Unset (default):
    /// uncapped, existing behavior.
    #[arg(long)]
    max_timeouts: Option<u64>,

    /// Hard cap on candidate+baseline crash count. See --max-timeouts for the exact semantics.
    #[arg(long)]
    max_crashes: Option<u64>,

    /// Hard cap on candidate+baseline invalid-result count. See --max-timeouts for the exact
    /// semantics.
    #[arg(long)]
    max_invalid: Option<u64>,

    /// Also write the JSON report to this file.
    #[arg(long)]
    report_json: Option<PathBuf>,

    /// Also write a human-readable Markdown report to this file.
    #[arg(long)]
    report_md: Option<PathBuf>,
}

#[derive(clap::Args)]
struct SprtArgs {
    /// Input file. Use "-" to read from stdin.
    input: PathBuf,

    /// Input format. Defaults to sniffing the file extension (.csv vs
    /// everything else); pass explicitly when reading CSV from stdin.
    #[arg(long, value_enum)]
    format: Option<FormatArg>,

    /// SPRT variant. `wald` (default): classic two-outcome test, draws
    /// excluded, via --elo0/--elo1 (logistic Elo). `trinomial`: draw rate
    /// estimated as a nuisance parameter, converges faster on draw-heavy
    /// data, via --belo0/--belo1 (BayesElo - a different scale from
    /// logistic Elo whenever the estimated draw rate is nonzero, see
    /// stats::trinomial_sprt's doc). `pentanomial`: paired-game test (same
    /// opening, colors swapped) over the pair's 5-value combined score,
    /// via --elo0/--elo1 (logistic Elo, same scale as wald) - always
    /// requires --paired-by-id, see stats::pentanomial_sprt's doc.
    #[arg(long, value_enum, default_value = "wald")]
    sprt_variant: SprtVariantArg,

    /// H0 for --sprt-variant wald: the candidate is at most this many
    /// logistic-Elo points stronger.
    #[arg(long, allow_hyphen_values = true)]
    elo0: Option<f64>,

    /// H1 for --sprt-variant wald: the candidate is at least this many
    /// logistic-Elo points stronger.
    #[arg(long, allow_hyphen_values = true)]
    elo1: Option<f64>,

    /// H0 for --sprt-variant trinomial: the candidate is at most this many
    /// BayesElo points stronger.
    #[arg(long, allow_hyphen_values = true)]
    belo0: Option<f64>,

    /// H1 for --sprt-variant trinomial: the candidate is at least this many
    /// BayesElo points stronger.
    #[arg(long, allow_hyphen_values = true)]
    belo1: Option<f64>,

    /// False-positive rate: probability of accepting H1 when H0 is true.
    #[arg(long, default_value_t = 0.05)]
    alpha: f64,

    /// False-negative rate: probability of accepting H0 when H1 is true.
    #[arg(long, default_value_t = 0.05)]
    beta: f64,

    /// Treat two records sharing an id as one testcase played twice and
    /// combine them into a single net observation. See `compare
    /// --paired-by-id` for the exact semantics.
    #[arg(long)]
    paired_by_id: bool,

    /// Minimum completed pairs before a boundary crossing counts as decisive. Must be >= 1.
    /// Requires --sprt-variant pentanomial (the only variant with a paired_count). `sprt::run`
    /// walks completed pairs in the order they complete (see its module doc) and never even
    /// evaluates a boundary crossing before this many pairs have completed - so a crossing seen
    /// only in the first few pairs is never "remembered" once the minimum is reached; the
    /// decision is always based on the LLR at the pair where the walk actually stops.
    #[arg(long)]
    min_paired_ids: Option<u64>,

    /// Reached-without-a-decision marker: if the sequential walk reaches this many completed
    /// pairs without a boundary crossing, it stops there with an inconclusive verdict (no
    /// truncated-SPRT decision rule) and `reason`/`stopping_reason` note the cap was hit. Pairs
    /// completed after this point, even if present in the input, never affect the verdict, LLR,
    /// or bucket counts. Requires --sprt-variant pentanomial, same as --min-paired-ids.
    #[arg(long)]
    max_paired_ids: Option<u64>,

    /// Require every id to appear in exactly 2 records instead of tolerating a lone id as an
    /// ordinary unpaired sample. Requires --paired-by-id. Extends --sprt-variant pentanomial's
    /// existing unconditional "exactly 2, no exceptions" pairing to wald/trinomial too; a
    /// documented no-op for pentanomial itself, which is already this strict regardless of this
    /// flag.
    #[arg(long)]
    require_complete_pairs: bool,

    /// How a failed trial affects the LLR. See `compare --failure-policy` for the exact
    /// semantics (applies identically here, across all three --sprt-variant choices - a `loss`-
    /// synthesized outcome nets against its pair partner the same way for --sprt-variant
    /// pentanomial as it does for wald/trinomial).
    #[arg(long, value_enum, default_value = "report-only")]
    failure_policy: FailurePolicyArg,

    /// Hard cap on candidate+baseline timeout count. See `compare --max-timeouts` for the exact
    /// semantics (breaching it forces validity=invalid/verdict=inconclusive regardless of the
    /// LLR).
    #[arg(long)]
    max_timeouts: Option<u64>,

    /// Hard cap on candidate+baseline crash count. See `compare --max-timeouts`.
    #[arg(long)]
    max_crashes: Option<u64>,

    /// Hard cap on candidate+baseline invalid-result count. See `compare --max-timeouts`.
    #[arg(long)]
    max_invalid: Option<u64>,

    /// Also write the JSON report to this file.
    #[arg(long)]
    report_json: Option<PathBuf>,

    /// Also write a human-readable Markdown report to this file.
    #[arg(long)]
    report_md: Option<PathBuf>,
}

#[derive(clap::Args)]
struct MatrixArgs {
    /// Legacy: one file per candidate, each measured against the same shared baseline (id/
    /// baseline/candidate/result schema). Candidate names come from each file's stem (e.g.
    /// "prompt_a.jsonl" -> "prompt_a"). At least one of `files`/`--matches` is required.
    #[arg(num_args = 1.., required_unless_present = "matches")]
    files: Vec<PathBuf>,

    /// Head-to-head match data between named competitors (id/a/b/result schema, result is
    /// a_win|b_win|draw). Competitor names come from each record's a/b fields, not the file
    /// name. Repeatable. Use the literal name "baseline" in a/b to connect this data to the
    /// baseline node implied by `files`.
    #[arg(long = "matches", num_args = 1.., required_unless_present = "files")]
    matches: Vec<PathBuf>,

    /// Input format, applied to every file (both legacy files and --matches). Defaults to
    /// sniffing each file's extension.
    #[arg(long, value_enum)]
    format: Option<FormatArg>,

    #[arg(long, default_value_t = 0.95)]
    confidence: f64,

    /// Treat two records sharing an id as one testcase played twice and
    /// combine them into a single net observation. See `compare
    /// --paired-by-id` for the exact semantics; applied independently to
    /// each file.
    #[arg(long)]
    paired_by_id: bool,

    /// Bootstrap resample count for general-graph confidence intervals. Has
    /// no effect when every edge touches "baseline" (star mode uses the
    /// closed-form Wilson interval instead).
    #[arg(long, default_value_t = 2_000)]
    resamples: usize,

    /// Bootstrap RNG seed for general-graph confidence intervals. Defaults
    /// to a fixed seed, so the same input reproduces bit-identical output.
    #[arg(long)]
    seed: Option<u64>,

    /// Bootstrap variant for general-graph confidence intervals. Same
    /// meaning as `compare`'s `--bootstrap-method`; has no effect in star
    /// mode (closed-form Wilson interval, no bootstrap involved).
    #[arg(long, value_enum, default_value = "percentile")]
    bootstrap_method: BootstrapMethodArg,

    /// Also write the JSON report to this file.
    #[arg(long)]
    report_json: Option<PathBuf>,

    /// Also write a human-readable Markdown report to this file.
    #[arg(long)]
    report_md: Option<PathBuf>,
}

#[derive(clap::Args)]
struct PlanArgs {
    /// Same as `matrix`'s `files`/`--matches`/`--format`/`--paired-by-id`/`--resamples`/
    /// `--seed`/`--bootstrap-method` - `plan` runs `matrix` internally and recommends from its
    /// result, so every input flag means exactly what it means there.
    #[arg(num_args = 1.., required_unless_present = "matches")]
    files: Vec<PathBuf>,

    #[arg(long = "matches", num_args = 1.., required_unless_present = "files")]
    matches: Vec<PathBuf>,

    #[arg(long, value_enum)]
    format: Option<FormatArg>,

    #[arg(long, default_value_t = 0.95)]
    confidence: f64,

    #[arg(long)]
    paired_by_id: bool,

    #[arg(long, default_value_t = 2_000)]
    resamples: usize,

    #[arg(long)]
    seed: Option<u64>,

    #[arg(long, value_enum, default_value = "percentile")]
    bootstrap_method: BootstrapMethodArg,

    /// The Elo gap worth being able to detect - recommendations narrow each pair's CI toward
    /// this half-width. Required: there's no sensible default for "how precise do you need this."
    #[arg(long, allow_hyphen_values = true)]
    min_elo: f64,

    /// Also write the JSON report to this file.
    #[arg(long)]
    report_json: Option<PathBuf>,

    /// Also write a human-readable Markdown report to this file.
    #[arg(long)]
    report_md: Option<PathBuf>,
}

#[derive(clap::Args)]
struct PowerArgs {
    #[arg(
        long,
        value_enum,
        required_unless_present = "sprt",
        conflicts_with = "sprt"
    )]
    metric: Option<PowerMetricArg>,

    /// The pass bar - identical meaning to `compare --min-effect`/`--pass-above` (the CI lower
    /// bound a real run must clear to pass).
    #[arg(
        long,
        allow_hyphen_values = true,
        required_unless_present = "sprt",
        conflicts_with = "sprt"
    )]
    min_effect: Option<f64>,

    /// The true effect being powered for - must be strictly greater than --min-effect. Evaluating
    /// power with the true effect equal to the pass bar only recovers the interval's own
    /// miscoverage at that boundary (~1-confidence), not a number that climbs toward
    /// --target-power with more trials - see `docs/metrics.md`'s `power` section.
    #[arg(
        long,
        allow_hyphen_values = true,
        required_unless_present = "sprt",
        conflicts_with = "sprt"
    )]
    assume_effect: Option<f64>,

    #[arg(long, default_value_t = 0.95, conflicts_with = "sprt")]
    confidence: f64,

    /// Target probability of reaching a passing verdict, assuming the true effect is exactly
    /// --assume-effect.
    #[arg(long, default_value_t = 0.80, conflicts_with = "sprt")]
    target_power: f64,

    /// Confidence interval method - same meaning as `compare --ci-method`; `exact`/`jeffreys` are
    /// only valid for winrate/sign-test, same restriction as `compare`.
    #[arg(long, value_enum, default_value = "wilson", conflicts_with = "sprt")]
    ci_method: CiMethodArg,

    /// Accepted but does not change the estimate - see `docs/metrics.md`'s `power` section for
    /// why the actual variance reduction from pairing can't be predicted without real data. Adds
    /// a caveat to the report's `notes` instead. For `--metric mean-diff --pilot FILE`, this DOES
    /// change the estimate - same-id records in the pilot data are netted before estimating a
    /// standard deviation, same as `compare --paired-by-id` itself.
    #[arg(long, conflicts_with = "sprt")]
    paired_by_id: bool,

    /// Assumed standard deviation of the paired (candidate - baseline) difference, for --metric
    /// mean-diff (mean-diff has no closed-form CI to search a hypothetical n against, so power
    /// analysis needs this assumption from the caller instead). Mutually exclusive with --pilot;
    /// exactly one is required when --metric mean-diff, neither is accepted otherwise.
    #[arg(long, conflicts_with_all = ["pilot", "sprt"])]
    assume_sd: Option<f64>,

    /// Pilot data file (same JSONL/CSV format as `compare`'s input) to estimate --metric
    /// mean-diff's standard deviation from, instead of supplying --assume-sd directly. Use "-" to
    /// read from stdin.
    #[arg(long, conflicts_with_all = ["assume_sd", "sprt"])]
    pilot: Option<PathBuf>,

    /// Format override for --pilot; same sniffing rules as `compare --format`.
    #[arg(long, value_enum, conflicts_with = "sprt")]
    pilot_format: Option<FormatArg>,

    /// Estimate SPRT's expected sample size (Wald's ASN approximation) instead of a
    /// CI-crossing-probability search. Requires --elo0/--elo1; mutually exclusive with
    /// --metric/--min-effect/--assume-effect/--confidence/--target-power/--ci-method/
    /// --paired-by-id, since Wald's alpha/beta already fix the guaranteed error rates - there's no
    /// target power to search a sample size for.
    #[arg(long, requires_all = ["elo0", "elo1"])]
    sprt: bool,

    /// H0 for --sprt: the candidate is at most this many logistic-Elo points stronger. Same
    /// meaning as `veridict sprt --elo0`.
    #[arg(long, allow_hyphen_values = true, requires = "sprt")]
    elo0: Option<f64>,

    /// H1 for --sprt: the candidate is at least this many logistic-Elo points stronger. Same
    /// meaning as `veridict sprt --elo1`.
    #[arg(long, allow_hyphen_values = true, requires = "sprt")]
    elo1: Option<f64>,

    /// False-positive rate for --sprt. Same meaning and default as `veridict sprt --alpha`.
    #[arg(long, default_value_t = 0.05)]
    alpha: f64,

    /// False-negative rate for --sprt. Same meaning and default as `veridict sprt --beta`.
    #[arg(long, default_value_t = 0.05)]
    beta: f64,

    /// For --sprt: also report the Monte Carlo probability that a real `veridict sprt` run
    /// still hasn't reached a decision after this many decisive trials, evaluated at the
    /// realistic worst-case true strength (halfway between --elo0/--elo1, not either endpoint).
    /// A planning number for choosing the next gate's trial budget/cutoff - it does not change
    /// how a real run itself should be stopped (alpha/beta already fully determine that).
    #[arg(long, requires = "sprt")]
    horizon: Option<u64>,

    /// Also write the JSON report to this file.
    #[arg(long)]
    report_json: Option<PathBuf>,

    /// Also write a human-readable Markdown report to this file.
    #[arg(long)]
    report_md: Option<PathBuf>,
}

#[derive(clap::Args)]
struct VerifyRunArgs {
    /// Path to the manifest.toml declaring this run's expected identifiers/hashes/schedule.
    manifest: PathBuf,

    /// Path to the games.jsonl (or CSV) of per-record observations. Use "-" to read from stdin.
    games: PathBuf,

    /// Input format for `games`. Defaults to sniffing the file extension (.csv vs everything
    /// else); pass explicitly when reading CSV from stdin.
    #[arg(long, value_enum)]
    format: Option<FormatArg>,

    /// Also write the JSON report to this file.
    #[arg(long)]
    report_json: Option<PathBuf>,

    /// Also write a human-readable Markdown report to this file.
    #[arg(long)]
    report_md: Option<PathBuf>,
}

#[derive(clap::Args)]
struct TimeSensitiveArgs {
    /// Input file. Use "-" to read from stdin.
    input: PathBuf,

    /// Input format. Defaults to sniffing the file extension (.csv vs
    /// everything else); pass explicitly when reading CSV from stdin.
    #[arg(long, value_enum)]
    format: Option<FormatArg>,

    /// H0: the candidate's decisive-observation-conditional success probability. Must satisfy
    /// 0 < p0 < p1 < 1.
    #[arg(long)]
    p0: f64,

    /// H1: the candidate's decisive-observation-conditional success probability.
    #[arg(long)]
    p1: f64,

    /// False-positive rate: probability of crossing the wealth threshold when H0 is true.
    #[arg(long, default_value_t = 0.05)]
    alpha: f64,

    /// Betting policy. `gro`: patient, classical anytime-valid baseline (bets p1 every trial,
    /// ignores the reward schedule). `bellman`: numerical grid approximation of the time-
    /// sensitive-optimal policy for the given reward schedule. `edo`: closed-form stationary
    /// approximation, valid only for `--reward exponential`.
    #[arg(long, value_enum)]
    policy: TimeSensitivePolicyArg,

    /// Reward schedule shape. Exactly one of --reward/--reward-schedule is required.
    #[arg(long, value_enum, conflicts_with = "reward_schedule")]
    reward: Option<RewardKindArg>,

    /// Deadline for --reward hard-deadline: reward is 1 for a rejection at or before this trial,
    /// 0 after.
    #[arg(long, requires = "reward")]
    deadline: Option<u64>,

    /// Time scale for --reward exponential: reward decays as exp(-t/time_scale).
    #[arg(long, requires = "reward")]
    time_scale: Option<f64>,

    /// Finite backward-induction truncation horizon for --reward exponential.
    #[arg(long, requires = "reward")]
    horizon: Option<u64>,

    /// Path to a custom reward-schedule JSON file: {"rewards": [{"until": N, "value": V}, ...,
    /// {"after": N, "value": 0.0}]}. Exactly one of --reward/--reward-schedule is required.
    #[arg(long, conflicts_with_all = ["reward", "deadline", "time_scale", "horizon"])]
    reward_schedule: Option<PathBuf>,

    /// Action-grid resolution. Only meaningful for --policy bellman; still validated (>= 2) for
    /// every policy.
    #[arg(long, default_value_t = 101)]
    action_grid_size: usize,

    /// Log-wealth grid resolution.
    #[arg(long, default_value_t = 200)]
    wealth_grid_size: usize,

    /// How a failed trial affects the wealth process. See `compare --failure-policy` for the
    /// exact semantics.
    #[arg(long, value_enum, default_value = "report-only")]
    failure_policy: FailurePolicyArg,

    /// Hard cap on candidate+baseline timeout count. See `compare --max-timeouts`.
    #[arg(long)]
    max_timeouts: Option<u64>,

    /// Hard cap on candidate+baseline crash count. See `compare --max-timeouts`.
    #[arg(long)]
    max_crashes: Option<u64>,

    /// Hard cap on candidate+baseline invalid-result count. See `compare --max-timeouts`.
    #[arg(long)]
    max_invalid: Option<u64>,

    /// Also write the JSON report to this file.
    #[arg(long)]
    report_json: Option<PathBuf>,

    /// Also write a human-readable Markdown report to this file.
    #[arg(long)]
    report_md: Option<PathBuf>,
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
enum PowerMetricArg {
    Winrate,
    SignTest,
    Elo,
    MeanDiff,
    /// Not implemented this round - `PowerMetric::new` rejects it with
    /// `VeridictError::PowerUnsupportedForRelativeDiff`. Offered as a real CLI choice (rather than
    /// simply omitted the way `quantile-diff` is) so `power --metric relative-diff` fails with a
    /// clear, domain-specific error instead of clap's generic "invalid value" usage message - see
    /// docs/research-map.md for what's deferred and why.
    RelativeDiff,
}

impl From<PowerMetricArg> for MetricKind {
    fn from(m: PowerMetricArg) -> Self {
        match m {
            PowerMetricArg::Winrate => MetricKind::WinRate,
            PowerMetricArg::SignTest => MetricKind::SignTest,
            PowerMetricArg::Elo => MetricKind::Elo,
            PowerMetricArg::MeanDiff => MetricKind::MeanDiff,
            PowerMetricArg::RelativeDiff => MetricKind::RelativeDiff,
        }
    }
}

/// A label for `AssumeSdOnlyForMeanDiff`'s error message; matches the CLI's `--metric` spelling.
fn power_metric_arg_label(m: PowerMetricArg) -> &'static str {
    match m {
        PowerMetricArg::Winrate => "winrate",
        PowerMetricArg::SignTest => "sign-test",
        PowerMetricArg::Elo => "elo",
        PowerMetricArg::MeanDiff => "mean-diff",
        PowerMetricArg::RelativeDiff => "relative-diff",
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum MetricArg {
    Winrate,
    MeanDiff,
    SignTest,
    Elo,
    QuantileDiff,
    RelativeDiff,
}

/// One comma-delimited `--min-effect` entry: either a bare number (the run's default) or
/// `metric=value` (an override for just that metric). See `CompareArgs::min_effect`'s doc.
#[derive(Clone, Copy)]
enum MinEffectEntry {
    Default(f64),
    Metric(MetricArg, f64),
}

fn parse_min_effect_entry(s: &str) -> Result<MinEffectEntry, String> {
    match s.split_once('=') {
        None => s
            .parse::<f64>()
            .map(MinEffectEntry::Default)
            .map_err(|e| format!("invalid --min-effect value '{s}': {e}")),
        Some((name, value)) => {
            let metric = <MetricArg as ValueEnum>::from_str(name, false).map_err(|_| {
                format!(
                    "invalid --min-effect metric '{name}' (expected one of: {})",
                    MetricArg::value_variants()
                        .iter()
                        .map(|v| v.to_possible_value().unwrap().get_name().to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })?;
            let value: f64 = value
                .parse()
                .map_err(|e| format!("invalid --min-effect value '{value}': {e}"))?;
            Ok(MinEffectEntry::Metric(metric, value))
        }
    }
}

fn metric_arg_label(m: MetricArg) -> String {
    m.to_possible_value().unwrap().get_name().to_string()
}

impl From<MetricArg> for MetricKind {
    fn from(m: MetricArg) -> Self {
        match m {
            MetricArg::Winrate => MetricKind::WinRate,
            MetricArg::MeanDiff => MetricKind::MeanDiff,
            MetricArg::SignTest => MetricKind::SignTest,
            MetricArg::Elo => MetricKind::Elo,
            MetricArg::QuantileDiff => MetricKind::QuantileDiff,
            MetricArg::RelativeDiff => MetricKind::RelativeDiff,
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum FormatArg {
    Jsonl,
    Csv,
}

#[derive(Clone, Copy, ValueEnum)]
enum CorrectionArg {
    None,
    Bonferroni,
    Holm,
}

impl From<CorrectionArg> for Correction {
    fn from(c: CorrectionArg) -> Self {
        match c {
            CorrectionArg::None => Correction::None,
            CorrectionArg::Bonferroni => Correction::Bonferroni,
            CorrectionArg::Holm => Correction::Holm,
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum CiMethodArg {
    Wilson,
    Exact,
    Jeffreys,
}

impl From<CiMethodArg> for CiMethod {
    fn from(m: CiMethodArg) -> Self {
        match m {
            CiMethodArg::Wilson => CiMethod::Wilson,
            CiMethodArg::Exact => CiMethod::Exact,
            CiMethodArg::Jeffreys => CiMethod::Jeffreys,
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum BootstrapMethodArg {
    Percentile,
    Bca,
    Basic,
}

impl From<BootstrapMethodArg> for BootstrapMethod {
    fn from(m: BootstrapMethodArg) -> Self {
        match m {
            BootstrapMethodArg::Percentile => BootstrapMethod::Percentile,
            BootstrapMethodArg::Bca => BootstrapMethod::Bca,
            BootstrapMethodArg::Basic => BootstrapMethod::Basic,
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum FailurePolicyArg {
    ReportOnly,
    Exclude,
    Loss,
}

impl From<FailurePolicyArg> for FailurePolicy {
    fn from(m: FailurePolicyArg) -> Self {
        match m {
            FailurePolicyArg::ReportOnly => FailurePolicy::ReportOnly,
            FailurePolicyArg::Exclude => FailurePolicy::Exclude,
            FailurePolicyArg::Loss => FailurePolicy::Loss,
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum SprtVariantArg {
    Wald,
    Trinomial,
    Pentanomial,
}

#[derive(Clone, Copy, ValueEnum)]
enum TimeSensitivePolicyArg {
    Bellman,
    Edo,
    Gro,
}

impl From<TimeSensitivePolicyArg> for time_sensitive::TimeSensitivePolicyKind {
    fn from(p: TimeSensitivePolicyArg) -> Self {
        match p {
            TimeSensitivePolicyArg::Bellman => time_sensitive::TimeSensitivePolicyKind::BellmanGrid,
            TimeSensitivePolicyArg::Edo => time_sensitive::TimeSensitivePolicyKind::Edo,
            TimeSensitivePolicyArg::Gro => time_sensitive::TimeSensitivePolicyKind::Gro,
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum RewardKindArg {
    HardDeadline,
    Exponential,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli.command) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(3)
        }
    }
}

/// Builds one `Thresholds` per requested metric, parallel to `metrics` (see `compare_many`'s
/// `thresholds: &[Thresholds]` parameter). `--pass-above`/`--fail-below` (when both given) fully
/// override `--min-effect` and broadcast to every metric, same as before per-metric overrides
/// existed. Otherwise each metric gets its own `Metric` override from `--min-effect` if one was
/// given, else the bare `Default` value if given, else 0.0 (today's implicit default).
fn resolve_thresholds(
    metrics: &[MetricArg],
    pass_above: Option<f64>,
    fail_below: Option<f64>,
    min_effect: &[MinEffectEntry],
) -> Result<Vec<Thresholds>, VeridictError> {
    let mut default: Option<f64> = None;
    let mut overrides: Vec<(MetricArg, f64)> = Vec::new();
    for entry in min_effect {
        match *entry {
            MinEffectEntry::Default(v) => {
                if let Some(existing) = default {
                    return Err(VeridictError::InvalidThreshold(format!(
                        "--min-effect gives more than one default value ({existing} and {v}); \
                         only one bare number is allowed per run"
                    )));
                }
                default = Some(v);
            }
            MinEffectEntry::Metric(m, v) => {
                let label = metric_arg_label(m);
                if overrides
                    .iter()
                    .any(|&(om, _)| MetricKind::from(om) == MetricKind::from(m))
                {
                    return Err(VeridictError::InvalidThreshold(format!(
                        "--min-effect specifies metric '{label}' more than once"
                    )));
                }
                if !metrics
                    .iter()
                    .any(|&rm| MetricKind::from(rm) == MetricKind::from(m))
                {
                    return Err(VeridictError::InvalidThreshold(format!(
                        "--min-effect '{label}=...' names a metric that wasn't requested via \
                         --metric"
                    )));
                }
                overrides.push((m, v));
            }
        }
    }

    if let (Some(pass_above), Some(fail_below)) = (pass_above, fail_below) {
        return metrics
            .iter()
            .map(|_| Thresholds::new(pass_above, fail_below))
            .collect();
    }

    metrics
        .iter()
        .map(|&m| {
            let v = overrides
                .iter()
                .find(|&&(om, _)| MetricKind::from(om) == MetricKind::from(m))
                .map(|&(_, v)| v)
                .unwrap_or_else(|| default.unwrap_or(0.0));
            Thresholds::symmetric(v).map_err(|e| {
                VeridictError::InvalidThreshold(format!(
                    "--min-effect for metric '{}': {e}",
                    metric_arg_label(m)
                ))
            })
        })
        .collect()
}

/// Which scale a metric's raw effect number lives on - see `CompareArgs::min_effect`'s doc for
/// the same three-way split. Used only to warn when a bare `--min-effect` default is silently
/// shared across metrics whose numbers aren't comparable.
fn min_effect_unit_family(m: MetricArg) -> &'static str {
    match m {
        MetricArg::Winrate | MetricArg::SignTest => "win-rate margin off 0.5",
        MetricArg::RelativeDiff => "relative ratio",
        MetricArg::MeanDiff | MetricArg::QuantileDiff | MetricArg::Elo => "raw units",
    }
}

/// Warns on stderr (never blocks the run - this is advisory, like the `--correction` deprecation
/// warning it's modeled on) when a bare `--min-effect` default silently applies to two or more
/// requested metrics whose unit families differ (see `min_effect_unit_family`) - e.g. `--min-effect
/// 0.01` meaning "barely above a coin flip" for `sign-test` and "a real 1% score change" for
/// `relative-diff` at the same time. Silent when every metric has its own `metric=value` override
/// (nothing is actually shared), when only one metric is requested (nothing to compare against),
/// or when `--pass-above`/`--fail-below` were given (they bypass `--min-effect` entirely).
fn warn_on_mixed_min_effect_units(
    metrics: &[MetricArg],
    pass_above: Option<f64>,
    fail_below: Option<f64>,
    min_effect: &[MinEffectEntry],
) {
    if pass_above.is_some() || fail_below.is_some() {
        return;
    }
    let default = min_effect.iter().find_map(|e| match *e {
        MinEffectEntry::Default(v) => Some(v),
        MinEffectEntry::Metric(..) => None,
    });
    let Some(default) = default else {
        return;
    };
    let overridden: Vec<MetricArg> = min_effect
        .iter()
        .filter_map(|e| match *e {
            MinEffectEntry::Metric(m, _) => Some(m),
            MinEffectEntry::Default(_) => None,
        })
        .collect();
    let defaulted: Vec<MetricArg> = metrics
        .iter()
        .copied()
        .filter(|m| {
            !overridden
                .iter()
                .any(|&om| MetricKind::from(om) == MetricKind::from(*m))
        })
        .collect();

    let mut families: Vec<&'static str> = defaulted
        .iter()
        .map(|&m| min_effect_unit_family(m))
        .collect();
    families.sort_unstable();
    families.dedup();
    if families.len() < 2 {
        return;
    }

    let named: Vec<String> = defaulted
        .iter()
        .map(|&m| format!("{} ({})", metric_arg_label(m), min_effect_unit_family(m)))
        .collect();
    eprintln!(
        "warning: --min-effect {default} applies the same threshold to {} without a per-metric \
         override, but these are not the same scale. Consider e.g. --min-effect {}.",
        named.join(" and "),
        defaulted
            .iter()
            .map(|&m| format!("{}={default}", metric_arg_label(m)))
            .collect::<Vec<_>>()
            .join(",")
    );
}

fn run(command: Command) -> Result<ExitCode, VeridictError> {
    match command {
        Command::Compare(args) => run_compare(args),
        Command::Sprt(args) => run_sprt(args),
        Command::Matrix(args) => run_matrix(args),
        Command::Plan(args) => run_plan(args),
        Command::Power(args) => run_power(args),
        Command::VerifyRun(args) => run_verify_run(args),
        Command::TimeSensitive(args) => run_time_sensitive(args),
    }
}

fn run_compare(args: CompareArgs) -> Result<ExitCode, VeridictError> {
    let thresholds = resolve_thresholds(
        &args.metrics,
        args.pass_above,
        args.fail_below,
        &args.min_effect,
    )?;
    warn_on_mixed_min_effect_units(
        &args.metrics,
        args.pass_above,
        args.fail_below,
        &args.min_effect,
    );

    let format = resolve_format(&args.input, args.format);
    let records = read_records(&args.input, format)?;
    let seed = args.seed.unwrap_or(DEFAULT_SEED);
    let ci_method: CiMethod = args.ci_method.into();
    let bootstrap_method: BootstrapMethod = args.bootstrap_method.into();
    let failure_policy: FailurePolicy = args.failure_policy.into();
    // `MetricConfig::new` itself treats `quantile` as silently unused by non-`QuantileDiff`
    // metrics (so it can be shared across e.g. `--metric mean-diff --metric quantile-diff` in one
    // run) - but a `--quantile` passed without *any* `--metric quantile-diff` requested at all is
    // a config error at this, the CLI boundary, not a silent no-op: the project's own "never
    // silently drop/fall back" rule (see `IncompatibleCiMethod`/`IncompatibleFailurePolicy`).
    if args.quantile.is_some()
        && !args
            .metrics
            .iter()
            .any(|m| matches!(m, MetricArg::QuantileDiff))
    {
        return Err(VeridictError::QuantileRequiresQuantileDiffMetric);
    }
    let metrics: Vec<MetricConfig> = args
        .metrics
        .into_iter()
        .map(|m| {
            MetricConfig::new(
                m.into(),
                ci_method,
                bootstrap_method,
                failure_policy,
                args.quantile,
            )
        })
        .collect::<Result<_, _>>()?;

    let correction: Correction = match (args.claim_correction, args.correction) {
        (Some(c), None) => c.into(),
        (None, Some(c)) => {
            eprintln!(
                "warning: --correction is deprecated; use --claim-correction instead \
                 (--correction will be removed in a future release)"
            );
            c.into()
        }
        (None, None) => Correction::None,
        (Some(_), Some(_)) => unreachable!("clap's conflicts_with rejects passing both"),
    };
    let caps = FailureCaps {
        max_timeouts: args.max_timeouts,
        max_crashes: args.max_crashes,
        max_invalid: args.max_invalid,
    };
    // Failure caps run first so an invalid report's verdict is already forced to Inconclusive
    // before claim correction inspects it - an invalid report must never count as a legitimate
    // statistical claim (see correction::apply_correction's doc).
    let (verdict, json, markdown) = if let [only] = metrics[..] {
        let mut report = veridict::compare_one(
            records,
            only,
            args.confidence,
            &thresholds[0],
            args.resamples,
            seed,
            args.paired_by_id,
            args.cluster_by_id,
        )?;
        verdict::apply_failure_caps(&mut report, &caps);
        veridict::correction::apply_correction(
            std::slice::from_mut(&mut report),
            &metrics,
            correction,
            args.confidence,
        )?;
        (
            report.verdict,
            report.to_json_pretty(),
            report.to_markdown(),
        )
    } else {
        let mut multi = veridict::compare_many(
            records,
            &metrics,
            args.confidence,
            &thresholds,
            args.resamples,
            seed,
            args.paired_by_id,
            args.cluster_by_id,
        )?;
        verdict::apply_failure_caps_to_multi(&mut multi, &caps);
        veridict::correction::apply_correction_to_multi(
            &mut multi,
            &metrics,
            correction,
            args.confidence,
        )?;
        (multi.verdict, multi.to_json_pretty(), multi.to_markdown())
    };

    println!("{json}");
    write_reports(&json, &markdown, &args.report_json, &args.report_md)?;
    Ok(exit_code_for(verdict))
}

fn run_sprt(args: SprtArgs) -> Result<ExitCode, VeridictError> {
    let (elo0, elo1, variant) = resolve_sprt_hypotheses(&args)?;
    let config = SprtConfig::new(elo0, elo1, args.alpha, args.beta)?;
    let format = resolve_format(&args.input, args.format);
    let records = read_records(&args.input, format)?;
    let failure_policy: FailurePolicy = args.failure_policy.into();

    let mut report = veridict::sprt::run(
        records,
        &config,
        variant,
        args.paired_by_id,
        failure_policy,
        args.require_complete_pairs,
        args.min_paired_ids,
        args.max_paired_ids,
    )?;
    let caps = FailureCaps {
        max_timeouts: args.max_timeouts,
        max_crashes: args.max_crashes,
        max_invalid: args.max_invalid,
    };
    veridict::sprt::apply_failure_caps(&mut report, &caps);
    let json = report.to_json_pretty();
    let markdown = report.to_markdown();

    println!("{json}");
    write_reports(&json, &markdown, &args.report_json, &args.report_md)?;
    Ok(exit_code_for(report.verdict))
}

/// Picks the (elo0, elo1) pair matching `--sprt-variant`, and rejects the
/// *other* variant's flags being set too - per AGENTS.md's "never silently
/// ignore invalid data", a wald run given `--belo0` (or vice versa) is a
/// user mistake worth a clear error, not a silently-dropped flag.
fn resolve_sprt_hypotheses(args: &SprtArgs) -> Result<(f64, f64, SprtVariant), VeridictError> {
    if args.require_complete_pairs && !args.paired_by_id {
        return Err(VeridictError::InvalidThreshold(
            "--require-complete-pairs requires --paired-by-id".to_string(),
        ));
    }
    if (args.min_paired_ids.is_some() || args.max_paired_ids.is_some())
        && !matches!(args.sprt_variant, SprtVariantArg::Pentanomial)
    {
        return Err(VeridictError::InvalidThreshold(
            "--min-paired-ids/--max-paired-ids require --sprt-variant pentanomial (the only \
             variant with a paired_count)"
                .to_string(),
        ));
    }
    if let Some(min) = args.min_paired_ids
        && min < 1
    {
        return Err(VeridictError::InvalidThreshold(
            "--min-paired-ids must be >= 1".to_string(),
        ));
    }
    if let (Some(min), Some(max)) = (args.min_paired_ids, args.max_paired_ids)
        && min > max
    {
        return Err(VeridictError::InvalidThreshold(format!(
            "--min-paired-ids ({min}) must be <= --max-paired-ids ({max})"
        )));
    }

    let wald_flags_given = args.elo0.is_some() || args.elo1.is_some();
    let trinomial_flags_given = args.belo0.is_some() || args.belo1.is_some();
    match args.sprt_variant {
        SprtVariantArg::Wald => {
            if trinomial_flags_given {
                return Err(VeridictError::InvalidThreshold(
                    "--belo0/--belo1 are only used with --sprt-variant trinomial; pass --elo0/--elo1 for the default wald variant".to_string(),
                ));
            }
            match (args.elo0, args.elo1) {
                (Some(e0), Some(e1)) => Ok((e0, e1, SprtVariant::Wald)),
                _ => Err(VeridictError::InvalidThreshold(
                    "--elo0 and --elo1 are required for --sprt-variant wald (the default)"
                        .to_string(),
                )),
            }
        }
        SprtVariantArg::Trinomial => {
            if wald_flags_given {
                return Err(VeridictError::InvalidThreshold(
                    "--elo0/--elo1 are only used with --sprt-variant wald; pass --belo0/--belo1 for --sprt-variant trinomial".to_string(),
                ));
            }
            match (args.belo0, args.belo1) {
                (Some(b0), Some(b1)) => Ok((b0, b1, SprtVariant::Trinomial)),
                _ => Err(VeridictError::InvalidThreshold(
                    "--belo0 and --belo1 are required for --sprt-variant trinomial".to_string(),
                )),
            }
        }
        // Pentanomial shares wald's logistic-Elo scale (no drawelo-style nuisance parameter
        // exists in this model, see stats::pentanomial_sprt's doc), so it takes the same
        // --elo0/--elo1 branch shape as wald, not trinomial's --belo0/--belo1.
        SprtVariantArg::Pentanomial => {
            if trinomial_flags_given {
                return Err(VeridictError::InvalidThreshold(
                    "--belo0/--belo1 are only used with --sprt-variant trinomial; pass --elo0/--elo1 for --sprt-variant pentanomial".to_string(),
                ));
            }
            if !args.paired_by_id {
                return Err(VeridictError::InvalidThreshold(
                    "--sprt-variant pentanomial requires --paired-by-id".to_string(),
                ));
            }
            match (args.elo0, args.elo1) {
                (Some(e0), Some(e1)) => Ok((e0, e1, SprtVariant::Pentanomial)),
                _ => Err(VeridictError::InvalidThreshold(
                    "--elo0 and --elo1 are required for --sprt-variant pentanomial".to_string(),
                )),
            }
        }
    }
}

/// Resolves `power --metric mean-diff`'s assumed standard deviation from exactly one of
/// `--assume-sd`/`--pilot` - only ever called once the caller has already confirmed `--metric
/// mean-diff` (see `run_power`), mirroring `resolve_sprt_hypotheses`'s own shape: a runtime check
/// rather than fought through clap's declarative attributes, since "exactly one of two optional
/// flags, required only for one enum value" isn't cleanly expressible there. Returns `(sd, source,
/// extra_notes)` - `extra_notes` carries pilot-specific caveats (tiny-sample, small-pilot-t-
/// correction) `power::estimate_trials` has no way to know about, since it never touches a file.
fn resolve_assume_sd(args: &PowerArgs) -> Result<(f64, &'static str, Vec<String>), VeridictError> {
    match (args.assume_sd, &args.pilot) {
        (Some(sd), None) => {
            if !sd.is_finite() || sd <= 0.0 {
                return Err(VeridictError::InvalidAssumeSd(sd));
            }
            Ok((sd, "assume-sd", Vec::new()))
        }
        (None, Some(pilot_path)) => {
            let format = resolve_format(pilot_path, args.pilot_format);
            let records = read_records(pilot_path, format)?;
            let diffs = power::pilot_diffs(records, args.paired_by_id)?;
            if diffs.len() < 2 {
                return Err(VeridictError::InsufficientPilotData {
                    path: pilot_path.display().to_string(),
                    count: diffs.len(),
                });
            }
            let sd = sample_sd(&diffs);
            if sd == 0.0 {
                return Err(VeridictError::ZeroVariancePilotData {
                    path: pilot_path.display().to_string(),
                    count: diffs.len(),
                });
            }
            let mut notes = Vec::new();
            if diffs.len() < 30 {
                notes.push(format!(
                    "--pilot '{}' has only {} usable paired diff(s) - below the conventional \
                     30-observation threshold for the sample standard deviation itself to be a \
                     reliable estimate; treat assume_sd as a rougher guess than usual. Also, \
                     normal quantiles (used here) slightly underestimate the required n relative \
                     to a small-sample t-distribution correction, which isn't applied.",
                    pilot_path.display(),
                    diffs.len(),
                ));
            }
            Ok((sd, "pilot", notes))
        }
        (None, None) => Err(VeridictError::MeanDiffPowerRequiresSd),
        (Some(_), Some(_)) => {
            unreachable!("clap's conflicts_with_all already rejects --assume-sd with --pilot")
        }
    }
}

fn run_matrix(args: MatrixArgs) -> Result<ExitCode, VeridictError> {
    let format = args.format;
    let mut seen_names = std::collections::HashSet::new();
    // Lazy: each `(name, records-iterator)` pair is only produced (and each
    // file only opened) as `matrix::run` pulls it, one candidate at a time -
    // see `matrix::run`'s own doc comment for why this bounds peak memory by
    // the largest single file rather than the sum of all of them. The
    // duplicate-name check still runs per-file, in the same interleaved
    // order as before, not hoisted into a separate upfront pass.
    let named_records = args.files.iter().map(move |path| {
        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("candidate")
            .to_string();
        if !seen_names.insert(name.clone()) {
            return Err(VeridictError::InvalidThreshold(format!(
                "duplicate candidate name '{name}' from input file stems; rename one of the files"
            )));
        }
        let fmt = resolve_format(path, format);
        let records = read_records(path, fmt)?;
        Ok((name, records))
    });
    let match_records = args
        .matches
        .iter()
        .map(move |path| read_match_records(path, resolve_format(path, format)));

    let matrix = matrix::run(
        named_records,
        match_records,
        args.confidence,
        args.paired_by_id,
        args.resamples,
        args.seed.unwrap_or(DEFAULT_SEED),
        args.bootstrap_method.into(),
    )?;
    let json = matrix.to_json_pretty();
    let markdown = matrix.to_markdown();

    println!("{json}");
    write_reports(&json, &markdown, &args.report_json, &args.report_md)?;
    Ok(ExitCode::from(0))
}

fn run_plan(args: PlanArgs) -> Result<ExitCode, VeridictError> {
    let format = args.format;
    let mut seen_names = std::collections::HashSet::new();
    let named_records = args.files.iter().map(move |path| {
        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("candidate")
            .to_string();
        if !seen_names.insert(name.clone()) {
            return Err(VeridictError::InvalidThreshold(format!(
                "duplicate candidate name '{name}' from input file stems; rename one of the files"
            )));
        }
        let fmt = resolve_format(path, format);
        let records = read_records(path, fmt)?;
        Ok((name, records))
    });
    let match_records = args
        .matches
        .iter()
        .map(move |path| read_match_records(path, resolve_format(path, format)));

    let plan = veridict::plan::run(
        named_records,
        match_records,
        args.confidence,
        args.paired_by_id,
        args.resamples,
        args.seed.unwrap_or(DEFAULT_SEED),
        args.bootstrap_method.into(),
        args.min_elo,
    )?;
    let json = plan.to_json_pretty();
    let markdown = plan.to_markdown();

    println!("{json}");
    write_reports(&json, &markdown, &args.report_json, &args.report_md)?;
    Ok(ExitCode::from(0))
}

fn run_power(args: PowerArgs) -> Result<ExitCode, VeridictError> {
    let (json, markdown) = if args.sprt {
        // clap's `requires_all = ["elo0", "elo1"]` on --sprt guarantees both are `Some` here.
        let report = power::estimate_sprt_expected_trials(
            args.elo0.expect("clap requires elo0 with --sprt"),
            args.elo1.expect("clap requires elo1 with --sprt"),
            args.alpha,
            args.beta,
            args.horizon,
        )?;
        (report.to_json_pretty(), report.to_markdown())
    } else {
        // clap's `required_unless_present = "sprt"` on these three guarantees `Some` here.
        let metric_arg = args.metric.expect("clap requires --metric without --sprt");
        if metric_arg != PowerMetricArg::MeanDiff
            && (args.assume_sd.is_some() || args.pilot.is_some())
        {
            return Err(VeridictError::AssumeSdOnlyForMeanDiff {
                metric: power_metric_arg_label(metric_arg),
            });
        }

        let (metric, extra_notes) = if metric_arg == PowerMetricArg::MeanDiff {
            let (assume_sd, sd_source, extra_notes) = resolve_assume_sd(&args)?;
            (
                power::PowerMetric::MeanDiff {
                    assume_sd,
                    sd_source,
                },
                extra_notes,
            )
        } else {
            let ci_method: CiMethod = args.ci_method.into();
            (
                power::PowerMetric::new(metric_arg.into(), ci_method)?,
                Vec::new(),
            )
        };

        let mut report = power::estimate_trials(
            metric,
            args.min_effect
                .expect("clap requires --min-effect without --sprt"),
            args.assume_effect
                .expect("clap requires --assume-effect without --sprt"),
            args.confidence,
            args.target_power,
            args.paired_by_id,
        )?;
        report.notes.extend(extra_notes);
        (report.to_json_pretty(), report.to_markdown())
    };

    println!("{json}");
    write_reports(&json, &markdown, &args.report_json, &args.report_md)?;
    Ok(ExitCode::from(0))
}

/// Unlike every other subcommand, `verify_run::run` needs the whole record set at once (pairing/
/// ordering/dedup checks are inherently cross-record - see `verify_run`'s own doc), so records
/// are collected into a `Vec` here rather than streamed the way `read_records`/`read_match_records`
/// are for `compare`/`sprt`/`matrix`.
fn run_verify_run(args: VerifyRunArgs) -> Result<ExitCode, VeridictError> {
    let manifest_text =
        std::fs::read_to_string(&args.manifest).map_err(|source| VeridictError::Io {
            path: args.manifest.display().to_string(),
            source,
        })?;
    let manifest =
        veridict::verify_run::parse_manifest(&args.manifest.display().to_string(), &manifest_text)?;

    let format = resolve_format(&args.games, args.format);
    let records: Vec<_> =
        read_verify_run_records(&args.games, format)?.collect::<Result<_, _>>()?;

    let report = veridict::verify_run::run(&manifest, records)?;
    let json = report.to_json_pretty();
    let markdown = report.to_markdown();

    println!("{json}");
    write_reports(&json, &markdown, &args.report_json, &args.report_md)?;
    Ok(match report.validity {
        veridict::Validity::Valid => ExitCode::from(0),
        veridict::Validity::Invalid => ExitCode::from(1),
    })
}

fn run_time_sensitive(args: TimeSensitiveArgs) -> Result<ExitCode, VeridictError> {
    let reward = resolve_time_sensitive_reward(&args)?;
    let config = time_sensitive::TimeSensitiveConfig {
        hypotheses: time_sensitive::BernoulliHypotheses {
            p0: args.p0,
            p1: args.p1,
        },
        alpha: args.alpha,
        reward,
        policy: args.policy.into(),
        action_grid_size: args.action_grid_size,
        wealth_grid_size: args.wealth_grid_size,
    };
    let format = resolve_format(&args.input, args.format);
    let records = read_records(&args.input, format)?;
    let failure_policy: FailurePolicy = args.failure_policy.into();

    let mut report = time_sensitive::run(records, config, failure_policy)?;
    let caps = FailureCaps {
        max_timeouts: args.max_timeouts,
        max_crashes: args.max_crashes,
        max_invalid: args.max_invalid,
    };
    time_sensitive::apply_failure_caps(&mut report, &caps);
    let json = report.to_json_pretty();
    let markdown = report.to_markdown();

    println!("{json}");
    write_reports(&json, &markdown, &args.report_json, &args.report_md)?;
    Ok(exit_code_for(report.verdict))
}

/// Picks the reward schedule from exactly one of `--reward`(`--deadline`/`--time-scale`/
/// `--horizon`)/`--reward-schedule`, and rejects a flag that doesn't belong to the chosen
/// `--reward` variant (e.g. `--time-scale` with `--reward hard-deadline`) - clap's declarative
/// `requires`/`conflicts_with` can express "at least one of --reward/--reward-schedule" and "not
/// both `--reward` and `--reward-schedule`" but not "exactly this flag combination for exactly
/// this enum value," the same gap `resolve_sprt_hypotheses` fills for `--sprt-variant`.
fn resolve_time_sensitive_reward(
    args: &TimeSensitiveArgs,
) -> Result<time_sensitive::RewardSchedule, VeridictError> {
    if let Some(path) = &args.reward_schedule {
        let text = std::fs::read_to_string(path).map_err(|source| VeridictError::Io {
            path: path.display().to_string(),
            source,
        })?;
        let file: time_sensitive::reward::RewardScheduleFile = serde_json::from_str(&text)
            .map_err(|source| VeridictError::InvalidJsonFile {
                path: path.display().to_string(),
                source,
            })?;
        return time_sensitive::reward::expand_schedule_file(&file);
    }
    match args.reward {
        Some(RewardKindArg::HardDeadline) => {
            if args.time_scale.is_some() || args.horizon.is_some() {
                return Err(VeridictError::InvalidThreshold(
                    "--time-scale/--horizon are only used with --reward exponential; pass \
                     --deadline for --reward hard-deadline"
                        .to_string(),
                ));
            }
            let deadline = args.deadline.ok_or_else(|| {
                VeridictError::InvalidThreshold(
                    "--reward hard-deadline requires --deadline".to_string(),
                )
            })?;
            Ok(time_sensitive::RewardSchedule::HardDeadline { deadline })
        }
        Some(RewardKindArg::Exponential) => {
            if args.deadline.is_some() {
                return Err(VeridictError::InvalidThreshold(
                    "--deadline is only used with --reward hard-deadline; pass --time-scale/\
                     --horizon for --reward exponential"
                        .to_string(),
                ));
            }
            let time_scale = args.time_scale.ok_or_else(|| {
                VeridictError::InvalidThreshold(
                    "--reward exponential requires --time-scale".to_string(),
                )
            })?;
            let horizon = args.horizon.ok_or_else(|| {
                VeridictError::InvalidThreshold(
                    "--reward exponential requires --horizon".to_string(),
                )
            })?;
            Ok(time_sensitive::RewardSchedule::ExponentialDecay {
                time_scale,
                horizon,
            })
        }
        None => Err(VeridictError::InvalidThreshold(
            "exactly one of --reward or --reward-schedule is required".to_string(),
        )),
    }
}

fn write_reports(
    json: &str,
    markdown: &str,
    report_json: &Option<PathBuf>,
    report_md: &Option<PathBuf>,
) -> Result<(), VeridictError> {
    if let Some(path) = report_json {
        std::fs::write(path, json).map_err(|source| VeridictError::Io {
            path: path.display().to_string(),
            source,
        })?;
    }
    if let Some(path) = report_md {
        std::fs::write(path, markdown).map_err(|source| VeridictError::Io {
            path: path.display().to_string(),
            source,
        })?;
    }
    Ok(())
}

fn exit_code_for(verdict: Verdict) -> ExitCode {
    match verdict {
        Verdict::Pass => ExitCode::from(0),
        Verdict::Fail => ExitCode::from(1),
        Verdict::Inconclusive => ExitCode::from(2),
    }
}

fn resolve_format(path: &Path, explicit: Option<FormatArg>) -> FormatArg {
    explicit.unwrap_or_else(|| match path.extension().and_then(|e| e.to_str()) {
        Some(ext) if ext.eq_ignore_ascii_case("csv") => FormatArg::Csv,
        _ => FormatArg::Jsonl,
    })
}

type RecordIter = Box<dyn Iterator<Item = Result<(usize, input::Record), VeridictError>>>;

/// Streams records lazily from the file/stdin - callers that only need
/// `winrate`/`elo`/`sign-test`/`sprt` (never `mean-diff`) get bounded memory
/// regardless of input size, since nothing here materializes a `Vec` up
/// front. `input::parse_jsonl`/`parse_csv` are already lazy iterators; this
/// just boxes whichever one applies so both format branches share one
/// return type.
fn read_records(path: &PathBuf, format: FormatArg) -> Result<RecordIter, VeridictError> {
    let reader = open_input(path)?;
    Ok(match format {
        FormatArg::Jsonl => Box::new(input::parse_jsonl(reader)),
        FormatArg::Csv => Box::new(input::parse_csv(reader)),
    })
}

type MatchRecordIter = Box<dyn Iterator<Item = Result<(usize, input::MatchRecord), VeridictError>>>;

/// Same lazy-streaming shape as `read_records`, for `matrix --matches`.
fn read_match_records(path: &PathBuf, format: FormatArg) -> Result<MatchRecordIter, VeridictError> {
    let reader = open_input(path)?;
    Ok(match format {
        FormatArg::Jsonl => Box::new(input::parse_jsonl(reader)),
        FormatArg::Csv => Box::new(input::parse_csv(reader)),
    })
}

type VerifyRunRecordIter =
    Box<dyn Iterator<Item = Result<(usize, veridict::verify_run::VerifyRunRecord), VeridictError>>>;

/// Same lazy-parsing shape as `read_records`/`read_match_records`; `run_verify_run` still
/// collects the result into a `Vec` (see its own doc) since `verify_run::run`'s checks are
/// cross-record, unlike `compare`/`sprt`/`matrix`'s incremental tallies.
fn read_verify_run_records(
    path: &PathBuf,
    format: FormatArg,
) -> Result<VerifyRunRecordIter, VeridictError> {
    let reader = open_input(path)?;
    Ok(match format {
        FormatArg::Jsonl => Box::new(input::parse_jsonl(reader)),
        FormatArg::Csv => Box::new(input::parse_csv(reader)),
    })
}

fn open_input(path: &PathBuf) -> Result<Box<dyn BufRead>, VeridictError> {
    if path.as_os_str() == "-" {
        // Streamed directly, not slurped into a Vec first: input::parse_jsonl/
        // parse_csv are lazy, so buffering all of stdin up front here would be
        // the one place that silently defeats streaming for piped input.
        Ok(Box::new(io::BufReader::new(io::stdin())))
    } else {
        let file = std::fs::File::open(path).map_err(|source| VeridictError::Io {
            path: path.display().to_string(),
            source,
        })?;
        Ok(Box::new(io::BufReader::new(file)))
    }
}
