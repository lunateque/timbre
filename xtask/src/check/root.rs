//! `root`: the repository root holds only the tracked entries an owner agreed to.
//!
//! A stray `notes.md`, plan or script at the root means someone wrote prose where the
//! public tree should not carry it (`docs/` and `.internal/` exist for that).

use super::Violation;

const ALLOWED: &[&str] = &[
    ".cargo",
    ".github",
    ".githooks",
    ".gitignore",
    "AGENTS.md",
    "Cargo.lock",
    "Cargo.toml",
    "LICENSE",
    "README.md",
    "clippy.toml",
    "committed.toml",
    "crates",
    "deny.toml",
    "deps.allow",
    "docs",
    "justfile",
    "rust-toolchain.toml",
    "rustfmt.toml",
    "typos.toml",
    "xtask",
];

pub(crate) fn check(files: &[String]) -> Vec<Violation> {
    let mut violations = Vec::new();
    for path in files {
        let first = path.split('/').next().unwrap_or(path);
        if !ALLOWED.contains(&first) {
            violations.push(Violation::new(
                "root",
                path.clone(),
                None,
                format!(
                    "the root entry `{first}` is not in the allowlist of xtask/src/check/root.rs"
                ),
            ));
        }
    }
    violations
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stray_file_at_the_root_is_reported() {
        let files: Vec<String> = [
            "notes.md",
            "docs/plan.md",
            "crates/x/notes.md",
            "xtask/src/main.rs",
        ]
        .iter()
        .map(|name| (*name).to_owned())
        .collect();
        let violations = check(&files);
        assert_eq!(violations.len(), 1);
        assert_eq!(violations[0].path, "notes.md");
        assert_eq!(violations[0].check, "root");
    }
}
