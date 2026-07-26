//! `verify-run`: checks that a completed run's raw data (a `manifest.toml` of declared
//! expectations plus a `games.jsonl` of per-record observations) is structurally sound *before*
//! any statistics (`compare`/`sprt`) are trusted on it - pairing, ordering, contamination, and
//! opaque-identifier consistency, not a metric or a verdict.
//!
//! Domain-agnostic by the same rule the rest of this crate follows (see `AGENTS.md`): every
//! hash/id field here is an opaque caller-supplied string, compared for equality/no-drift only -
//! this module never opens, hashes, or interprets an actual binary/weight/corpus file, and never
//! assigns domain meaning to a field's contents. `role` is the domain-agnostic stand-in for
//! "color/side reversed within a pair": two records sharing an `id` must declare two different
//! `role` values, whatever the caller's own convention for `role` is.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::Validity;
use crate::error::VeridictError;
use crate::metrics::{FailureBreakdown, tally_status};

/// Versions `Manifest`'s own (non-envelope) shape - distinct from `report::REPORT_SCHEMA_VERSION`
/// (which versions `VerifyRunReport`'s output shape) and from the envelope's own `schema_version`
/// field below (which versions the four-repo envelope contract, echoed but never validated by
/// veridict). Three different documents, three different version numbers.
pub const MANIFEST_SCHEMA_VERSION: u32 = 1;

/// Cap on how many `Violation`s `VerifyRunReport::violations` ever carries - a badly corrupted
/// run (e.g. every pair duplicated) could otherwise produce a report proportional to the whole
/// input. `violation_count` always holds the exact total regardless of this cap; the emitted
/// sample is deterministic (the first `MAX_REPORTED_VIOLATIONS` after `run`'s own sort), not a
/// random subset, so the same input always reports the same sample.
const MAX_REPORTED_VIOLATIONS: usize = 500;

/// A run's declared expectations, read from `manifest.toml`. Every envelope field (see the
/// module doc) is `Option` and opaque - compared for consistency against `games.jsonl`, never
/// computed or interpreted. `#[serde(deny_unknown_fields)]` here (unlike `VerifyRunRecord`)
/// because a typo'd manifest key must not silently skip a check the caller thought they'd
/// enabled - manifest fields are more like a declared expectation than data, and this project's
/// convention is to never silently ignore invalid data.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    /// Required; mismatch vs `MANIFEST_SCHEMA_VERSION` is a hard error (see `parse_manifest`),
    /// not a violation - an unrecognized manifest shape can't be safely checked at all.
    pub manifest_schema_version: u32,

    // Experiment envelope fields (shared four-repo contract) - opaque, never interpreted, only
    // compared or echoed.
    /// Drives `check_experiment_contamination`.
    pub experiment_id: Option<String>,
    pub candidate_id: Option<String>,
    pub baseline_id: Option<String>,
    pub lineage_id: Option<String>,
    /// Drives one independent sub-check of `check_environment_consistency`.
    pub dataset_sha256: Option<String>,
    /// Passthrough only - no per-record counterpart this round.
    pub split_sha256: Option<String>,
    /// Passthrough only - no per-record counterpart this round.
    pub teacher_manifest_sha256: Option<String>,
    /// Drives one independent sub-check of `check_environment_consistency`.
    pub binary_sha256: Option<String>,
    /// Drives one independent sub-check of `check_environment_consistency`.
    pub weight_sha256: Option<String>,
    pub init_seed: Option<i64>,
    pub split_seed: Option<i64>,
    pub shuffle_seed: Option<i64>,
    /// Pure passthrough echo of the envelope's own `validity` slot - deliberately never feeds
    /// this report's own computed `validity` (which is purely `violations.is_empty()`). Whatever
    /// upstream stage populated this is its own claim, not something verify-run adopts as its own.
    pub validity: Option<Validity>,
    /// The ENVELOPE's own version tag (one of the 14 shared fields) - opaque passthrough, never
    /// validated by veridict. Deliberately a different field from `manifest_schema_version`
    /// above: that one is veridict-specific and enforced; this one is the four-repo envelope's
    /// own version, echoed but never interpreted, same as every other envelope field.
    pub schema_version: Option<String>,

    // Non-envelope, verify-run-specific:
    /// Drives one independent sub-check of `check_environment_consistency` - not part of the
    /// original envelope; closes the "no mid-run config change" check (time control, thread
    /// count, etc. - anything that drifts without touching a hashed artifact file).
    pub config_sha256: Option<String>,
    /// Declared order of gated pair `id`s. Drives `check_schedule_order`.
    pub schedule: Option<Vec<String>>,
}

impl Manifest {
    /// Whether any manifest-dependent check has something to check against - the three
    /// manifest-independent checks (`check_pair_completeness`/`check_global_index_uniqueness`/
    /// `check_role_consistency`) always run regardless and don't count toward this.
    fn has_anything_to_verify(&self) -> bool {
        self.schedule.is_some()
            || self.experiment_id.is_some()
            || self.dataset_sha256.is_some()
            || self.binary_sha256.is_some()
            || self.weight_sha256.is_some()
            || self.config_sha256.is_some()
    }
}

/// Parses and validates a `manifest.toml`'s text. `path` is used only for the error message (a
/// caller reading from a real file passes its path; a caller reading from an in-memory string
/// for a test can pass any label).
pub fn parse_manifest(path: &str, text: &str) -> Result<Manifest, VeridictError> {
    let manifest: Manifest = toml::from_str(text).map_err(|source| VeridictError::InvalidToml {
        path: path.to_string(),
        source,
    })?;
    if manifest.manifest_schema_version != MANIFEST_SCHEMA_VERSION {
        return Err(VeridictError::UnsupportedManifestSchemaVersion {
            found: manifest.manifest_schema_version,
            supported: MANIFEST_SCHEMA_VERSION,
        });
    }
    Ok(manifest)
}

/// One `games.jsonl` record for `verify-run`. A genuinely separate schema from `input::Record`:
/// verify-run computes no metric (no `baseline`/`candidate`/`result` numeric fields), and needs
/// several fields `Record` has no room for (`global_index`, `role`, `include_in_gate`, the
/// per-record envelope-subset fields). No `#[serde(deny_unknown_fields)]` here (unlike
/// `Manifest`): an unknown key on a data record is harmless extra caller metadata, matching
/// `input::Record`'s existing precedent.
#[derive(Debug, Clone, Deserialize)]
pub struct VerifyRunRecord {
    /// Pairing key. A non-paired record simply omits it.
    pub id: Option<String>,
    /// Caller-declared per-record sequence number. Drives `check_global_index_uniqueness`.
    pub global_index: Option<u64>,
    /// Opaque orientation tag - the domain-agnostic stand-in for "color/side reversed". Must
    /// differ within a pair (see `check_role_consistency`); whatever "role" means is the
    /// caller's business.
    pub role: Option<String>,
    /// `None`/`Some(true)` = gated (counts toward `check_schedule_order`); `Some(false)` =
    /// burn-in, excluded from the gated schedule order.
    pub include_in_gate: Option<bool>,
    pub experiment_id: Option<String>,
    pub dataset_sha256: Option<String>,
    pub binary_sha256: Option<String>,
    pub weight_sha256: Option<String>,
    pub config_sha256: Option<String>,
    /// `ok|timeout|crash|invalid` - reuses `TrialStatus`/`tally_status` verbatim.
    pub baseline_status: Option<String>,
    pub candidate_status: Option<String>,
}

/// One structural defect found in the run. `lines` is plural (unlike
/// `VeridictError::SchemaMismatch`'s single `line`) because most violations here are relational -
/// they implicate two or more records, not one.
#[derive(Debug, Serialize)]
pub struct Violation {
    pub check: &'static str,
    pub id: Option<String>,
    pub lines: Vec<usize>,
    pub field: Option<&'static str>,
    pub detail: String,
}

/// A check that had nothing to check against - structured and machine-readable, not just a
/// prose warning, so a caller can tell "this passed" from "this wasn't evaluated" without
/// parsing text. Reported instead of a silent pass (false reassurance) or a hard failure
/// (unusable until every optional field is emitted).
#[derive(Debug, Serialize)]
pub struct SkippedCheck {
    pub check: &'static str,
    pub field: Option<&'static str>,
    pub reason: String,
}

#[derive(Debug, Serialize)]
pub struct VerifyRunReport {
    pub schema_version: u32,
    /// `Valid` iff `violations` is empty. Reuses the existing `Validity` enum directly rather
    /// than inventing a parallel type - no `Verdict`-shaped three-way concept applies here (a
    /// structural invariant either holds or it doesn't; there's no "inconclusive").
    pub validity: Validity,
    pub reason: String,
    /// Sorted deterministically (see `run`'s doc), then capped at `MAX_REPORTED_VIOLATIONS` -
    /// see `violation_count`/`violations_truncated` for the exact total when this list was cut
    /// short.
    pub violations: Vec<Violation>,
    /// Exact total violation count, even when `violations` above was truncated - `validity`/
    /// `reason` are derived from this, never from `violations.len()`.
    pub violation_count: u64,
    /// `true` iff `violation_count > MAX_REPORTED_VIOLATIONS`, i.e. `violations` above is a
    /// deterministic (first-`MAX_REPORTED_VIOLATIONS`-after-sort) sample, not the full list.
    pub violations_truncated: bool,
    pub checks_skipped: Vec<SkippedCheck>,
    /// Human-readable mirror of `checks_skipped`, same "one computation, two representations"
    /// idiom `report::DataQuality`/`warnings` already use elsewhere.
    pub warnings: Vec<String>,
    pub record_count: u64,
    /// Count of `id`-groups with exactly 2 records, excluding burn-in pairs (either record
    /// marked `include_in_gate: false`) - the formal, gated pair count. Distinct from
    /// `SprtReport::paired_count`, which counts pairs a pentanomial LLR walk actually analyzed.
    pub paired_count: u64,
    pub timeouts: u64,
    pub crashes: u64,
    pub invalid: u64,
    pub failure_breakdown: FailureBreakdown,
    /// The whole manifest, echoed back verbatim.
    pub manifest: Manifest,
}

/// Every `id`-group among `records` that carries a non-`None` `id`, regardless of
/// `include_in_gate` - pairing is a property of `id`, not of gating, so a burn-in record that
/// unexpectedly reuses a gated pair's `id` legitimately trips `check_pair_completeness`/
/// `check_role_consistency`. `BTreeMap`, not `HashMap`: iteration order must be
/// process-independent (see `run`'s doc on why the final violation order is sorted).
fn group_by_id(
    records: &[(usize, VerifyRunRecord)],
) -> BTreeMap<String, Vec<(usize, &VerifyRunRecord)>> {
    let mut groups: BTreeMap<String, Vec<(usize, &VerifyRunRecord)>> = BTreeMap::new();
    for (line, record) in records {
        if let Some(id) = &record.id {
            groups.entry(id.clone()).or_default().push((*line, record));
        }
    }
    groups
}

/// Merges the original request's "#2 pair IDs appear exactly twice" and "#5 no incomplete
/// pairs" - identical mechanism, one check.
fn check_pair_completeness(
    groups: &BTreeMap<String, Vec<(usize, &VerifyRunRecord)>>,
) -> Vec<Violation> {
    groups
        .iter()
        .filter(|(_, records)| records.len() != 2)
        .map(|(id, records)| Violation {
            check: "pair_completeness",
            id: Some(id.clone()),
            lines: records.iter().map(|(line, _)| *line).collect(),
            field: Some("id"),
            detail: format!(
                "id '{id}' appears {} time(s); pairing requires exactly 2 records per id",
                records.len()
            ),
        })
        .collect()
}

/// Merges the original request's "#4 no duplicate global index".
fn check_global_index_uniqueness(
    records: &[(usize, VerifyRunRecord)],
) -> (Vec<Violation>, Option<SkippedCheck>) {
    let mut by_index: BTreeMap<u64, Vec<usize>> = BTreeMap::new();
    for (line, record) in records {
        if let Some(index) = record.global_index {
            by_index.entry(index).or_default().push(*line);
        }
    }
    if by_index.is_empty() {
        return (
            Vec::new(),
            Some(SkippedCheck {
                check: "global_index_uniqueness",
                field: Some("global_index"),
                reason: "no record declared global_index".to_string(),
            }),
        );
    }
    let violations = by_index
        .into_iter()
        .filter(|(_, lines)| lines.len() > 1)
        .map(|(index, lines)| Violation {
            check: "global_index_uniqueness",
            id: None,
            lines,
            field: Some("global_index"),
            detail: format!("global_index {index} appears on more than one record"),
        })
        .collect();
    (violations, None)
}

/// Merges the original request's "#1 candidate/baseline orientation" and "#3 color/side
/// reversal confirmed" - both are the same underlying `role` field, checked two ways: within a
/// pair (must differ) and across the whole run (only one role-pairing convention should be in
/// use). A pair whose completeness `check_pair_completeness` already flagged (size != 2) is
/// skipped here rather than double-reported for the same underlying defect.
fn check_role_consistency(
    groups: &BTreeMap<String, Vec<(usize, &VerifyRunRecord)>>,
) -> (Vec<Violation>, Option<SkippedCheck>) {
    let mut violations = Vec::new();
    let mut role_pairings: BTreeMap<(String, String), Vec<usize>> = BTreeMap::new();
    let mut any_role_seen = false;

    for (id, records) in groups {
        if records.len() != 2 {
            continue;
        }
        let (line_a, a) = records[0];
        let (line_b, b) = records[1];
        let (Some(role_a), Some(role_b)) = (&a.role, &b.role) else {
            continue;
        };
        any_role_seen = true;
        if role_a == role_b {
            violations.push(Violation {
                check: "role_consistency",
                id: Some(id.clone()),
                lines: vec![line_a, line_b],
                field: Some("role"),
                detail: format!(
                    "pair '{id}' has the same role ('{role_a}') on both records; a valid pair \
                     requires two different roles"
                ),
            });
            continue;
        }
        if a.include_in_gate == Some(false) || b.include_in_gate == Some(false) {
            // Burn-in pairs may legitimately use a different role vocabulary than gated
            // pairs; only gated pairs vote on the run's single "expected" role pairing.
            continue;
        }
        let mut combo = [role_a.clone(), role_b.clone()];
        combo.sort();
        let [first, second] = combo;
        role_pairings
            .entry((first, second))
            .or_default()
            .push(line_a);
    }

    if !any_role_seen {
        return (
            violations,
            Some(SkippedCheck {
                check: "role_consistency",
                field: Some("role"),
                reason: "no record declared role".to_string(),
            }),
        );
    }

    if role_pairings.len() > 1 {
        let combos: Vec<String> = role_pairings
            .keys()
            .map(|(a, b)| format!("{{{a}, {b}}}"))
            .collect();
        violations.push(Violation {
            check: "role_consistency",
            id: None,
            lines: role_pairings.values().flatten().copied().collect(),
            field: Some("role"),
            detail: format!(
                "more than one distinct role pairing is used across the run: {}",
                combos.join(", ")
            ),
        });
    }

    (violations, None)
}

/// The original request's "#6 permutation order match" plus the set-membership half of "#9 no
/// burn-in/other-run contamination" (a full ordered-sequence-equality check already implies set
/// equality, so a separate membership check would be redundant).
fn check_schedule_order(
    manifest: &Manifest,
    records: &[(usize, VerifyRunRecord)],
) -> (Vec<Violation>, Option<SkippedCheck>) {
    let Some(schedule) = &manifest.schedule else {
        return (
            Vec::new(),
            Some(SkippedCheck {
                check: "schedule_order",
                field: Some("schedule"),
                reason: "manifest.schedule not declared".to_string(),
            }),
        );
    };

    let mut seen = std::collections::HashSet::new();
    let mut actual = Vec::new();
    for (_, record) in records {
        if record.include_in_gate == Some(false) {
            continue;
        }
        if let Some(id) = &record.id
            && seen.insert(id.clone())
        {
            actual.push(id.clone());
        }
    }

    if &actual == schedule {
        return (Vec::new(), None);
    }
    let mismatch_at = actual
        .iter()
        .zip(schedule.iter())
        .position(|(a, e)| a != e)
        .unwrap_or_else(|| actual.len().min(schedule.len()));
    let describe = |ids: &[String], at: usize| {
        ids.get(at)
            .map(|id| format!("'{id}'"))
            .unwrap_or_else(|| "<end of list>".to_string())
    };
    (
        vec![Violation {
            check: "schedule_order",
            id: None,
            lines: Vec::new(),
            field: Some("schedule"),
            detail: format!(
                "actual gated pair order diverges from manifest.schedule at position \
                 {mismatch_at} (expected {}, got {}); full expected order: {schedule:?}, full \
                 actual order: {actual:?}",
                describe(schedule, mismatch_at),
                describe(&actual, mismatch_at),
            ),
        }],
        None,
    )
}

/// The other half of "#9 no burn-in/other-run contamination": a record whose declared
/// `experiment_id` disagrees with the manifest's.
fn check_experiment_contamination(
    manifest: &Manifest,
    records: &[(usize, VerifyRunRecord)],
) -> (Vec<Violation>, Option<SkippedCheck>) {
    let Some(expected) = &manifest.experiment_id else {
        return (
            Vec::new(),
            Some(SkippedCheck {
                check: "experiment_contamination",
                field: Some("experiment_id"),
                reason: "manifest.experiment_id not declared".to_string(),
            }),
        );
    };
    let violations = records
        .iter()
        .filter_map(|(line, record)| {
            let actual = record.experiment_id.as_ref()?;
            (actual != expected).then(|| Violation {
                check: "experiment_contamination",
                id: record.id.clone(),
                lines: vec![*line],
                field: Some("experiment_id"),
                detail: format!(
                    "record's experiment_id '{actual}' does not match manifest's '{expected}'"
                ),
            })
        })
        .collect();
    (violations, None)
}

/// One opaque hash/identifier field checked for drift between the manifest's declared value and
/// every record that repeats it - the mechanism shared by all four sub-checks in
/// `check_environment_consistency`.
fn check_one_hash_field(
    field: &'static str,
    expected: &Option<String>,
    records: &[(usize, VerifyRunRecord)],
    get: impl Fn(&VerifyRunRecord) -> &Option<String>,
) -> (Vec<Violation>, Option<SkippedCheck>) {
    let Some(expected) = expected else {
        return (
            Vec::new(),
            Some(SkippedCheck {
                check: "environment_consistency",
                field: Some(field),
                reason: format!("manifest.{field} not declared"),
            }),
        );
    };
    let violations = records
        .iter()
        .filter_map(|(line, record)| {
            let actual = get(record).as_ref()?;
            (actual != expected).then(|| Violation {
                check: "environment_consistency",
                id: record.id.clone(),
                lines: vec![*line],
                field: Some(field),
                detail: format!(
                    "record's {field} '{actual}' does not match manifest's declared '{expected}'"
                ),
            })
        })
        .collect();
    (violations, None)
}

/// Merges the original request's "#7 binary/weight/corpus hash consistency" and "#8 no mid-run
/// config change" - `config_sha256` (not part of the original envelope) closes the config-drift
/// gap the hash fields alone can't: engine configuration (time control, thread count, search
/// depth) that never touches a hashed artifact file, exactly the kind of drift that motivated
/// this feature.
type HashFieldGetter = fn(&VerifyRunRecord) -> &Option<String>;

fn check_environment_consistency(
    manifest: &Manifest,
    records: &[(usize, VerifyRunRecord)],
) -> (Vec<Violation>, Vec<SkippedCheck>) {
    let fields: [(&'static str, &Option<String>, HashFieldGetter); 4] = [
        ("dataset_sha256", &manifest.dataset_sha256, |r| {
            &r.dataset_sha256
        }),
        ("binary_sha256", &manifest.binary_sha256, |r| {
            &r.binary_sha256
        }),
        ("weight_sha256", &manifest.weight_sha256, |r| {
            &r.weight_sha256
        }),
        ("config_sha256", &manifest.config_sha256, |r| {
            &r.config_sha256
        }),
    ];
    let mut violations = Vec::new();
    let mut skipped = Vec::new();
    for (field, expected, get) in fields {
        let (v, s) = check_one_hash_field(field, expected, records, get);
        violations.extend(v);
        skipped.extend(s);
    }
    (violations, skipped)
}

/// The original request's "#10 invalid/timeout/protocol-error counts" - `protocol error` maps
/// onto the existing `invalid` status rather than a new domain-specific status variant; an
/// unrecognized status string is still the existing hard `VeridictError::UnrecognizedStatus`.
fn tally_failures(records: &[(usize, VerifyRunRecord)]) -> Result<FailureBreakdown, VeridictError> {
    let mut failures = FailureBreakdown::default();
    for (line, record) in records {
        if let Some(status) = record.baseline_status.as_deref() {
            tally_status(status, *line, "baseline_status", &mut failures.baseline)?;
        }
        if let Some(status) = record.candidate_status.as_deref() {
            tally_status(status, *line, "candidate_status", &mut failures.candidate)?;
        }
    }
    Ok(failures)
}

/// Runs every check unconditionally over the whole buffered record set, collecting *all*
/// violations rather than failing fast on the first one - the same rationale
/// `FailureBreakdown`/`DataQuality` already establish: the point of this command is diagnosing
/// everything wrong with a run in one report, not being told about one defect and having to
/// re-run to find the next.
///
/// `records` is buffered whole (not streamed) deliberately: every check here is inherently
/// cross-record (pairing needs every record sharing an `id` seen together, schedule-order needs
/// the complete first-occurrence sequence, global-index dedup needs every value at once), so
/// streaming genuinely isn't practical the way it is for `compare`/`sprt`'s incremental tallies.
///
/// Violation order is sorted by `(first line, check, field, id)` before the report is built -
/// `BTreeMap`-based grouping alone isn't sufficient, since concatenating 6 independently-ordered
/// check outputs still needs a final total order. This determinism is load-bearing for golden
/// fixture tests: any process-order-dependent ordering would make them intermittently flaky.
pub fn run(
    manifest: &Manifest,
    records: Vec<(usize, VerifyRunRecord)>,
) -> Result<VerifyRunReport, VeridictError> {
    if records.is_empty() {
        return Err(VeridictError::EmptyInput);
    }
    if !manifest.has_anything_to_verify() {
        return Err(VeridictError::ManifestDeclaresNothingToVerify);
    }

    let groups = group_by_id(&records);

    let mut violations = check_pair_completeness(&groups);
    let mut checks_skipped = Vec::new();

    let (v, s) = check_global_index_uniqueness(&records);
    violations.extend(v);
    checks_skipped.extend(s);

    let (v, s) = check_role_consistency(&groups);
    violations.extend(v);
    checks_skipped.extend(s);

    let (v, s) = check_schedule_order(manifest, &records);
    violations.extend(v);
    checks_skipped.extend(s);

    let (v, s) = check_experiment_contamination(manifest, &records);
    violations.extend(v);
    checks_skipped.extend(s);

    let (v, s) = check_environment_consistency(manifest, &records);
    violations.extend(v);
    checks_skipped.extend(s);

    violations.sort_by(|a, b| {
        let key = |v: &Violation| {
            (
                v.lines.first().copied().unwrap_or(usize::MAX),
                v.check,
                v.field.unwrap_or(""),
                v.id.clone().unwrap_or_default(),
            )
        };
        key(a).cmp(&key(b))
    });

    let failure_breakdown = tally_failures(&records)?;
    // Excludes burn-in pairs (either record marked `include_in_gate: false`) - this is the
    // *formal* (gated) pair count, the same convention `check_role_consistency` uses to keep
    // burn-in's own role vocabulary from voting on the gated run's pairing convention.
    let paired_count = groups
        .values()
        .filter(|g| {
            g.len() == 2
                && g[0].1.include_in_gate != Some(false)
                && g[1].1.include_in_gate != Some(false)
        })
        .count() as u64;
    let warnings = checks_skipped
        .iter()
        .map(|s| format!("{}: {}", s.check, s.reason))
        .collect();

    let validity = if violations.is_empty() {
        Validity::Valid
    } else {
        Validity::Invalid
    };
    let violation_count = violations.len() as u64;
    let violations_truncated = violations.len() > MAX_REPORTED_VIOLATIONS;
    let reason = if violations.is_empty() {
        "No structural violations found.".to_string()
    } else {
        format!(
            "{violation_count} structural violation(s) found; see `violations`.{}",
            if violations_truncated {
                format!(" (showing first {MAX_REPORTED_VIOLATIONS})")
            } else {
                String::new()
            }
        )
    };
    violations.truncate(MAX_REPORTED_VIOLATIONS);

    Ok(VerifyRunReport {
        schema_version: crate::report::REPORT_SCHEMA_VERSION,
        validity,
        reason,
        violations,
        violation_count,
        violations_truncated,
        checks_skipped,
        warnings,
        record_count: records.len() as u64,
        paired_count,
        timeouts: failure_breakdown.baseline.timeout + failure_breakdown.candidate.timeout,
        crashes: failure_breakdown.baseline.crash + failure_breakdown.candidate.crash,
        invalid: failure_breakdown.baseline.invalid + failure_breakdown.candidate.invalid,
        failure_breakdown,
        manifest: manifest.clone(),
    })
}

impl VerifyRunReport {
    pub fn to_json_pretty(&self) -> String {
        serde_json::to_string_pretty(self).expect(
            "VerifyRunReport contains only finite fields and strings; serialization cannot fail",
        )
    }

    pub fn to_markdown(&self) -> String {
        let b = &self.failure_breakdown.baseline;
        let c = &self.failure_breakdown.candidate;
        let mut violations = String::new();
        for v in &self.violations {
            violations.push_str(&format!(
                "- [{}] {} (lines: {:?})\n",
                v.check, v.detail, v.lines
            ));
        }
        let mut skipped = String::new();
        for s in &self.checks_skipped {
            skipped.push_str(&format!("- {}: {}\n", s.check, s.reason));
        }
        format!(
            "# Veridict Verify-Run Report\n\n\
             Validity: {validity}\n\n\
             {reason}\n\n\
             Violations ({violation_count}{truncated_note}):\n{violations}\n\
             Checks skipped ({skipped_count}):\n{skipped}\n\
             Records: {record_count}, paired: {paired_count}\n\
             Status counts:\n\
             - timeout: {timeouts} (baseline={b_timeout}, candidate={c_timeout})\n\
             - crash: {crashes} (baseline={b_crash}, candidate={c_crash})\n\
             - invalid: {invalid} (baseline={b_invalid}, candidate={c_invalid})\n",
            validity = crate::report::serde_str(&self.validity),
            reason = self.reason,
            violation_count = self.violation_count,
            truncated_note = if self.violations_truncated {
                format!(", showing first {}", self.violations.len())
            } else {
                String::new()
            },
            skipped_count = self.checks_skipped.len(),
            record_count = self.record_count,
            paired_count = self.paired_count,
            timeouts = self.timeouts,
            crashes = self.crashes,
            invalid = self.invalid,
            b_timeout = b.timeout,
            c_timeout = c.timeout,
            b_crash = b.crash,
            c_crash = c.crash,
            b_invalid = b.invalid,
            c_invalid = c.invalid,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_manifest() -> Manifest {
        Manifest {
            manifest_schema_version: MANIFEST_SCHEMA_VERSION,
            experiment_id: None,
            candidate_id: None,
            baseline_id: None,
            lineage_id: None,
            dataset_sha256: None,
            split_sha256: None,
            teacher_manifest_sha256: None,
            binary_sha256: None,
            weight_sha256: None,
            init_seed: None,
            split_seed: None,
            shuffle_seed: None,
            validity: None,
            schema_version: None,
            config_sha256: None,
            schedule: None,
        }
    }

    fn record(id: &str, role: &str) -> VerifyRunRecord {
        VerifyRunRecord {
            id: Some(id.to_string()),
            global_index: None,
            role: Some(role.to_string()),
            include_in_gate: None,
            experiment_id: None,
            dataset_sha256: None,
            binary_sha256: None,
            weight_sha256: None,
            config_sha256: None,
            baseline_status: None,
            candidate_status: None,
        }
    }

    fn clean_pairs(n: usize) -> Vec<(usize, VerifyRunRecord)> {
        (0..n)
            .flat_map(|i| {
                let id = format!("op{i}");
                [(i * 2 + 1, record(&id, "a")), (i * 2 + 2, record(&id, "b"))]
            })
            .collect()
    }

    // --- parse_manifest ---

    #[test]
    fn parse_manifest_reads_a_minimal_valid_manifest() {
        let manifest = parse_manifest("m.toml", "manifest_schema_version = 1\n").unwrap();
        assert_eq!(manifest.manifest_schema_version, 1);
        assert_eq!(manifest.experiment_id, None);
    }

    #[test]
    fn parse_manifest_rejects_malformed_toml() {
        assert!(matches!(
            parse_manifest("m.toml", "not = [valid\n"),
            Err(VeridictError::InvalidToml { .. })
        ));
    }

    #[test]
    fn parse_manifest_rejects_unsupported_schema_version() {
        assert!(matches!(
            parse_manifest("m.toml", "manifest_schema_version = 999\n"),
            Err(VeridictError::UnsupportedManifestSchemaVersion {
                found: 999,
                supported: 1
            })
        ));
    }

    #[test]
    fn parse_manifest_rejects_unknown_fields() {
        assert!(matches!(
            parse_manifest(
                "m.toml",
                "manifest_schema_version = 1\ntypo_field = \"oops\"\n"
            ),
            Err(VeridictError::InvalidToml { .. })
        ));
    }

    // --- run: happy path ---

    #[test]
    fn clean_run_with_matching_schedule_is_valid() {
        let mut manifest = empty_manifest();
        manifest.schedule = Some(vec!["op0".to_string(), "op1".to_string()]);
        let records = clean_pairs(2);
        let report = run(&manifest, records).unwrap();
        assert_eq!(report.validity, Validity::Valid);
        assert!(report.violations.is_empty());
        assert_eq!(report.paired_count, 2);
        assert_eq!(report.record_count, 4);
    }

    // --- run: guard errors ---

    #[test]
    fn empty_records_is_an_error() {
        let mut manifest = empty_manifest();
        manifest.experiment_id = Some("exp1".to_string());
        assert!(matches!(
            run(&manifest, Vec::new()),
            Err(VeridictError::EmptyInput)
        ));
    }

    #[test]
    fn manifest_declaring_nothing_is_an_error() {
        let manifest = empty_manifest();
        let records = clean_pairs(1);
        assert!(matches!(
            run(&manifest, records),
            Err(VeridictError::ManifestDeclaresNothingToVerify)
        ));
    }

    // --- check_pair_completeness ---

    #[test]
    fn incomplete_pair_is_a_violation() {
        let mut manifest = empty_manifest();
        manifest.experiment_id = Some("exp1".to_string());
        let mut records = clean_pairs(2);
        records.push((100, record("lonely", "a")));
        let report = run(&manifest, records).unwrap();
        assert_eq!(report.validity, Validity::Invalid);
        assert!(
            report
                .violations
                .iter()
                .any(|v| v.check == "pair_completeness" && v.id.as_deref() == Some("lonely"))
        );
    }

    #[test]
    fn tripled_id_is_a_violation() {
        let mut manifest = empty_manifest();
        manifest.experiment_id = Some("exp1".to_string());
        let mut records = clean_pairs(1);
        records.push((100, record("op0", "a")));
        let report = run(&manifest, records).unwrap();
        assert!(
            report
                .violations
                .iter()
                .any(|v| v.check == "pair_completeness" && v.lines.len() == 3)
        );
    }

    // --- check_role_consistency ---

    #[test]
    fn same_role_on_both_pair_members_is_a_violation() {
        let mut manifest = empty_manifest();
        manifest.experiment_id = Some("exp1".to_string());
        let records = vec![(1, record("op0", "a")), (2, record("op0", "a"))];
        let report = run(&manifest, records).unwrap();
        assert!(
            report
                .violations
                .iter()
                .any(|v| v.check == "role_consistency" && v.id.as_deref() == Some("op0"))
        );
    }

    #[test]
    fn inconsistent_role_pairing_across_run_is_a_violation() {
        let mut manifest = empty_manifest();
        manifest.experiment_id = Some("exp1".to_string());
        let mut records = clean_pairs(3); // all pairs use {a, b}
        records.push((100, record("odd", "x")));
        records.push((101, record("odd", "y"))); // a different {x, y} pairing
        let report = run(&manifest, records).unwrap();
        assert!(
            report
                .violations
                .iter()
                .any(|v| v.check == "role_consistency"
                    && v.detail.contains("more than one distinct role pairing"))
        );
    }

    #[test]
    fn burn_in_pair_with_a_different_role_vocabulary_is_not_a_violation() {
        let mut manifest = empty_manifest();
        manifest.experiment_id = Some("exp1".to_string());
        let mut records = clean_pairs(3); // all gated pairs use {a, b}
        records.push((
            100,
            VerifyRunRecord {
                include_in_gate: Some(false),
                ..record("burn_in", "x")
            },
        ));
        records.push((
            101,
            VerifyRunRecord {
                include_in_gate: Some(false),
                ..record("burn_in", "y")
            },
        ));
        let report = run(&manifest, records).unwrap();
        assert!(
            !report
                .violations
                .iter()
                .any(|v| v.check == "role_consistency")
        );
    }

    #[test]
    fn role_consistency_is_skipped_when_no_record_declares_role() {
        let mut manifest = empty_manifest();
        manifest.experiment_id = Some("exp1".to_string());
        let records: Vec<_> = (0..2)
            .flat_map(|i| {
                let id = format!("op{i}");
                [
                    (
                        i * 2 + 1,
                        VerifyRunRecord {
                            role: None,
                            ..record(&id, "unused")
                        },
                    ),
                    (
                        i * 2 + 2,
                        VerifyRunRecord {
                            role: None,
                            ..record(&id, "unused")
                        },
                    ),
                ]
            })
            .collect();
        let report = run(&manifest, records).unwrap();
        assert!(
            report
                .checks_skipped
                .iter()
                .any(|s| s.check == "role_consistency")
        );
        assert!(
            !report
                .violations
                .iter()
                .any(|v| v.check == "role_consistency")
        );
    }

    // --- check_global_index_uniqueness ---

    #[test]
    fn duplicate_global_index_is_a_violation() {
        let mut manifest = empty_manifest();
        manifest.experiment_id = Some("exp1".to_string());
        let records = vec![
            (
                1,
                VerifyRunRecord {
                    global_index: Some(7),
                    ..record("op0", "a")
                },
            ),
            (
                2,
                VerifyRunRecord {
                    global_index: Some(7),
                    ..record("op0", "b")
                },
            ),
        ];
        let report = run(&manifest, records).unwrap();
        assert!(
            report
                .violations
                .iter()
                .any(|v| v.check == "global_index_uniqueness")
        );
    }

    #[test]
    fn violations_beyond_the_cap_are_counted_but_not_all_emitted() {
        let mut manifest = empty_manifest();
        manifest.experiment_id = Some("exp1".to_string());
        // 600 distinct duplicated global_index values - one violation each, well past
        // MAX_REPORTED_VIOLATIONS (500).
        let records: Vec<_> = (0..600u64)
            .flat_map(|i| {
                let id = format!("op{i}");
                [
                    (
                        (i * 2 + 1) as usize,
                        VerifyRunRecord {
                            global_index: Some(i),
                            ..record(&id, "a")
                        },
                    ),
                    (
                        (i * 2 + 2) as usize,
                        VerifyRunRecord {
                            global_index: Some(i),
                            ..record(&id, "b")
                        },
                    ),
                ]
            })
            .collect();
        let report = run(&manifest, records).unwrap();
        assert_eq!(report.validity, Validity::Invalid);
        assert_eq!(report.violation_count, 600);
        assert!(report.violations_truncated);
        assert_eq!(report.violations.len(), 500);
        assert!(report.reason.contains("600"));
    }

    // --- check_schedule_order ---

    #[test]
    fn schedule_mismatch_is_a_violation() {
        let mut manifest = empty_manifest();
        manifest.schedule = Some(vec!["op1".to_string(), "op0".to_string()]); // reversed
        let records = clean_pairs(2);
        let report = run(&manifest, records).unwrap();
        assert!(
            report
                .violations
                .iter()
                .any(|v| v.check == "schedule_order")
        );
    }

    #[test]
    fn burn_in_records_are_excluded_from_schedule_order() {
        let mut manifest = empty_manifest();
        manifest.schedule = Some(vec!["op0".to_string()]);
        let mut records = clean_pairs(1);
        records.push((
            100,
            VerifyRunRecord {
                include_in_gate: Some(false),
                ..record("burn_in", "a")
            },
        ));
        records.push((
            101,
            VerifyRunRecord {
                include_in_gate: Some(false),
                ..record("burn_in", "b")
            },
        ));
        let report = run(&manifest, records).unwrap();
        assert!(
            !report
                .violations
                .iter()
                .any(|v| v.check == "schedule_order")
        );
        // The burn-in pair still counts toward record_count/pair-completeness checking, but not
        // toward the formal (gated) paired_count.
        assert_eq!(report.paired_count, 1);
        assert_eq!(report.record_count, 4);
    }

    // --- check_experiment_contamination ---

    #[test]
    fn mismatched_experiment_id_is_a_violation() {
        let mut manifest = empty_manifest();
        manifest.experiment_id = Some("exp1".to_string());
        let mut records = clean_pairs(1);
        records.push((
            100,
            VerifyRunRecord {
                experiment_id: Some("exp2".to_string()),
                ..record("contaminant", "a")
            },
        ));
        let report = run(&manifest, records).unwrap();
        assert!(
            report
                .violations
                .iter()
                .any(|v| v.check == "experiment_contamination")
        );
    }

    // --- check_environment_consistency ---

    #[test]
    fn hash_drift_is_a_violation() {
        let mut manifest = empty_manifest();
        manifest.experiment_id = Some("exp1".to_string());
        manifest.binary_sha256 = Some("abc123".to_string());
        let mut records = clean_pairs(1);
        records.push((
            100,
            VerifyRunRecord {
                binary_sha256: Some("different".to_string()),
                ..record("op1", "a")
            },
        ));
        let report = run(&manifest, records).unwrap();
        assert!(
            report
                .violations
                .iter()
                .any(|v| v.check == "environment_consistency" && v.field == Some("binary_sha256"))
        );
    }

    #[test]
    fn config_drift_is_a_violation() {
        let mut manifest = empty_manifest();
        manifest.config_sha256 = Some("cfg1".to_string());
        let mut records = clean_pairs(1);
        records.push((
            100,
            VerifyRunRecord {
                config_sha256: Some("cfg2".to_string()),
                ..record("op1", "a")
            },
        ));
        let report = run(&manifest, records).unwrap();
        assert!(
            report
                .violations
                .iter()
                .any(|v| v.check == "environment_consistency" && v.field == Some("config_sha256"))
        );
    }

    #[test]
    fn environment_consistency_reports_a_skip_per_undeclared_field() {
        let mut manifest = empty_manifest();
        manifest.experiment_id = Some("exp1".to_string());
        let report = run(&manifest, clean_pairs(1)).unwrap();
        let skipped_fields: Vec<_> = report
            .checks_skipped
            .iter()
            .filter(|s| s.check == "environment_consistency")
            .filter_map(|s| s.field)
            .collect();
        assert_eq!(skipped_fields.len(), 4); // dataset/binary/weight/config all undeclared
    }

    // --- ordering determinism ---

    #[test]
    fn violations_are_sorted_deterministically() {
        let mut manifest = empty_manifest();
        manifest.experiment_id = Some("exp1".to_string());
        let mut records = vec![(1, record("z_lonely", "a")), (2, record("a_lonely", "a"))];
        records.truncate(2);
        // Two independent lone-id violations at different lines - assert first-line order.
        let report1 = run(&manifest, records.clone()).unwrap();
        let report2 = run(&manifest, records).unwrap();
        let lines1: Vec<_> = report1.violations.iter().map(|v| v.lines.clone()).collect();
        let lines2: Vec<_> = report2.violations.iter().map(|v| v.lines.clone()).collect();
        assert_eq!(lines1, lines2);
        assert_eq!(report1.violations[0].lines, vec![1]);
    }
}
