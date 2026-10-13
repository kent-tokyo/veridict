//! JSONL input parsing.
//!
//! Deliberately a single flat struct with `Option` fields rather than a
//! `#[serde(untagged)]` enum: untagged enums produce an opaque "data did not
//! match any variant" error on mismatch, which conflicts with the
//! requirement that bad input produce a clear, line-numbered error.

use std::io::{BufRead, Read};
use std::marker::PhantomData;

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

/// Head-to-head match between two named competitors, for `matrix --matches`.
/// A separate schema rather than an extension of `Record`: unlike `Record`,
/// there is no sparse/status-only variant to support here, so `a`/`b`/
/// `result` are required, not `Option`, and a record missing any of them is
/// unconditionally invalid.
#[derive(Debug, Clone, Deserialize)]
pub struct MatchRecord {
    pub id: Option<String>,
    pub a: String,
    pub b: String,
    pub result: String,
}

/// Parses one JSONL record per non-blank line. Blank lines are skipped
/// silently (they carry no data, so skipping them isn't "ignoring invalid
/// data"). Each item is `(1-based line number, T)`; a malformed line
/// surfaces as `Err` rather than panicking or being dropped. Generic over
/// `T` so this same body serves both `Record` (baseline/candidate schema)
/// and `MatchRecord` (named-competitor schema) - nothing here is
/// `Record`-specific.
pub fn parse_jsonl<T: serde::de::DeserializeOwned, R: BufRead>(
    reader: R,
) -> impl Iterator<Item = Result<(usize, T), VeridictError>> {
    JsonlIter {
        reader,
        buffer: Vec::with_capacity(8 * 1024),
        line_no: 0,
        finished: false,
        marker: PhantomData,
    }
}

/// Reuses one byte buffer across lines. `BufRead::lines` allocates a fresh `String` for every
/// record; this keeps allocation capacity while still returning owned, deserialized records.
struct JsonlIter<T, R> {
    reader: R,
    buffer: Vec<u8>,
    line_no: usize,
    finished: bool,
    marker: PhantomData<T>,
}

impl<T: serde::de::DeserializeOwned, R: BufRead> Iterator for JsonlIter<T, R> {
    type Item = Result<(usize, T), VeridictError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.finished {
            return None;
        }

        loop {
            self.buffer.clear();
            match self.reader.read_until(b'\n', &mut self.buffer) {
                Ok(0) => {
                    self.finished = true;
                    return None;
                }
                Ok(_) => self.line_no += 1,
                Err(source) => {
                    self.finished = true;
                    return Some(Err(VeridictError::Io {
                        path: "<stream>".to_string(),
                        source,
                    }));
                }
            }

            // ASCII JSON needs no separate UTF-8 pass. Preserve `BufRead::lines`'s InvalidData
            // behavior for malformed non-ASCII input, and its Unicode-whitespace blank-line rule.
            if self.buffer.iter().any(|byte| !byte.is_ascii()) {
                let line = match std::str::from_utf8(&self.buffer) {
                    Ok(line) => line,
                    Err(source) => {
                        return Some(Err(VeridictError::Io {
                            path: "<stream>".to_string(),
                            source: std::io::Error::new(std::io::ErrorKind::InvalidData, source),
                        }));
                    }
                };
                if line.trim().is_empty() {
                    continue;
                }
            } else if self.buffer.iter().all(|byte| byte.is_ascii_whitespace()) {
                continue;
            }

            return Some(
                serde_json::from_slice::<T>(&self.buffer)
                    .map(|record| (self.line_no, record))
                    .map_err(|source| VeridictError::InvalidJson {
                        line: self.line_no,
                        source,
                    }),
            );
        }
    }
}

/// Parses one record per CSV row, using the header row to map columns onto
/// `T`'s fields (e.g. for `Record`: id, baseline, candidate, result,
/// baseline_status, candidate_status). Empty cells deserialize to `None`,
/// same as an absent JSON field.
///
/// // ponytail: line numbers here are record indices (header=1, first data
/// // row=2), not physical file lines; a quoted field containing a newline
/// // will make this number diverge from `wc -l`. Upgrade to
/// // `csv::Error::position()` if that precision is ever needed.
pub fn parse_csv<T: serde::de::DeserializeOwned, R: Read>(
    reader: R,
) -> impl Iterator<Item = Result<(usize, T), VeridictError>> {
    let rdr = csv::ReaderBuilder::new()
        .has_headers(true)
        .from_reader(reader);
    rdr.into_deserialize::<T>()
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
        let records: Vec<_> = parse_jsonl::<Record, _>(Cursor::new(input))
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
        let records: Vec<_> = parse_jsonl::<Record, _>(Cursor::new(input))
            .collect::<Result<_, VeridictError>>()
            .unwrap();
        assert_eq!(records.len(), 2);
    }

    #[test]
    fn unicode_whitespace_only_lines_remain_blank() {
        let input =
            "{\"id\":\"a\",\"result\":\"draw\"}\n\u{2003}\n{\"id\":\"b\",\"result\":\"draw\"}\n";
        let records: Vec<_> = parse_jsonl::<Record, _>(Cursor::new(input))
            .collect::<Result<_, VeridictError>>()
            .unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[1].0, 3);
    }

    #[test]
    fn invalid_utf8_remains_an_io_error() {
        let input = [b'{', b'}', b'\n', 0xff, b'\n'];
        let results: Vec<_> = parse_jsonl::<Record, _>(Cursor::new(input)).collect();
        assert!(matches!(results[1], Err(VeridictError::Io { .. })));
    }

    #[test]
    fn malformed_json_reports_line_number() {
        let input = "{\"id\":\"a\",\"result\":\"draw\"}\nnot json\n";
        let results: Vec<_> = parse_jsonl::<Record, _>(Cursor::new(input)).collect();
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
        let records: Vec<_> = parse_csv::<Record, _>(Cursor::new(input))
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
        let results: Vec<_> = parse_csv::<Record, _>(Cursor::new(input)).collect();
        match &results[0] {
            Err(VeridictError::InvalidCsv { line, .. }) => assert_eq!(*line, 2),
            other => panic!("expected InvalidCsv on line 2, got {other:?}"),
        }
    }

    #[test]
    fn parses_match_record_jsonl() {
        let input = "{\"id\":\"m1\",\"a\":\"prompt_a\",\"b\":\"prompt_b\",\"result\":\"a_win\"}\n";
        let records: Vec<_> = parse_jsonl::<MatchRecord, _>(Cursor::new(input))
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].1.a, "prompt_a");
        assert_eq!(records[0].1.b, "prompt_b");
        assert_eq!(records[0].1.result, "a_win");
    }

    #[test]
    fn match_record_missing_a_field_is_invalid_json() {
        let input = "{\"b\":\"prompt_b\",\"result\":\"a_win\"}\n";
        let results: Vec<_> = parse_jsonl::<MatchRecord, _>(Cursor::new(input)).collect();
        assert!(matches!(
            results[0],
            Err(VeridictError::InvalidJson { line: 1, .. })
        ));
    }
}
