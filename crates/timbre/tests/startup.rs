//! Startup behavior of the binary, observed through its real stdout.
//! Piped stdout is never a terminal, so every line here is JSON.
//!
//! No module is named here: which modules are wired is the generated
//! `modules.rs`'s business, and every combination of them is built and tested
//! by `cargo xtask removability`.

use std::process::{Command, Stdio};

// The clippy.toml test allowance covers `#[test]` functions only; this helper
// wants the same loud failure, so the expectation is stated at the site.
#[expect(
    clippy::expect_used,
    reason = "test helper: a missing binary must show why"
)]
fn run(log_value: &str) -> (Vec<String>, Option<i32>) {
    let output = Command::new(env!("CARGO_BIN_EXE_timbre"))
        .env("TIMBRE_LOG", log_value)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("the binary runs");
    let stdout = String::from_utf8(output.stdout).expect("stdout is utf-8");
    (
        stdout.lines().map(str::to_owned).collect(),
        output.status.code(),
    )
}

#[test]
fn info_logs_started_and_exits_successfully() {
    let (lines, code) = run("info");
    assert_eq!(code, Some(0), "startup must succeed");
    assert!(
        lines
            .iter()
            .any(|line| line.contains(r#""event":"timbre.started""#)),
        "a timbre.started line, got {lines:?}"
    );
}

#[test]
fn an_unparsable_filter_exits_with_a_json_explanation() {
    let (lines, code) = run("loud");
    assert_eq!(code, Some(1));
    let line = lines
        .iter()
        .find(|line| line.contains(r#""event":"timbre.log_filter_invalid""#))
        .expect("a timbre.log_filter_invalid line");
    assert!(line.contains(r#""value":"loud""#));
    assert!(line.contains(r#""code":"kernel.log_filter_invalid""#));
}

#[test]
fn warn_hides_every_startup_line() {
    let (lines, code) = run("warn");
    assert_eq!(code, Some(0));
    assert!(lines.is_empty(), "stdout must be empty: {lines:?}");
}
