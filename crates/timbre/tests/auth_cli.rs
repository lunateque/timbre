//! What the device commands put on the real binary's stdout.
//!
//! The issued secret is the one thing the server never stores and never
//! logs, so it must appear exactly once - on stdout, by itself - and
//! nowhere else. Diagnostics on piped stdout use the JSON log.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

struct TempDir(PathBuf);

impl TempDir {
    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[expect(
    clippy::expect_used,
    reason = "test helper: a scratch directory the run cannot do without"
)]
fn temp_data_dir(tag: &str) -> TempDir {
    static COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let unique = format!(
        "timbre-auth-cli-{tag}-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    );
    let path = std::env::temp_dir().join(unique);
    std::fs::create_dir_all(&path).expect("the scratch directory exists");
    TempDir(path)
}

#[expect(
    clippy::expect_used,
    reason = "test helper: a missing binary must show why"
)]
fn run(dir: &TempDir, args: &[&str]) -> (Vec<String>, Option<i32>) {
    let output = Command::new(env!("CARGO_BIN_EXE_timbre"))
        .env("TIMBRE_LOG", "debug")
        .env("TIMBRE_SERVER__DATA_DIR", dir.path().as_os_str())
        .args(args)
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
fn an_issued_secret_is_printed_once_and_never_logged() {
    let dir = temp_data_dir("issue");
    assert_eq!(run(&dir, &["user", "add", "alice"]).1, Some(0));
    let (lines, code) = run(&dir, &["device", "issue", "alice", "laptop"]);
    assert_eq!(code, Some(0), "issuing succeeds: {lines:?}");
    let bare: Vec<&String> = lines.iter().filter(|line| !line.starts_with('{')).collect();
    assert_eq!(bare.len(), 1, "one bare line, the secret: {lines:?}");
    let secret = bare.first().copied().cloned().unwrap_or_default();
    assert!(secret.contains('_'), "the secret is a key: {secret}");
    assert_eq!(
        lines.iter().filter(|line| line.contains(&secret)).count(),
        1,
        "the secret appears exactly once in everything the binary wrote"
    );

    // Nothing that reads devices back may show it, or its digest.
    let (listed, code) = run(&dir, &["device", "list", "alice"]);
    assert_eq!(code, Some(0), "listing succeeds: {listed:?}");
    let joined = listed.join("\n");
    assert!(joined.contains("laptop"), "the device is listed: {joined}");
    assert!(
        !joined.contains(&secret) && !joined.contains("digest"),
        "a listing shows no secret and no digest: {joined}"
    );
}

#[test]
fn command_results_are_not_log_lines_and_arguments_are_usage_errors() {
    let dir = temp_data_dir("command-output");
    for args in [
        &["user", "add", "--admin"][..],
        &["device", "issue", "alice"],
        &["device", "issue", "alice", "bad\u{200b}label"],
        &["device", "issue", "alice", "laptop", "--expires", "+2"],
        &["device", "issue", "alice", "laptop", "--expires", "36501"],
    ] {
        assert_eq!(run(&dir, args).1, Some(2), "{args:?}");
    }
    assert_eq!(run(&dir, &["user", "add", "alice"]).1, Some(0));
    let result = Command::new(env!("CARGO_BIN_EXE_timbre"))
        .env("TIMBRE_LOG", "error")
        .env("TIMBRE_SERVER__DATA_DIR", dir.path())
        .args(["user", "list"])
        .output()
        .unwrap();
    assert!(result.status.success());
    assert!(String::from_utf8(result.stdout).unwrap().contains("alice"));
}
