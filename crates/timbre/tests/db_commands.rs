//! The `timbre db` commands' exit behavior, observed through the real binary.
//! Piped stdout is never a terminal, so every line here is JSON.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// One temp data directory per test, removed on drop.
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
        "timbre-db-test-{tag}-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    );
    let path = std::env::temp_dir().join(unique);
    std::fs::create_dir_all(&path).expect("the scratch directory exists");
    TempDir(path)
}

/// Run one command against a private data directory so `db` touches a
/// fresh database instead of the crate's `./data`. The directory is
/// returned un-deleted; the caller removes it after its assertions.
#[expect(
    clippy::expect_used,
    reason = "test helper: a missing binary must show why"
)]
fn run_db(tag: &str, args: &[&str]) -> (Vec<String>, Option<i32>, PathBuf) {
    let dir = temp_data_dir(tag);
    let output = Command::new(env!("CARGO_BIN_EXE_timbre"))
        .env("TIMBRE_LOG", "info")
        .env("TIMBRE_SERVER__DATA_DIR", dir.path().as_os_str())
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("the binary runs");
    let stdout = String::from_utf8(output.stdout).expect("stdout is utf-8");
    // The directory must outlive the assertion strings, so it is returned
    // to the caller rather than dropped here.
    let kept = dir.path().to_path_buf();
    std::mem::forget(dir);
    (
        stdout.lines().map(str::to_owned).collect(),
        output.status.code(),
        kept,
    )
}

#[expect(
    clippy::expect_used,
    reason = "test helper: a missing binary must show why"
)]
fn run_in_dir(data_dir: &Path, args: &[&str]) -> (Vec<String>, Option<i32>) {
    let output = Command::new(env!("CARGO_BIN_EXE_timbre"))
        .env("TIMBRE_LOG", "info")
        .env("TIMBRE_SERVER__DATA_DIR", data_dir.as_os_str())
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
fn an_existing_backup_destination_without_force_exits_two() {
    let (lines, code, dir) = run_db("backup-exists", &["db", "check"]);
    assert_eq!(code, Some(0), "the database opens: {lines:?}");
    let snap = dir.join("snap.db");
    let snap_text = snap.to_str().expect("the scratch path is text");
    let first = run_in_dir(&dir, &["db", "backup", snap_text]);
    assert_eq!(first.1, Some(0), "the first backup runs: {:?}", first.0);
    let second = run_in_dir(&dir, &["db", "backup", snap_text]);
    assert_eq!(second.1, Some(2), "the existing destination exits 2");
    assert!(
        second
            .0
            .iter()
            .any(|line| line.contains(r#""event":"timbre.usage""#) && line.contains("--force")),
        "the usage line names `--force`: {:?}",
        second.0
    );
    assert!(
        !second
            .0
            .iter()
            .any(|line| line.contains(r#""event":"timbre.db_backup""#)),
        "a usage error is not a backup result: {:?}",
        second.0
    );
    // `--force` in any position replaces the file.
    let forced = run_in_dir(&dir, &["db", "backup", "--force", snap_text]);
    assert_eq!(
        forced.1,
        Some(0),
        "`--force` before the name works: {:?}",
        forced.0
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn an_unknown_db_verb_is_a_usage_error() {
    let (lines, code, dir) = run_db("bad-verb", &["db", "vacuum"]);
    assert_eq!(code, Some(2), "an unknown verb exits 2");
    let usage = lines
        .iter()
        .find(|line| line.contains(r#""event":"timbre.usage""#))
        .unwrap_or_else(|| panic!("a timbre.usage line in: {lines:?}"));
    assert!(usage.contains("vacuum"), "the verb is named: {usage}");
    assert!(
        !lines
            .iter()
            .any(|line| line.contains(r#""event":"timbre.db_check""#)),
        "the unknown verb is not reported as `check`: {lines:?}"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_backup_without_a_target_is_a_usage_error() {
    let (lines, code, dir) = run_db("no-target", &["db", "backup"]);
    assert_eq!(code, Some(2), "a missing target exits 2");
    assert!(
        lines
            .iter()
            .any(|line| line.contains(r#""event":"timbre.usage""#)
                && line.contains("usage: timbre db backup")),
        "the backup usage line answers: {lines:?}"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_wrong_flag_under_the_other_command_is_a_usage_error() {
    // `--yes` belongs to `prune`, `--force` to `backup`; a swap must be a
    // usage error, never a silently ignored flag.
    let (lines, code, dir) = run_db("wrong-flag", &["db", "backup", "--yes"]);
    assert_eq!(code, Some(2), "`backup --yes` exits 2: {lines:?}");
    let pruned = run_in_dir(&dir, &["db", "prune", "--force"]);
    assert_eq!(pruned.1, Some(2), "`prune --force` exits 2");
    let _ = std::fs::remove_dir_all(dir);
}
