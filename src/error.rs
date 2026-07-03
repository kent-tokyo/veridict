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

    #[error(
        "line {line}: unrecognized result '{value}' (expected baseline_win|candidate_win|draw)"
    )]
    UnrecognizedOutcome { line: usize, value: String },

    #[error("io error reading '{path}': {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
}
