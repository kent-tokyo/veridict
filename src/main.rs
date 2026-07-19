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
    VeridictError, input, matrix, power,
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

    /// Symmetric effect-size threshold: pass if the CI lower bound is >= this, fail if the CI upper bound is <= -this. Ignored if --pass-above/--fail-below are given.
    #[arg(long)]
    min_effect: Option<f64>,

    /// Explicit pass threshold. Requires --fail-below.
    #[arg(long, requires = "fail_below", allow_hyphen_values = true)]
    pass_above: Option<f64>,

    /// Explicit fail threshold. Requires --pass-above.
    #[arg(long, requires = "pass_above", allow_hyphen_values = true)]
    fail_below: Option<f64>,

    /// Bootstrap resample count, used only by --metric mean-diff/quantile-diff.
    #[arg(long, default_value_t = 10_000)]
    resamples: usize,

    /// Bootstrap RNG seed, used only by --metric mean-diff/quantile-diff. Defaults to a
    /// fixed seed, so the same input reproduces bit-identical output in CI.
    #[arg(long)]
    seed: Option<u64>,

    /// Confidence interval method for --metric winrate/sign-test. `exact`
    /// (Clopper-Pearson) and `jeffreys` are only valid for those two metrics;
    /// combining either with --metric elo/mean-diff is a config error, not a
    /// silent fallback.
    #[arg(long, value_enum, default_value = "wilson")]
    ci_method: CiMethodArg,

    /// Bootstrap variant for --metric mean-diff/quantile-diff. `bca` corrects for bias and
    /// skewness (not available for quantile-diff - the sample quantile's jackknife acceleration
    /// term has no solid footing for a non-smooth statistic, a config error rather than a
    /// silent fallback); `basic` reflects the percentile interval around the point estimate
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
    /// (mean-diff/sign-test/quantile-diff cluster support is deferred, see docs/research-map.md).
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

    /// Multiple-comparison correction across this run's metric family (relevant whenever more
    /// than one --metric is given; a single-metric run is a harmless no-op). `none` (default):
    /// today's existing behavior, unchanged. `bonferroni`: uniform per-metric significance
    /// alpha/family_size. `holm`: step-down, uniformly more powerful than Bonferroni for the same
    /// family-wise guarantee. Either can only downgrade an unadjusted pass to inconclusive, never
    /// invent a fail - see docs/metrics.md's --correction section.
    #[arg(long, value_enum, default_value = "none")]
    correction: CorrectionArg,

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

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
enum PowerMetricArg {
    Winrate,
    SignTest,
    Elo,
    MeanDiff,
}

impl From<PowerMetricArg> for MetricKind {
    fn from(m: PowerMetricArg) -> Self {
        match m {
            PowerMetricArg::Winrate => MetricKind::WinRate,
            PowerMetricArg::SignTest => MetricKind::SignTest,
            PowerMetricArg::Elo => MetricKind::Elo,
            PowerMetricArg::MeanDiff => MetricKind::MeanDiff,
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
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum MetricArg {
    Winrate,
    MeanDiff,
    SignTest,
    Elo,
    QuantileDiff,
}

impl From<MetricArg> for MetricKind {
    fn from(m: MetricArg) -> Self {
        match m {
            MetricArg::Winrate => MetricKind::WinRate,
            MetricArg::MeanDiff => MetricKind::MeanDiff,
            MetricArg::SignTest => MetricKind::SignTest,
            MetricArg::Elo => MetricKind::Elo,
            MetricArg::QuantileDiff => MetricKind::QuantileDiff,
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

fn run(command: Command) -> Result<ExitCode, VeridictError> {
    match command {
        Command::Compare(args) => run_compare(args),
        Command::Sprt(args) => run_sprt(args),
        Command::Matrix(args) => run_matrix(args),
        Command::Plan(args) => run_plan(args),
        Command::Power(args) => run_power(args),
    }
}

fn run_compare(args: CompareArgs) -> Result<ExitCode, VeridictError> {
    let thresholds = match (args.pass_above, args.fail_below) {
        (Some(pass_above), Some(fail_below)) => Thresholds::new(pass_above, fail_below)?,
        _ => Thresholds::symmetric(args.min_effect.unwrap_or(0.0))?,
    };

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

    let correction: Correction = args.correction.into();
    let caps = FailureCaps {
        max_timeouts: args.max_timeouts,
        max_crashes: args.max_crashes,
        max_invalid: args.max_invalid,
    };
    let (verdict, json, markdown) = if let [only] = metrics[..] {
        let mut report = veridict::compare_one(
            records,
            only,
            args.confidence,
            &thresholds,
            args.resamples,
            seed,
            args.paired_by_id,
            args.cluster_by_id,
        )?;
        veridict::correction::apply_correction(
            std::slice::from_mut(&mut report),
            &metrics,
            correction,
            args.confidence,
        )?;
        verdict::apply_failure_caps(&mut report, &caps);
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
        veridict::correction::apply_correction(
            &mut multi.reports,
            &metrics,
            correction,
            args.confidence,
        )?;
        multi.verdict = verdict::aggregate(multi.reports.iter().map(|r| r.verdict));
        verdict::apply_failure_caps_to_multi(&mut multi, &caps);
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

    let mut report =
        veridict::sprt::run(records, &config, variant, args.paired_by_id, failure_policy)?;
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
