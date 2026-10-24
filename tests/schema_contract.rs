//! Contract tests that keep emitted report versions aligned with the checked-in schemas.

use assert_cmd::Command;
use predicates::prelude::*;

fn veridict() -> Command {
    Command::cargo_bin("veridict").unwrap()
}

fn assert_required_fields_are_declared(schema: &serde_json::Value, location: &str) {
    if let Some(required) = schema.get("required").and_then(serde_json::Value::as_array) {
        let properties = schema
            .get("properties")
            .and_then(serde_json::Value::as_object)
            .unwrap_or_else(|| panic!("{location}: required fields need an object properties map"));
        for field in required {
            let field = field
                .as_str()
                .unwrap_or_else(|| panic!("{location}: required entries must be strings"));
            assert!(
                properties.contains_key(field),
                "{location}: required field {field:?} is missing from properties"
            );
        }
    }

    if let Some(properties) = schema
        .get("properties")
        .and_then(serde_json::Value::as_object)
    {
        for (name, child) in properties {
            assert_required_fields_are_declared(child, &format!("{location}.properties.{name}"));
        }
    }
    if let Some(definitions) = schema.get("$defs").and_then(serde_json::Value::as_object) {
        for (name, child) in definitions {
            assert_required_fields_are_declared(child, &format!("{location}.$defs.{name}"));
        }
    }
    if let Some(items) = schema.get("items") {
        assert_required_fields_are_declared(items, &format!("{location}.items"));
    }
    for keyword in ["allOf", "anyOf", "oneOf"] {
        if let Some(branches) = schema.get(keyword).and_then(serde_json::Value::as_array) {
            for (index, child) in branches.iter().enumerate() {
                assert_required_fields_are_declared(
                    child,
                    &format!("{location}.{keyword}[{index}]"),
                );
            }
        }
    }
}

#[test]
fn every_checked_in_schema_is_valid_json_with_consistent_required_fields() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("schemas");
    let mut schemas: Vec<_> = std::fs::read_dir(&root)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .collect();
    schemas.sort();
    assert!(!schemas.is_empty());

    for path in schemas {
        let schema: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(
            schema["$schema"],
            "https://json-schema.org/draft/2020-12/schema",
            "{} uses an unexpected JSON Schema draft",
            path.display()
        );
        assert_required_fields_are_declared(&schema, &path.display().to_string());
    }
}

#[test]
fn schema_version_appears_in_compare_sprt_and_matrix_reports() {
    let stdin = "{\"result\":\"candidate_win\"}\n{\"result\":\"baseline_win\"}\n";
    veridict()
        .args(["compare", "-", "--metric", "winrate"])
        .write_stdin(stdin)
        .assert()
        .stdout(predicate::str::contains("\"schema_version\": 1"));
    veridict()
        .args(["sprt", "-", "--elo0", "0", "--elo1", "10"])
        .write_stdin(stdin)
        .assert()
        .stdout(predicate::str::contains("\"schema_version\": 2"));
    veridict()
        .args(["matrix", "examples/winloss.jsonl"])
        .assert()
        .stdout(predicate::str::contains("\"schema_version\": 1"));
}

fn validate_top_level_contract(
    report: &serde_json::Value,
    schema: &serde_json::Value,
) -> Result<(), String> {
    let report = report
        .as_object()
        .ok_or_else(|| "report is not an object".to_string())?;
    let properties = schema["properties"]
        .as_object()
        .ok_or_else(|| "schema has no properties".to_string())?;
    for required in schema["required"]
        .as_array()
        .ok_or_else(|| "schema has no required array".to_string())?
    {
        let required = required.as_str().unwrap();
        if !report.contains_key(required) {
            return Err(format!("missing required field {required}"));
        }
    }
    if schema["additionalProperties"] == serde_json::Value::Bool(false) {
        for field in report.keys() {
            if !properties.contains_key(field) {
                return Err(format!("unknown field {field}"));
            }
        }
    }
    let expected_version = schema["properties"]["schema_version"]["const"]
        .as_u64()
        .ok_or_else(|| "schema_version has no integer const".to_string())?;
    if report["schema_version"].as_u64() != Some(expected_version) {
        return Err("schema_version does not match schema const".to_string());
    }
    Ok(())
}

#[test]
fn sprt_v1_and_v2_top_level_contracts_are_unambiguous() {
    let stdin = "{\"result\":\"candidate_win\"}\n{\"result\":\"baseline_win\"}\n";
    let output = veridict()
        .args(["sprt", "-", "--elo0", "0", "--elo1", "10"])
        .write_stdin(stdin)
        .output()
        .unwrap();
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let v1: serde_json::Value = serde_json::from_slice(
        &std::fs::read(root.join("schemas/sprt-report-v1.schema.json")).unwrap(),
    )
    .unwrap();
    let v2: serde_json::Value = serde_json::from_slice(
        &std::fs::read(root.join("schemas/sprt-report.schema.json")).unwrap(),
    )
    .unwrap();

    assert!(validate_top_level_contract(&report, &v2).is_ok());
    assert!(validate_top_level_contract(&report, &v1).is_err());

    let mut legacy = report;
    let legacy_object = legacy.as_object_mut().unwrap();
    legacy_object.insert("schema_version".to_string(), serde_json::json!(1));
    legacy_object.remove("warnings");
    legacy_object.remove("data_quality");
    assert!(validate_top_level_contract(&legacy, &v1).is_ok());
    assert!(validate_top_level_contract(&legacy, &v2).is_err());
}
