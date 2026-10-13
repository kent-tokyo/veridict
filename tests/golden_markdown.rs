//! Golden Markdown coverage for every fixed-sample verdict and for a
//! validity failure. Each golden is also rebuilt from the command's JSON
//! fields, which keeps Markdown a rendering of the machine-readable contract
//! rather than a second source of decision information.
//!
//! Regenerate after an intentional rendering change with
//! `UPDATE_GOLDEN=1 cargo test --test golden_markdown`, then review every
//! `tests/fixtures/compare_*.golden.md` diff.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use assert_cmd::Command;
use serde_json::Value;

fn veridict() -> Command {
    Command::cargo_bin("veridict").unwrap()
}

fn temp_markdown_path(name: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "veridict-golden-markdown-{}-{nonce}-{name}.md",
        std::process::id()
    ))
}

fn string<'a>(json: &'a Value, field: &str) -> &'a str {
    json[field]
        .as_str()
        .unwrap_or_else(|| panic!("{field} must be a JSON string"))
}

fn number(json: &Value, field: &str) -> f64 {
    json[field]
        .as_f64()
        .unwrap_or_else(|| panic!("{field} must be a JSON number"))
}

fn count(json: &Value, field: &str) -> u64 {
    json[field]
        .as_u64()
        .unwrap_or_else(|| panic!("{field} must be a JSON integer"))
}

fn fmt_effect(metric: &str, value: f64) -> String {
    match metric {
        "winrate" | "sign-test" => format!("{:+.1} pp", value * 100.0),
        "elo" | "paired-elo" => format!("{value:+.1} elo"),
        "mean-diff" | "quantile-diff" => format!("{value:+.4}"),
        "relative-diff" => format!("{:+.1}%", value * 100.0),
        other => panic!("unexpected metric {other}"),
    }
}

fn markdown_from_json(json: &Value) -> String {
    let metric = string(json, "metric");
    let verdict = match json["inconclusive_kind"].as_str() {
        Some(kind) => format!("{} ({kind})", string(json, "verdict")),
        None => string(json, "verdict").to_string(),
    };
    let metric_label = match json["quantile"].as_f64() {
        Some(q) => format!("{metric} (q={q:.2})"),
        None => metric.to_string(),
    };
    let estimate_line = match json["estimated_additional_trials"].as_u64() {
        Some(n) => format!("\nEstimated additional trials to reach a decisive verdict: ~{n}\n"),
        None => String::new(),
    };
    let baseline = &json["failure_breakdown"]["baseline"];
    let candidate = &json["failure_breakdown"]["candidate"];

    let mut markdown = format!(
        "# Veridict Report\n\n\
         Verdict: {verdict}\n\
         Validity: {validity}\n\
         Promotion: {promotion}\n\n\
         Metric: {metric_label}\n\
         Effect: {effect}\n\
         {confidence}% CI: {ci_low} to {ci_high}\n\
         Threshold for pass: {pass_above} / Threshold for fail: {fail_below}\n\n\
         {reason}\n\
         {estimate_line}\n\
         Samples: baseline={baseline_count}, candidate={candidate_count}, paired={paired_count}\n\n\
         Status counts:\n\
         - timeout: {timeouts} (baseline={baseline_timeout}, candidate={candidate_timeout})\n\
         - crash: {crashes} (baseline={baseline_crash}, candidate={candidate_crash})\n\
         - invalid: {invalid} (baseline={baseline_invalid}, candidate={candidate_invalid})\n",
        validity = string(json, "validity"),
        promotion = string(json, "promotion"),
        effect = fmt_effect(metric, number(json, "effect")),
        confidence = number(json, "confidence") * 100.0,
        ci_low = fmt_effect(metric, number(json, "ci_low")),
        ci_high = fmt_effect(metric, number(json, "ci_high")),
        pass_above = fmt_effect(metric, number(json, "pass_above")),
        fail_below = fmt_effect(metric, number(json, "fail_below")),
        reason = string(json, "reason"),
        baseline_count = count(json, "baseline_count"),
        candidate_count = count(json, "candidate_count"),
        paired_count = count(json, "paired_count"),
        timeouts = count(json, "timeouts"),
        crashes = count(json, "crashes"),
        invalid = count(json, "invalid"),
        baseline_timeout = count(baseline, "timeout"),
        candidate_timeout = count(candidate, "timeout"),
        baseline_crash = count(baseline, "crash"),
        candidate_crash = count(candidate, "crash"),
        baseline_invalid = count(baseline, "invalid"),
        candidate_invalid = count(candidate, "invalid"),
    );

    let warnings = json["warnings"].as_array().expect("warnings array");
    if !warnings.is_empty() {
        markdown.push_str("\nWarnings:\n");
        for warning in warnings {
            markdown.push_str("- ");
            markdown.push_str(warning.as_str().expect("warning string"));
            markdown.push('\n');
        }
    }
    markdown
}

fn check_golden(name: &str, args: &[&str], expected_code: i32) {
    let report_path = temp_markdown_path(name);
    let mut command = veridict();
    command.args(args).args([
        "--report-md",
        report_path.to_str().expect("UTF-8 temp path"),
    ]);
    let output = command.output().unwrap();
    assert_eq!(
        output.status.code(),
        Some(expected_code),
        "unexpected exit code, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: Value = serde_json::from_slice(&output.stdout).expect("valid report JSON");
    let markdown = std::fs::read_to_string(&report_path).expect("Markdown report");
    let _ = std::fs::remove_file(&report_path);

    assert_eq!(
        markdown,
        markdown_from_json(&json),
        "Markdown contains content not represented by JSON, or omits/misrenders a JSON field"
    );

    let fixture_path = format!("tests/fixtures/{name}.golden.md");
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::write(&fixture_path, markdown).unwrap();
    } else {
        let expected = std::fs::read_to_string(&fixture_path).unwrap_or_else(|_| {
            panic!(
                "missing golden fixture {fixture_path}; run UPDATE_GOLDEN=1 cargo test --test golden_markdown and review the diff"
            )
        });
        assert_eq!(markdown, expected, "Markdown golden drifted");
    }
}

#[test]
fn pass_markdown_is_golden_and_json_derived() {
    check_golden(
        "compare_pass",
        &[
            "compare",
            "tests/fixtures/action_pass.jsonl",
            "--metric",
            "winrate",
            "--min-effect",
            "0.1",
        ],
        0,
    );
}

#[test]
fn fail_markdown_is_golden_and_json_derived() {
    check_golden(
        "compare_fail",
        &[
            "compare",
            "tests/fixtures/action_fail.jsonl",
            "--metric",
            "winrate",
            "--min-effect",
            "0.1",
        ],
        1,
    );
}

#[test]
fn inconclusive_markdown_is_golden_and_json_derived() {
    check_golden(
        "compare_inconclusive",
        &[
            "compare",
            "tests/fixtures/action_inconclusive.jsonl",
            "--metric",
            "winrate",
            "--min-effect",
            "0.1",
        ],
        2,
    );
}

#[test]
fn invalid_validity_evidence_is_golden_and_json_derived() {
    check_golden(
        "compare_invalid_validity",
        &[
            "compare",
            "tests/fixtures/action_invalid_evidence.jsonl",
            "--metric",
            "winrate",
            "--min-effect",
            "0.1",
            "--max-timeouts",
            "0",
        ],
        2,
    );
}
