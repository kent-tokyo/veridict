//! JSONL input parsing.
//!
//! Deliberately a single flat struct with `Option` fields rather than a
//! `#[serde(untagged)]` enum: untagged enums produce an opaque "data did not
//! match any variant" error on mismatch, which conflicts with the
//! requirement that bad input produce a clear, line-numbered error.

use std::io::{BufRead, Read};

use serde::Deserialize;

use crate::error::VeridictError;

#[derive(Debug, Clone, Deserialize)]
pub struct Record {
    pub id: Option<String>,
    pub baseline: Option<f64>,
    pub candidate: Option<f64>,
    pub result: Option<String>,
    pub baseline_status: Option<String>,
    pub candidate_status: Option<String>,
}

/// Parses one JSONL record per non-blank line. Blank lines are skipped
/// silently (they carry no data, so skipping them isn't "ignoring invalid
/// data"). Each item is `(1-based line number, Record)`; a malformed line
/// surfaces as `Err` rather than panicking or being dropped.
pub fn parse_jsonl<R: BufRead>(
    reader: R,
) -> impl Iterator<Item = Result<(usize, Record), VeridictError>> {
    reader.lines().enumerate().filter_map(|(idx, line)| {
        let line_no = idx + 1;
        let line = match line {
            Ok(l) => l,
            Err(source) => {
                return Some(Err(VeridictError::Io {
                    path: "<stream>".to_string(),
                    source,
                }));
            }
        };
        if line.trim().is_empty() {
            return None;
        }
        match serde_json::from_str::<Record>(&line) {
            Ok(record) => Some(Ok((line_no, record))),
            Err(source) => Some(Err(VeridictError::InvalidJson {
                line: line_no,
                source,
            })),
        }
    })
}

/// Parses one record per CSV row, using the header row to map columns onto
/// `Record`'s fields (same field names as the JSONL shape: id, baseline,
/// candidate, result, baseline_status, candidate_status). Empty cells
/// deserialize to `None`, same as an absent JSON field.
///
/// // ponytail: line numbers here are record indices (header=1, first data
/// // row=2), not physical file lines; a quoted field containing a newline
/// // will make this number diverge from `wc -l`. Upgrade to
/// // `csv::Error::position()` if that precision is ever needed.
pub fn parse_csv<R: Read>(
    reader: R,
) -> impl Iterator<Item = Result<(usize, Record), VeridictError>> {
    let rdr = csv::ReaderBuilder::new()
        .has_headers(true)
        .from_reader(reader);
    rdr.into_deserialize::<Record>()
        .enumerate()
        .map(|(idx, result)| {
            let line = idx + 2;
            match result {
                Ok(record) => Ok((line, record)),
                Err(source) => Err(VeridictError::InvalidCsv { line, source }),
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn parses_all_five_spec_shapes() {
        let input = concat!(
            "{\"id\":\"case-001\",\"baseline\":0.81,\"candidate\":0.84}\n",
            "{\"id\":\"case-002\",\"result\":\"candidate_win\"}\n",
            "{\"id\":\"case-003\",\"result\":\"draw\"}\n",
            "{\"id\":\"case-004\",\"baseline_status\":\"ok\",\"candidate_status\":\"timeout\"}\n",
            "{\"id\":\"case-005\",\"baseline_status\":\"ok\",\"candidate_status\":\"invalid\"}\n",
        );
        let records: Vec<_> = parse_jsonl(Cursor::new(input))
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(records.len(), 5);
        assert_eq!(records[0].1.baseline, Some(0.81));
        assert_eq!(records[1].1.result.as_deref(), Some("candidate_win"));
        assert_eq!(records[3].1.candidate_status.as_deref(), Some("timeout"));
    }

    #[test]
    fn blank_lines_skipped() {
        let input = "{\"id\":\"a\",\"result\":\"draw\"}\n\n\n{\"id\":\"b\",\"result\":\"draw\"}\n";
        let records: Vec<_> = parse_jsonl(Cursor::new(input))
            .collect::<Result<_, VeridictError>>()
            .unwrap();
        assert_eq!(records.len(), 2);
    }

    #[test]
    fn malformed_json_reports_line_number() {
        let input = "{\"id\":\"a\",\"result\":\"draw\"}\nnot json\n";
        let results: Vec<_> = parse_jsonl(Cursor::new(input)).collect();
        assert!(results[0].is_ok());
        match &results[1] {
            Err(VeridictError::InvalidJson { line, .. }) => assert_eq!(*line, 2),
            other => panic!("expected InvalidJson on line 2, got {other:?}"),
        }
    }

    #[test]
    fn parses_csv_with_empty_cells_as_none() {
        let input = "id,baseline,candidate,result,baseline_status,candidate_status\n\
                      case-001,0.81,0.84,,,\n\
                      case-002,,,candidate_win,,\n\
                      case-004,,,,ok,timeout\n";
        let records: Vec<_> = parse_csv(Cursor::new(input))
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(records.len(), 3);
        assert_eq!(records[0].1.baseline, Some(0.81));
        assert_eq!(records[0].1.result, None);
        assert_eq!(records[1].1.result.as_deref(), Some("candidate_win"));
        assert_eq!(records[2].1.candidate_status.as_deref(), Some("timeout"));
    }

    #[test]
    fn malformed_csv_reports_a_record_number() {
        let input = "id,baseline,candidate\ncase-001,not-a-number,0.5\n";
        let results: Vec<_> = parse_csv(Cursor::new(input)).collect();
        match &results[0] {
            Err(VeridictError::InvalidCsv { line, .. }) => assert_eq!(*line, 2),
            other => panic!("expected InvalidCsv on line 2, got {other:?}"),
        }
    }
}
