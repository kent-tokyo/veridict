/// All errors the library can return. Every variant represents bad input or
/// bad configuration; the CLI maps every one of them to exit code 3.
#[derive(Debug, thiserror::Error)]
pub enum VeridictError {
    #[error("invalid JSON on line {line}: {source}")]
    InvalidJson {
        line: usize,
        #[source]
        source: serde_json::Error,
    },

    #[error("invalid CSV on line {line}: {source}")]
    InvalidCsv {
        line: usize,
        #[source]
        source: csv::Error,
    },

    /// `context` names what was being computed (e.g. "metric winrate",
    /// "sprt") when a record turned out to carry none of the fields that
    /// computation understands. A plain label rather than `MetricKind`
    /// because not every consumer of this error is a `MetricKind` (SPRT
    /// isn't one of the `compare` metrics).
    #[error("line {line}: record incompatible with {context}: {detail}")]
    SchemaMismatch {
        line: usize,
        context: &'static str,
        detail: String,
    },

    #[error("duplicate id '{id}' in paired numeric input (line {line})")]
    DuplicateId { id: String, line: usize },

    #[error("input contains no records")]
    EmptyInput,

    #[error("invalid confidence level {0}: must be finite and in (0, 1)")]
    InvalidConfidence(f64),

    #[error("invalid threshold configuration: {0}")]
    InvalidThreshold(String),

    #[error(
        "--ci-method {method} is only supported for metric winrate or sign-test (got {metric}); \
         elo's p_hat/n is fractional (draws count as 0.5) and mean-diff isn't a binomial \
         proportion, so neither an exact nor a Jeffreys binomial credible interval applies to \
         either"
    )]
    IncompatibleCiMethod {
        method: &'static str,
        metric: &'static str,
    },

    #[error(
        "--failure-policy {policy} is only supported for metric winrate or elo (got {metric}); \
         mean-diff/sign-test have no win/loss/draw outcome for a failed trial to become, so \
         there's no principled way to exclude or lose a numeric trial"
    )]
    IncompatibleFailurePolicy {
        policy: &'static str,
        metric: &'static str,
    },

    #[error(
        "--bootstrap-method {method} is not available for {metric}; the sample quantile is a \
         non-smooth statistic, so BCa's jackknife acceleration term has no solid asymptotic \
         footing the way it does for mean-diff - use percentile or basic instead (see \
         docs/metrics.md's quantile-diff section)"
    )]
    IncompatibleBootstrapMethod {
        method: &'static str,
        metric: &'static str,
    },

    #[error("invalid --quantile {0}: must be finite and in (0, 1)")]
    InvalidQuantile(f64),

    #[error("--quantile requires --metric quantile-diff to be one of the requested metrics")]
    QuantileRequiresQuantileDiffMetric,

    #[error(
        "power estimation for metric {metric} isn't supported via this flat (kind, ci_method) \
         constructor; winrate/sign-test/elo take a ci_method here, but mean-diff needs an assumed \
         standard deviation instead - construct PowerMetric::MeanDiff {{ assume_sd, sd_source }} \
         directly (see docs/metrics.md's power section)"
    )]
    UnsupportedPowerMetric { metric: &'static str },

    #[error(
        "power estimation isn't supported for metric quantile-diff; it needs an order-statistic \
         asymptotic variance (or a density estimate at the quantile), a separate research \
         problem from mean-diff's closed-form power - see docs/research-map.md"
    )]
    PowerUnsupportedForQuantileDiff,

    #[error(
        "--assume-effect ({assume_effect}) must be strictly greater than --min-effect \
         ({min_effect}); power evaluated with the true effect equal to the pass threshold is \
         just the interval's own miscoverage at that boundary (~1-confidence), not something \
         more trials climbs toward a target power - see docs/metrics.md's power section"
    )]
    PowerRequiresEffectGap { min_effect: f64, assume_effect: f64 },

    #[error("invalid --target-power {0}: must be finite and in (0, 1)")]
    InvalidTargetPower(f64),

    #[error(
        "power search exceeded {cap} trials without finding a stable n at target_power \
         {target_power}; the requested effect gap (min_effect={min_effect}, \
         assume_effect={assume_effect}) is too small relative to --confidence/--target-power for \
         this to be a practical experiment size"
    )]
    PowerSearchExceededCap {
        cap: u64,
        min_effect: f64,
        assume_effect: f64,
        target_power: f64,
    },

    #[error("invalid --assume-sd {0}: must be finite and greater than 0")]
    InvalidAssumeSd(f64),

    #[error(
        "--pilot '{path}' produced only {count} usable paired diff(s); at least 2 are needed to \
         estimate a standard deviation"
    )]
    InsufficientPilotData { path: String, count: usize },

    #[error(
        "--pilot '{path}' data has zero variance ({count} identical paired diff(s)) - a sample \
         standard deviation of 0 isn't a usable power-analysis input; pass --assume-sd directly \
         with a real assumed value instead"
    )]
    ZeroVariancePilotData { path: String, count: usize },

    #[error(
        "--metric mean-diff requires exactly one of --assume-sd or --pilot FILE (there's no real \
         data pre-experiment to estimate a standard deviation from otherwise) - see \
         docs/metrics.md's power section"
    )]
    MeanDiffPowerRequiresSd,

    #[error(
        "--assume-sd/--pilot are only used with --metric mean-diff (got {metric}); the other \
         metrics' power search doesn't need an assumed standard deviation"
    )]
    AssumeSdOnlyForMeanDiff { metric: &'static str },

    #[error(
        "line {line}: invalid numeric value in field '{field}': {value} (NaN/Infinity not allowed)"
    )]
    InvalidValue {
        line: usize,
        field: &'static str,
        value: f64,
    },

    #[error(
        "line {line}: unrecognized status '{value}' in field '{field}' (expected ok|timeout|crash|invalid)"
    )]
    UnrecognizedStatus {
        line: usize,
        field: &'static str,
        value: String,
    },

    #[error("line {line}: unrecognized result '{value}' (expected {expected})")]
    UnrecognizedOutcome {
        line: usize,
        value: String,
        expected: &'static str,
    },

    #[error("io error reading '{path}': {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },

    #[error(
        "--cluster-by-id is only supported for metric winrate or elo (got {metric}); mean-diff/\
         sign-test/quantile-diff cluster-robust support is a separate, deferred piece of work \
         (see docs/research-map.md)"
    )]
    IncompatibleClusterById { metric: &'static str },

    #[error(
        "--cluster-by-id and --paired-by-id are mutually exclusive: pairing nets exactly two \
         records sharing an id into one observation, while clustering keeps every record in an \
         id group as its own resampling unit - the two describe incompatible ways to treat a \
         repeated id"
    )]
    ClusterByIdConflictsWithPairedById,

    #[error(
        "--correction is not supported together with --cluster-by-id: achieved_alpha rebuilds a \
         plain i.i.d. Wilson CI from the raw record count, not the cluster bootstrap CI the report \
         actually used - under positive intra-cluster correlation that reconstruction is narrower \
         than the true cluster-robust CI, so correction could under-downgrade a pass instead of \
         catching it, the opposite of this project's own false-pass-is-worse-than-inconclusive \
         bias. Rejected outright rather than silently applying a mismatched correction; \
         cluster-aware correction is a separate, deferred piece of work (see \
         docs/research-map.md)"
    )]
    CorrectionConflictsWithClusterById,

    #[error(
        "Bradley-Terry MM solver did not converge after {iterations} iterations \
         (largest relative rating change {last_relative_change:.3e} still above the \
         {threshold:.0e} threshold; competitor index {worst_competitor} changed most). \
         This input already passed the strong-connectivity check, so it is not \
         literally disconnected - it usually means a technically-connected but heavily \
         lopsided graph (e.g. one subgroup beat another all-but-once). Inspect win/loss \
         tallies for near-total sweeps between subgroups before rerunning."
    )]
    BradleyTerryDidNotConverge {
        iterations: usize,
        last_relative_change: f64,
        threshold: f64,
        worst_competitor: usize,
    },
}
