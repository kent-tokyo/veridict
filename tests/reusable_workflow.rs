//! Contract tests for the reusable GitHub Actions gate. The runner script is
//! extracted from the workflow itself, so these tests exercise the exact Bash
//! block that hosted Actions executes rather than a second copy that can drift.

use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

const WORKFLOW: &str = ".github/workflows/veridict-gate.yml";

struct GateRun {
    root: PathBuf,
    exit_code: u8,
}

impl GateRun {
    fn report_dir(&self) -> PathBuf {
        self.root.join("veridict-report")
    }
}

impl Drop for GateRun {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn extracted_gate_script() -> String {
    let workflow = fs::read_to_string(WORKFLOW).unwrap();
    let run_gate = workflow
        .find("        id: run-gate\n")
        .expect("run-gate step");
    let run_block = workflow[run_gate..]
        .find("        run: |\n")
        .map(|offset| run_gate + offset + "        run: |\n".len())
        .expect("run-gate script");

    workflow[run_block..]
        .lines()
        .take_while(|line| line.is_empty() || line.starts_with("          "))
        .map(|line| line.strip_prefix("          ").unwrap_or(line))
        .collect::<Vec<_>>()
        .join("\n")
}

fn unique_temp_dir(case: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "veridict-reusable-workflow-{}-{nonce}-{case}",
        std::process::id()
    ))
}

fn run_gate(
    case: &str,
    command: &str,
    input: &str,
    manifest: &str,
    arguments_json: &str,
) -> GateRun {
    run_gate_impl(case, command, input, manifest, arguments_json, false)
}

fn run_gate_impl(
    case: &str,
    command: &str,
    input: &str,
    manifest: &str,
    arguments_json: &str,
    simulate_legacy_usage_error: bool,
) -> GateRun {
    let root = unique_temp_dir(case);
    fs::create_dir_all(&root).unwrap();
    let script = root.join("run-gate.sh");
    let output_file = root.join("github-output.txt");
    fs::write(&script, extracted_gate_script()).unwrap();

    let binary = Path::new(env!("CARGO_BIN_EXE_veridict"));
    let binary_dir = if simulate_legacy_usage_error {
        let stub_dir = root.join("stub-bin");
        fs::create_dir(&stub_dir).unwrap();
        let stub = stub_dir.join("veridict");
        fs::write(
            &stub,
            "#!/usr/bin/env bash\nprintf 'error: invalid value from legacy clap\\n' >&2\nexit 2\n",
        )
        .unwrap();
        #[cfg(unix)]
        fs::set_permissions(&stub, fs::Permissions::from_mode(0o755)).unwrap();
        #[cfg(not(unix))]
        panic!("legacy CLI stub execution requires Unix permissions");
        stub_dir
    } else {
        binary.parent().unwrap().to_path_buf()
    };
    let mut paths = vec![binary_dir];
    paths.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    let path = std::env::join_paths(paths).unwrap();

    let output = Command::new("bash")
        .arg(&script)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .env("PATH", path)
        .env("RUNNER_TEMP", &root)
        .env("GITHUB_OUTPUT", &output_file)
        .env("VERIDICT_VERSION", "0.19.6")
        .env("VERIDICT_COMMAND", command)
        .env("VERIDICT_INPUT", input)
        .env("VERIDICT_MANIFEST", manifest)
        .env("VERIDICT_ARGUMENTS_JSON", arguments_json)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "workflow capture step failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let github_output = fs::read_to_string(output_file).unwrap();
    let exit_code = github_output
        .lines()
        .find_map(|line| line.strip_prefix("exit_code="))
        .expect("exit_code output")
        .parse()
        .unwrap();

    GateRun { root, exit_code }
}

fn assert_evidence(run: &GateRun, expected_code: u8, expect_report: bool) {
    assert_eq!(run.exit_code, expected_code);
    let report_dir = run.report_dir();
    assert!(report_dir.join("gate.json").is_file());
    assert!(report_dir.join("gate.md").is_file());
    assert!(report_dir.join("stdout.json").is_file());
    assert!(report_dir.join("stderr.txt").is_file());
    assert_eq!(report_dir.join("report.json").is_file(), expect_report);
    assert_eq!(report_dir.join("report.md").is_file(), expect_report);

    let gate: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(report_dir.join("gate.json")).unwrap()).unwrap();
    assert_eq!(gate["exit_code"], expected_code);
    assert_eq!(gate["report_generated"], expect_report);
}

#[test]
fn compare_fixtures_preserve_all_four_exit_classes_and_evidence() {
    let arguments = r#"["--metric","winrate","--min-effect","0.1"]"#;
    for (case, fixture, expected, report) in [
        ("pass", "tests/fixtures/action_pass.jsonl", 0, true),
        ("fail", "tests/fixtures/action_fail.jsonl", 1, true),
        (
            "inconclusive",
            "tests/fixtures/action_inconclusive.jsonl",
            2,
            true,
        ),
        ("invalid", "tests/fixtures/action_invalid.jsonl", 3, false),
    ] {
        let run = run_gate(case, "compare", fixture, "", arguments);
        assert_evidence(&run, expected, report);
    }
}

#[test]
fn workflow_runner_supports_sprt_and_verify_run() {
    let sprt = run_gate(
        "sprt",
        "sprt",
        "tests/fixtures/action_pass.jsonl",
        "",
        r#"["--elo0","0","--elo1","100"]"#,
    );
    assert_evidence(&sprt, 0, true);

    let verify = run_gate(
        "verify-run",
        "verify-run",
        "tests/fixtures/verify_run_clean.jsonl",
        "tests/fixtures/verify_run_clean.toml",
        "[]",
    );
    assert_evidence(&verify, 0, true);
}

#[test]
#[cfg(unix)]
fn workflow_normalizes_legacy_cli_usage_errors_to_invalid() {
    let run = run_gate_impl(
        "invalid-cli-argument",
        "compare",
        "tests/fixtures/action_pass.jsonl",
        "",
        r#"["--metric","not-a-metric"]"#,
        true,
    );
    assert_evidence(&run, 3, false);
    assert!(
        fs::read_to_string(run.report_dir().join("stderr.txt"))
            .unwrap()
            .contains("invalid value from legacy clap")
    );
}

#[test]
fn workflow_pins_the_crate_and_requires_no_consumer_secret() {
    let workflow = fs::read_to_string(WORKFLOW).unwrap();
    assert!(workflow.contains("VERIDICT_VERSION: \"0.19.6\""));
    assert!(workflow.contains("cargo install veridict --version \"=$VERIDICT_VERSION\" --locked"));
    assert!(workflow.contains("uses: actions/upload-artifact@v4"));
    assert!(workflow.contains("if: always()"));
    assert!(!workflow.contains("secrets:"));
    assert!(!workflow.contains("${{ secrets."));
}
