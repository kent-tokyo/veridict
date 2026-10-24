use super::*;

#[derive(Parser)]
#[command(
    name = "veridict",
    version,
    about = "Statistical decision gate: is the candidate actually better than the baseline?"
)]
pub(super) struct Cli {
    #[command(subcommand)]
    pub(super) command: Command,
}

#[derive(Subcommand)]
pub(super) enum Command {
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
pub(super) struct CompareArgs {
    /// Input file. Use "-" to read from stdin.
    pub(super) input: PathBuf,

    /// Repeat to run several metrics against the same input and combine
    /// them into one verdict (fail dominates, then inconclusive, then pass).
    #[arg(long = "metric", value_enum, required = true)]
    pub(super) metrics: Vec<MetricArg>,

    /// Input format. Defaults to sniffing the file extension (.csv vs
    /// everything else); pass explicitly when reading CSV from stdin.
    #[arg(long, value_enum)]
    pub(super) format: Option<FormatArg>,

    #[arg(long, default_value_t = 0.95)]
    pub(super) confidence: f64,

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
    pub(super) min_effect: Vec<MinEffectEntry>,

    /// Explicit pass threshold. Requires --fail-below.
    #[arg(long, requires = "fail_below", allow_hyphen_values = true)]
    pub(super) pass_above: Option<f64>,

    /// Explicit fail threshold. Requires --pass-above.
    #[arg(long, requires = "pass_above", allow_hyphen_values = true)]
    pub(super) fail_below: Option<f64>,

    /// Bootstrap resample count, used by bootstrap-based metrics including --metric paired-elo.
    #[arg(long, default_value_t = 10_000)]
    pub(super) resamples: usize,

    /// Bootstrap RNG seed, used by bootstrap-based metrics including --metric paired-elo.
    /// Defaults to a fixed seed, so the same input reproduces bit-identical output in CI.
    #[arg(long)]
    pub(super) seed: Option<u64>,

    /// Confidence interval method for --metric winrate/sign-test. `exact`
    /// (Clopper-Pearson) and `jeffreys` are only valid for those two metrics;
    /// combining either with --metric elo/paired-elo/mean-diff/relative-diff is a config error,
    /// not a silent fallback.
    #[arg(long, value_enum, default_value = "wilson")]
    pub(super) ci_method: CiMethodArg,

    /// Bootstrap variant for --metric mean-diff/quantile-diff/relative-diff. `bca` corrects for
    /// bias and skewness (not available for quantile-diff - the sample quantile's jackknife
    /// acceleration term has no solid footing for a non-smooth statistic, a config error rather
    /// than a silent fallback; available for relative-diff, whose sample mean is smooth the same
    /// way mean-diff's is); `basic` reflects the percentile interval around the point estimate
    /// (simpler, no bias-correction of its own); `percentile` stays the default so existing CI
    /// numbers don't silently shift.
    #[arg(long, value_enum, default_value = "percentile")]
    pub(super) bootstrap_method: BootstrapMethodArg,

    /// Quantile to measure for --metric quantile-diff (e.g. 0.95 for p95); must be in (0, 1).
    /// Defaults to 0.5 (the median) when --metric quantile-diff is used without this flag.
    /// Unused by every other requested metric, so it can be combined with e.g. --metric
    /// mean-diff in the same multi-metric run - but a config error if no --metric
    /// quantile-diff is requested at all.
    #[arg(long)]
    pub(super) quantile: Option<f64>,

    /// Treat two records sharing an id as one testcase played twice (e.g.
    /// roles swapped to cancel the testcase's own bias) and combine them
    /// into a single net observation instead of two independent ones. An id
    /// used only once is an ordinary unpaired sample; 3+ uses of the same
    /// id is rejected as a data error. `--metric paired-elo` is stricter: every record requires
    /// an id and every id must have exactly two usable outcomes.
    #[arg(long, conflicts_with = "cluster_by_id")]
    pub(super) paired_by_id: bool,

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
    pub(super) cluster_by_id: bool,

    /// How a failed trial affects --metric winrate/elo/paired-elo (other metrics reject anything
    /// but the default as a config error). `report-only` (default): failures are tallied and a
    /// literal `result`, when present, is still used. `exclude`: a failed side's `result` is not
    /// tallied - it differs from `report-only` only when a record has both a failure status and a
    /// `result`.
    /// `loss`: a failed side's outcome is synthesized (candidate failed -> baseline_win,
    /// baseline failed -> candidate_win, both failed -> draw), overriding any literal `result`
    /// on the same record.
    #[arg(long, value_enum, default_value = "report-only")]
    pub(super) failure_policy: FailurePolicyArg,

    /// Multiple-comparison correction across this run's metric family of *simultaneous per-metric
    /// claims* (relevant whenever more than one --metric is given; a single-metric run is a
    /// harmless no-op) - populates each report's family_adjusted_verdict/family_adjusted_promotion
    /// and, for a multi-metric run, the overall simultaneous_claims_promotion. Does NOT change
    /// verdict/promotion above (the deployment gate already requires every metric to pass, which
    /// needs no correction of its own - see docs/metrics.md's --claim-correction section). `none`
    /// (default): today's existing behavior, unchanged. `bonferroni`: uniform per-metric
    /// significance alpha/family_size. `holm`: step-down, uniformly more powerful than Bonferroni
    /// for the same family-wise guarantee. Rejected as a configuration error for a family that
    /// includes --metric paired-elo/mean-diff/quantile-diff/relative-diff (no closed-form CI to
    /// correct) or
    /// was run with --cluster-by-id (correction would reconstruct the wrong CI shape).
    #[arg(long, value_enum, conflicts_with = "correction")]
    pub(super) claim_correction: Option<CorrectionArg>,

    /// Deprecated alias for --claim-correction, kept for one release. Prints a deprecation
    /// warning to stderr and behaves identically; --claim-correction/--correction are mutually
    /// exclusive.
    #[arg(long, value_enum, conflicts_with = "claim_correction")]
    pub(super) correction: Option<CorrectionArg>,

    /// Hard cap on candidate+baseline timeout count. Breaching it forces validity=invalid,
    /// verdict=inconclusive, promotion=not_promoted regardless of the metric's own effect/CI -
    /// unlike --failure-policy, this can never be satisfied by more clean trials. Unset (default):
    /// uncapped, existing behavior.
    #[arg(long)]
    pub(super) max_timeouts: Option<u64>,

    /// Hard cap on candidate+baseline crash count. See --max-timeouts for the exact semantics.
    #[arg(long)]
    pub(super) max_crashes: Option<u64>,

    /// Hard cap on candidate+baseline invalid-result count. See --max-timeouts for the exact
    /// semantics.
    #[arg(long)]
    pub(super) max_invalid: Option<u64>,

    /// Also write the JSON report to this file.
    #[arg(long)]
    pub(super) report_json: Option<PathBuf>,

    /// Also write a human-readable Markdown report to this file.
    #[arg(long)]
    pub(super) report_md: Option<PathBuf>,
}

#[derive(clap::Args)]
pub(super) struct SprtArgs {
    /// Input file. Use "-" to read from stdin.
    pub(super) input: PathBuf,

    /// Input format. Defaults to sniffing the file extension (.csv vs
    /// everything else); pass explicitly when reading CSV from stdin.
    #[arg(long, value_enum)]
    pub(super) format: Option<FormatArg>,

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
    pub(super) sprt_variant: SprtVariantArg,

    /// H0 for --sprt-variant wald: the candidate is at most this many
    /// logistic-Elo points stronger.
    #[arg(long, allow_hyphen_values = true)]
    pub(super) elo0: Option<f64>,

    /// H1 for --sprt-variant wald: the candidate is at least this many
    /// logistic-Elo points stronger.
    #[arg(long, allow_hyphen_values = true)]
    pub(super) elo1: Option<f64>,

    /// H0 for --sprt-variant trinomial: the candidate is at most this many
    /// BayesElo points stronger.
    #[arg(long, allow_hyphen_values = true)]
    pub(super) belo0: Option<f64>,

    /// H1 for --sprt-variant trinomial: the candidate is at least this many
    /// BayesElo points stronger.
    #[arg(long, allow_hyphen_values = true)]
    pub(super) belo1: Option<f64>,

    /// False-positive rate: probability of accepting H1 when H0 is true.
    #[arg(long, default_value_t = 0.05)]
    pub(super) alpha: f64,

    /// False-negative rate: probability of accepting H0 when H1 is true. Alpha and beta must sum
    /// to less than 1 so the lower boundary remains below the upper boundary.
    #[arg(long, default_value_t = 0.05)]
    pub(super) beta: f64,

    /// Treat two records sharing an id as one testcase played twice and
    /// combine them into a single net observation. See `compare
    /// --paired-by-id` for the exact semantics.
    #[arg(long)]
    pub(super) paired_by_id: bool,

    /// Minimum completed pairs before a boundary crossing counts as decisive. Must be >= 1.
    /// Requires --sprt-variant pentanomial (the only variant with a paired_count). `sprt::run`
    /// walks completed pairs in the order they complete (see its module doc) and never even
    /// evaluates a boundary crossing before this many pairs have completed - so a crossing seen
    /// only in the first few pairs is never "remembered" once the minimum is reached; the
    /// decision is always based on the LLR at the pair where the walk actually stops.
    #[arg(long)]
    pub(super) min_paired_ids: Option<u64>,

    /// Reached-without-a-decision marker. Must be >= 1. If the walk reaches this many completed
    /// pairs without a boundary crossing, it stops there with an inconclusive verdict (no
    /// truncated-SPRT decision rule) and `reason`/`stopping_reason` note the cap was hit. Pairs
    /// completed after this point, even if present in the input, never affect the verdict, LLR,
    /// or bucket counts. Requires --sprt-variant pentanomial, same as --min-paired-ids.
    #[arg(long)]
    pub(super) max_paired_ids: Option<u64>,

    /// Require every id to appear in exactly 2 records instead of tolerating a lone id as an
    /// ordinary unpaired sample. Requires --paired-by-id. Extends --sprt-variant pentanomial's
    /// existing unconditional "exactly 2, no exceptions" pairing to wald/trinomial too; a
    /// documented no-op for pentanomial itself, which is already this strict regardless of this
    /// flag.
    #[arg(long)]
    pub(super) require_complete_pairs: bool,

    /// How a failed trial affects the LLR. See `compare --failure-policy` for the exact
    /// semantics (applies identically here, across all three --sprt-variant choices - a `loss`-
    /// synthesized outcome nets against its pair partner the same way for --sprt-variant
    /// pentanomial as it does for wald/trinomial).
    #[arg(long, value_enum, default_value = "report-only")]
    pub(super) failure_policy: FailurePolicyArg,

    /// Hard cap on candidate+baseline timeout count. See `compare --max-timeouts` for the exact
    /// semantics (breaching it forces validity=invalid/verdict=inconclusive regardless of the
    /// LLR).
    #[arg(long)]
    pub(super) max_timeouts: Option<u64>,

    /// Hard cap on candidate+baseline crash count. See `compare --max-timeouts`.
    #[arg(long)]
    pub(super) max_crashes: Option<u64>,

    /// Hard cap on candidate+baseline invalid-result count. See `compare --max-timeouts`.
    #[arg(long)]
    pub(super) max_invalid: Option<u64>,

    /// Also write the JSON report to this file.
    #[arg(long)]
    pub(super) report_json: Option<PathBuf>,

    /// Also write a human-readable Markdown report to this file.
    #[arg(long)]
    pub(super) report_md: Option<PathBuf>,
}

#[derive(clap::Args)]
pub(super) struct MatrixArgs {
    /// Legacy: one file per candidate, each measured against the same shared baseline (id/
    /// baseline/candidate/result schema). Candidate names come from each file's stem (e.g.
    /// "prompt_a.jsonl" -> "prompt_a"). At least one of `files`/`--matches` is required.
    #[arg(num_args = 1.., required_unless_present = "matches")]
    pub(super) files: Vec<PathBuf>,

    /// Head-to-head match data between named competitors (id/a/b/result schema, result is
    /// a_win|b_win|draw). Competitor names come from each record's a/b fields, not the file
    /// name. Repeatable. Use the literal name "baseline" in a/b to connect this data to the
    /// baseline node implied by `files`.
    #[arg(long = "matches", num_args = 1.., required_unless_present = "files")]
    pub(super) matches: Vec<PathBuf>,

    /// Input format, applied to every file (both legacy files and --matches). Defaults to
    /// sniffing each file's extension.
    #[arg(long, value_enum)]
    pub(super) format: Option<FormatArg>,

    #[arg(long, default_value_t = 0.95)]
    pub(super) confidence: f64,

    /// Treat two records sharing an id as one testcase played twice and
    /// combine them into a single net observation. See `compare
    /// --paired-by-id` for the exact semantics; applied independently to
    /// each file.
    #[arg(long)]
    pub(super) paired_by_id: bool,

    /// Bootstrap resample count for general-graph confidence intervals. Has
    /// no effect when every edge touches "baseline" (star mode uses the
    /// closed-form Wilson interval instead).
    #[arg(long, default_value_t = 2_000)]
    pub(super) resamples: usize,

    /// Bootstrap RNG seed for general-graph confidence intervals. Defaults
    /// to a fixed seed, so the same input reproduces bit-identical output.
    #[arg(long)]
    pub(super) seed: Option<u64>,

    /// Bootstrap variant for general-graph confidence intervals. Same
    /// meaning as `compare`'s `--bootstrap-method`; has no effect in star
    /// mode (closed-form Wilson interval, no bootstrap involved).
    #[arg(long, value_enum, default_value = "percentile")]
    pub(super) bootstrap_method: BootstrapMethodArg,

    /// Also write the JSON report to this file.
    #[arg(long)]
    pub(super) report_json: Option<PathBuf>,

    /// Also write a human-readable Markdown report to this file.
    #[arg(long)]
    pub(super) report_md: Option<PathBuf>,
}

#[derive(clap::Args)]
pub(super) struct PlanArgs {
    /// Same as `matrix`'s `files`/`--matches`/`--format`/`--paired-by-id`/`--resamples`/
    /// `--seed`/`--bootstrap-method` - `plan` runs `matrix` internally and recommends from its
    /// result, so every input flag means exactly what it means there.
    #[arg(num_args = 1.., required_unless_present = "matches")]
    pub(super) files: Vec<PathBuf>,

    #[arg(long = "matches", num_args = 1.., required_unless_present = "files")]
    pub(super) matches: Vec<PathBuf>,

    #[arg(long, value_enum)]
    pub(super) format: Option<FormatArg>,

    #[arg(long, default_value_t = 0.95)]
    pub(super) confidence: f64,

    #[arg(long)]
    pub(super) paired_by_id: bool,

    #[arg(long, default_value_t = 2_000)]
    pub(super) resamples: usize,

    #[arg(long)]
    pub(super) seed: Option<u64>,

    #[arg(long, value_enum, default_value = "percentile")]
    pub(super) bootstrap_method: BootstrapMethodArg,

    /// The Elo gap worth being able to detect - recommendations narrow each pair's CI toward
    /// this half-width. Required: there's no sensible default for "how precise do you need this."
    #[arg(long, allow_hyphen_values = true)]
    pub(super) min_elo: f64,

    /// Also write the JSON report to this file.
    #[arg(long)]
    pub(super) report_json: Option<PathBuf>,

    /// Also write a human-readable Markdown report to this file.
    #[arg(long)]
    pub(super) report_md: Option<PathBuf>,
}

#[derive(clap::Args)]
pub(super) struct PowerArgs {
    #[arg(
        long,
        value_enum,
        required_unless_present = "sprt",
        conflicts_with = "sprt"
    )]
    pub(super) metric: Option<PowerMetricArg>,

    /// The pass bar - identical meaning to `compare --min-effect`/`--pass-above` (the CI lower
    /// bound a real run must clear to pass).
    #[arg(
        long,
        allow_hyphen_values = true,
        required_unless_present = "sprt",
        conflicts_with = "sprt"
    )]
    pub(super) min_effect: Option<f64>,

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
    pub(super) assume_effect: Option<f64>,

    #[arg(long, default_value_t = 0.95, conflicts_with = "sprt")]
    pub(super) confidence: f64,

    /// Target probability of reaching a passing verdict, assuming the true effect is exactly
    /// --assume-effect.
    #[arg(long, default_value_t = 0.80, conflicts_with = "sprt")]
    pub(super) target_power: f64,

    /// Confidence interval method - same meaning as `compare --ci-method`; `exact`/`jeffreys` are
    /// only valid for winrate/sign-test, same restriction as `compare`.
    #[arg(long, value_enum, default_value = "wilson", conflicts_with = "sprt")]
    pub(super) ci_method: CiMethodArg,

    /// Accepted but does not change the estimate - see `docs/metrics.md`'s `power` section for
    /// why the actual variance reduction from pairing can't be predicted without real data. Adds
    /// a caveat to the report's `notes` instead. For `--metric mean-diff --pilot FILE`, this DOES
    /// change the estimate - same-id records in the pilot data are netted before estimating a
    /// standard deviation, same as `compare --paired-by-id` itself.
    #[arg(long, conflicts_with = "sprt")]
    pub(super) paired_by_id: bool,

    /// Assumed standard deviation of the paired (candidate - baseline) difference, for --metric
    /// mean-diff (mean-diff has no closed-form CI to search a hypothetical n against, so power
    /// analysis needs this assumption from the caller instead). Mutually exclusive with --pilot;
    /// exactly one is required when --metric mean-diff, neither is accepted otherwise.
    #[arg(long, conflicts_with_all = ["pilot", "sprt"])]
    pub(super) assume_sd: Option<f64>,

    /// Pilot data file (same JSONL/CSV format as `compare`'s input) to estimate --metric
    /// mean-diff's standard deviation from, instead of supplying --assume-sd directly. Use "-" to
    /// read from stdin.
    #[arg(long, conflicts_with_all = ["assume_sd", "sprt"])]
    pub(super) pilot: Option<PathBuf>,

    /// Format override for --pilot; same sniffing rules as `compare --format`.
    #[arg(long, value_enum, conflicts_with = "sprt")]
    pub(super) pilot_format: Option<FormatArg>,

    /// Estimate SPRT's expected sample size (Wald's ASN approximation) instead of a
    /// CI-crossing-probability search. Requires --elo0/--elo1; mutually exclusive with
    /// --metric/--min-effect/--assume-effect/--confidence/--target-power/--ci-method/
    /// --paired-by-id, since Wald's alpha/beta already fix the guaranteed error rates - there's no
    /// target power to search a sample size for.
    #[arg(long, requires_all = ["elo0", "elo1"])]
    pub(super) sprt: bool,

    /// H0 for --sprt: the candidate is at most this many logistic-Elo points stronger. Same
    /// meaning as `veridict sprt --elo0`.
    #[arg(long, allow_hyphen_values = true, requires = "sprt")]
    pub(super) elo0: Option<f64>,

    /// H1 for --sprt: the candidate is at least this many logistic-Elo points stronger. Same
    /// meaning as `veridict sprt --elo1`.
    #[arg(long, allow_hyphen_values = true, requires = "sprt")]
    pub(super) elo1: Option<f64>,

    /// False-positive rate for --sprt. Same meaning and default as `veridict sprt --alpha`.
    #[arg(long, default_value_t = 0.05)]
    pub(super) alpha: f64,

    /// False-negative rate for --sprt. Same meaning and default as `veridict sprt --beta`.
    /// Alpha and beta must sum to less than 1.
    #[arg(long, default_value_t = 0.05)]
    pub(super) beta: f64,

    /// For --sprt: also report the Monte Carlo probability that a real `veridict sprt` run
    /// still hasn't reached a decision after this many decisive trials, evaluated at the
    /// realistic worst-case true strength (halfway between --elo0/--elo1, not either endpoint).
    /// A planning number for choosing the next gate's trial budget/cutoff - it does not change
    /// how a real run itself should be stopped (alpha/beta already fully determine that).
    #[arg(long, requires = "sprt")]
    pub(super) horizon: Option<u64>,

    /// Also write the JSON report to this file.
    #[arg(long)]
    pub(super) report_json: Option<PathBuf>,

    /// Also write a human-readable Markdown report to this file.
    #[arg(long)]
    pub(super) report_md: Option<PathBuf>,
}

#[derive(clap::Args)]
pub(super) struct VerifyRunArgs {
    /// Path to the manifest.toml declaring this run's expected identifiers/hashes/schedule.
    pub(super) manifest: PathBuf,

    /// Path to the games.jsonl (or CSV) of per-record observations. Use "-" to read from stdin.
    pub(super) games: PathBuf,

    /// Input format for `games`. Defaults to sniffing the file extension (.csv vs everything
    /// else); pass explicitly when reading CSV from stdin.
    #[arg(long, value_enum)]
    pub(super) format: Option<FormatArg>,

    /// Also write the JSON report to this file.
    #[arg(long)]
    pub(super) report_json: Option<PathBuf>,

    /// Also write a human-readable Markdown report to this file.
    #[arg(long)]
    pub(super) report_md: Option<PathBuf>,
}

#[derive(clap::Args)]
pub(super) struct TimeSensitiveArgs {
    /// Input file. Use "-" to read from stdin.
    pub(super) input: PathBuf,

    /// Input format. Defaults to sniffing the file extension (.csv vs
    /// everything else); pass explicitly when reading CSV from stdin.
    #[arg(long, value_enum)]
    pub(super) format: Option<FormatArg>,

    /// H0: the candidate's decisive-observation-conditional success probability. Must satisfy
    /// 0 < p0 < p1 < 1.
    #[arg(long)]
    pub(super) p0: f64,

    /// H1: the candidate's decisive-observation-conditional success probability.
    #[arg(long)]
    pub(super) p1: f64,

    /// False-positive rate: probability of crossing the wealth threshold when H0 is true.
    #[arg(long, default_value_t = 0.05)]
    pub(super) alpha: f64,

    /// Betting policy. `gro`: patient, classical anytime-valid baseline (bets p1 every trial,
    /// ignores the reward schedule). `bellman`: numerical grid approximation of the time-
    /// sensitive-optimal policy for the given reward schedule. `edo`: closed-form stationary
    /// approximation, valid only for `--reward exponential`.
    #[arg(long, value_enum)]
    pub(super) policy: TimeSensitivePolicyArg,

    /// Reward schedule shape. Exactly one of --reward/--reward-schedule is required.
    #[arg(long, value_enum, conflicts_with = "reward_schedule")]
    pub(super) reward: Option<RewardKindArg>,

    /// Deadline for --reward hard-deadline: reward is 1 for a rejection at or before this trial,
    /// 0 after.
    #[arg(long, requires = "reward")]
    pub(super) deadline: Option<u64>,

    /// Time scale for --reward exponential: reward decays as exp(-t/time_scale).
    #[arg(long, requires = "reward")]
    pub(super) time_scale: Option<f64>,

    /// Finite backward-induction truncation horizon for --reward exponential.
    #[arg(long, requires = "reward")]
    pub(super) horizon: Option<u64>,

    /// Path to a custom reward-schedule JSON file: {"rewards": [{"until": N, "value": V}, ...,
    /// {"after": N, "value": 0.0}]}. Exactly one of --reward/--reward-schedule is required.
    #[arg(long, conflicts_with_all = ["reward", "deadline", "time_scale", "horizon"])]
    pub(super) reward_schedule: Option<PathBuf>,

    /// Action-grid resolution. Only meaningful for --policy bellman; still validated (>= 2) for
    /// every policy.
    #[arg(long, default_value_t = 101)]
    pub(super) action_grid_size: usize,

    /// Log-wealth grid resolution.
    #[arg(long, default_value_t = 200)]
    pub(super) wealth_grid_size: usize,

    /// How a failed trial affects the wealth process. See `compare --failure-policy` for the
    /// exact semantics.
    #[arg(long, value_enum, default_value = "report-only")]
    pub(super) failure_policy: FailurePolicyArg,

    /// Hard cap on candidate+baseline timeout count. See `compare --max-timeouts`.
    #[arg(long)]
    pub(super) max_timeouts: Option<u64>,

    /// Hard cap on candidate+baseline crash count. See `compare --max-timeouts`.
    #[arg(long)]
    pub(super) max_crashes: Option<u64>,

    /// Hard cap on candidate+baseline invalid-result count. See `compare --max-timeouts`.
    #[arg(long)]
    pub(super) max_invalid: Option<u64>,

    /// Also write the JSON report to this file.
    #[arg(long)]
    pub(super) report_json: Option<PathBuf>,

    /// Also write a human-readable Markdown report to this file.
    #[arg(long)]
    pub(super) report_md: Option<PathBuf>,
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(super) enum PowerMetricArg {
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
pub(super) fn power_metric_arg_label(m: PowerMetricArg) -> &'static str {
    match m {
        PowerMetricArg::Winrate => "winrate",
        PowerMetricArg::SignTest => "sign-test",
        PowerMetricArg::Elo => "elo",
        PowerMetricArg::MeanDiff => "mean-diff",
        PowerMetricArg::RelativeDiff => "relative-diff",
    }
}

#[derive(Clone, Copy, ValueEnum)]
pub(super) enum MetricArg {
    Winrate,
    MeanDiff,
    SignTest,
    Elo,
    PairedElo,
    QuantileDiff,
    RelativeDiff,
}

/// One comma-delimited `--min-effect` entry: either a bare number (the run's default) or
/// `metric=value` (an override for just that metric). See `CompareArgs::min_effect`'s doc.
#[derive(Clone, Copy)]
pub(super) enum MinEffectEntry {
    Default(f64),
    Metric(MetricArg, f64),
}

pub(super) fn parse_min_effect_entry(s: &str) -> Result<MinEffectEntry, String> {
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

pub(super) fn metric_arg_label(m: MetricArg) -> String {
    m.to_possible_value().unwrap().get_name().to_string()
}

impl From<MetricArg> for MetricKind {
    fn from(m: MetricArg) -> Self {
        match m {
            MetricArg::Winrate => MetricKind::WinRate,
            MetricArg::MeanDiff => MetricKind::MeanDiff,
            MetricArg::SignTest => MetricKind::SignTest,
            MetricArg::Elo => MetricKind::Elo,
            MetricArg::PairedElo => MetricKind::PairedElo,
            MetricArg::QuantileDiff => MetricKind::QuantileDiff,
            MetricArg::RelativeDiff => MetricKind::RelativeDiff,
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
pub(super) enum FormatArg {
    Jsonl,
    Csv,
}

#[derive(Clone, Copy, ValueEnum)]
pub(super) enum CorrectionArg {
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
pub(super) enum CiMethodArg {
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
pub(super) enum BootstrapMethodArg {
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
pub(super) enum FailurePolicyArg {
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
pub(super) enum SprtVariantArg {
    Wald,
    Trinomial,
    Pentanomial,
}

#[derive(Clone, Copy, ValueEnum)]
pub(super) enum TimeSensitivePolicyArg {
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
pub(super) enum RewardKindArg {
    HardDeadline,
    Exponential,
}
