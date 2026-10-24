//! Shared status parsing and failure-policy outcome resolution.

use super::*;

/// Parses `raw` and tallies it into `counts` if it's a failure - returns the parsed
/// `TrialStatus` either way (not just `Ok(())`) so callers can also feed it to
/// `effective_outcome` without re-parsing the same string a second time.
pub(crate) fn tally_status(
    raw: &str,
    line: usize,
    field: &'static str,
    counts: &mut FailureCounts,
) -> Result<TrialStatus, VeridictError> {
    match TrialStatus::parse(raw) {
        Some(TrialStatus::Ok) => Ok(TrialStatus::Ok),
        Some(status @ TrialStatus::Timeout) => {
            counts.timeout += 1;
            Ok(status)
        }
        Some(status @ TrialStatus::Crash) => {
            counts.crash += 1;
            Ok(status)
        }
        Some(status @ TrialStatus::Invalid) => {
            counts.invalid += 1;
            Ok(status)
        }
        None => Err(VeridictError::UnrecognizedStatus {
            line,
            field,
            value: raw.to_string(),
        }),
    }
}

/// The outcome to actually record for win/loss/draw-shaped metrics (`winrate`/`elo`) and `sprt`,
/// given the record's parsed per-side status and `--failure-policy`. `ReportOnly` is exactly
/// today's pre-`FailurePolicy` behavior, unchanged: a failure is tallied by the caller (via
/// `tally_status`, before this runs) but never itself contributes an outcome - only a literal
/// `result` field does. `Exclude`/`Loss` only diverge from `ReportOnly` when a failure status is
/// present (a status-only record, the common case, already contributes nothing under any
/// policy).
///
/// Neither `Exclude` nor `Loss` validates `result`'s contents when a failure status is present -
/// once a policy has decided the trial's outcome is excluded or overridden, an unparseable
/// `result` string alongside it is moot, not silently-ignored invalid data (the ordinary
/// `UnrecognizedOutcome` check still applies whenever `result` is actually consulted, i.e.
/// whenever neither side failed, or under `ReportOnly`).
pub(crate) fn effective_outcome(
    policy: FailurePolicy,
    baseline_status: Option<TrialStatus>,
    candidate_status: Option<TrialStatus>,
    result: Option<&str>,
    line: usize,
) -> Result<Option<Outcome>, VeridictError> {
    let baseline_failed = matches!(baseline_status, Some(s) if s != TrialStatus::Ok);
    let candidate_failed = matches!(candidate_status, Some(s) if s != TrialStatus::Ok);

    match policy {
        FailurePolicy::ReportOnly => parse_result_outcome(result, line),
        FailurePolicy::Exclude if baseline_failed || candidate_failed => Ok(None),
        FailurePolicy::Exclude => parse_result_outcome(result, line),
        // A failure status overrides any literal `result` on the same record - trusting the
        // execution-level failure signal is the conservative choice (AGENTS.md: "a false pass
        // is worse than an inconclusive result"), so a `candidate_win` result next to a
        // `candidate_status: crash` can never silently override the crash.
        FailurePolicy::Loss => match (baseline_failed, candidate_failed) {
            (true, true) => Ok(Some(Outcome::Draw)),
            (true, false) => Ok(Some(Outcome::CandidateWin)),
            (false, true) => Ok(Some(Outcome::BaselineWin)),
            (false, false) => parse_result_outcome(result, line),
        },
    }
}

fn parse_result_outcome(
    result: Option<&str>,
    line: usize,
) -> Result<Option<Outcome>, VeridictError> {
    match result {
        None => Ok(None),
        Some(r) => match Outcome::parse(r) {
            Some(o) => Ok(Some(o)),
            None => Err(VeridictError::UnrecognizedOutcome {
                line,
                value: r.to_string(),
                expected: "baseline_win|candidate_win|draw",
            }),
        },
    }
}
