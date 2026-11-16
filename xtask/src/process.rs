//! Running external programs: git, cargo, and the tools the gate calls directly.

use crate::Outcome;
use std::path::Path;
use std::process::{Command, Stdio};

pub(crate) fn run(program: &str, args: &[&str], dir: &Path) -> Outcome {
    let rendered = format!("`{program} {}`", args.join(" "));
    let status = Command::new(program)
        .args(args)
        .current_dir(dir)
        // xtask itself runs under `cargo run`, so `CARGO_PKG_NAME=xtask` sits in our
        // environment; cargo subcommands (cargo-machete) read that variable to detect
        // being launched by cargo, and the inherited value makes them misparse argv.
        .env_remove("CARGO_PKG_NAME")
        .status()
        .map_err(|error| format!("{rendered} failed to start: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{rendered} exited with {status}"))
    }
}

pub(crate) fn output(program: &str, args: &[&str], dir: &Path) -> Outcome<String> {
    let rendered = format!("`{program} {}`", args.join(" "));
    let child = Command::new(program)
        .args(args)
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|error| format!("{rendered} failed to start: {error}"))?;
    if !child.status.success() {
        let detail = String::from_utf8_lossy(&child.stderr);
        return Err(format!(
            "{rendered} exited with {}: {}",
            child.status,
            detail.trim()
        ));
    }
    String::from_utf8(child.stdout)
        .map_err(|error| format!("{rendered} wrote invalid UTF-8: {error}"))
}
