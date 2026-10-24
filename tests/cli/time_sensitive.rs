use super::*;

#[test]
fn huge_custom_reward_schedule_exits_three_without_panicking() {
    let path = std::env::temp_dir().join("veridict_cli_test_huge_reward_schedule.json");
    std::fs::write(
        &path,
        r#"{"rewards":[{"until":18446744073709551615,"value":1.0},{"after":18446744073709551615,"value":0.0}]}"#,
    )
    .unwrap();

    veridict()
        .args([
            "time-sensitive",
            "-",
            "--p0",
            "0.5",
            "--p1",
            "0.6",
            "--policy",
            "gro",
            "--reward-schedule",
            path.to_str().unwrap(),
        ])
        .write_stdin("{\"result\":\"candidate_win\"}\n")
        .assert()
        .code(3)
        .stderr(predicate::str::contains("supported maximum"));

    std::fs::remove_file(path).ok();
}
