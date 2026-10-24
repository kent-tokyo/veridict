//! CLI input and report-file I/O. Parsing stays lazy so streaming commands keep bounded memory.

use std::io::{self, BufRead};
use std::path::{Path, PathBuf};

use veridict::{VeridictError, input};

use super::FormatArg;

pub(super) fn write_reports(
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

pub(super) fn resolve_format(path: &Path, explicit: Option<FormatArg>) -> FormatArg {
    explicit.unwrap_or_else(|| match path.extension().and_then(|e| e.to_str()) {
        Some(ext) if ext.eq_ignore_ascii_case("csv") => FormatArg::Csv,
        _ => FormatArg::Jsonl,
    })
}

type RecordIter = Box<dyn Iterator<Item = Result<(usize, input::Record), VeridictError>>>;

/// Streams records lazily from the file/stdin. Commands whose aggregators use bounded state do
/// not materialize the full input here.
pub(super) fn read_records(path: &PathBuf, format: FormatArg) -> Result<RecordIter, VeridictError> {
    let reader = open_input(path)?;
    Ok(match format {
        FormatArg::Jsonl => Box::new(input::parse_jsonl(reader)),
        FormatArg::Csv => Box::new(input::parse_csv(reader)),
    })
}

type MatchRecordIter = Box<dyn Iterator<Item = Result<(usize, input::MatchRecord), VeridictError>>>;

pub(super) fn read_match_records(
    path: &PathBuf,
    format: FormatArg,
) -> Result<MatchRecordIter, VeridictError> {
    let reader = open_input(path)?;
    Ok(match format {
        FormatArg::Jsonl => Box::new(input::parse_jsonl(reader)),
        FormatArg::Csv => Box::new(input::parse_csv(reader)),
    })
}

type VerifyRunRecordIter =
    Box<dyn Iterator<Item = Result<(usize, veridict::verify_run::VerifyRunRecord), VeridictError>>>;

/// Parsing is lazy; `verify_run::run` decides when cross-record checks require collection.
pub(super) fn read_verify_run_records(
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
        Ok(Box::new(io::BufReader::new(io::stdin())))
    } else {
        let file = std::fs::File::open(path).map_err(|source| VeridictError::Io {
            path: path.display().to_string(),
            source,
        })?;
        Ok(Box::new(io::BufReader::new(file)))
    }
}
