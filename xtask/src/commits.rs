//! `commits`: the rules of the history.
//!
//! `committed` (<https://crates.io/crates/committed>) enforces conventional style, the
//! subject shape, capitalization, length, the imperative mood and the ban on fixup,
//! wip and merge commits; its configuration is the root `committed.toml`. This module
//! runs it and adds only the rules the generic tool cannot know: the body a `feat`,
//! `fix` or `perf` commit owes the reader, the banned phrases shared with comments,
//! and the ASCII-only rule that keeps `git log` readable everywhere.

use crate::Outcome;
use crate::check::banned;
use crate::{process, workspace};
use std::path::Path;

/// The `git log` record separator: a control character ordinary messages do not
/// contain (git allows any bytes in a message, so the choice is a convention, not a
/// guarantee).
const COMMIT_SEPARATOR: char = '\x1e';
/// The `git log` separator between a commit's hash and its message, the same kind of
/// control character.
const FIELD_SEPARATOR: char = '\x1f';

/// Every commit reachable through `range` (a branch name or a revision range, the
/// same spelling `git log` takes): `committed` first, then the xtask rules.
pub(crate) fn in_range(range: &str) -> Outcome<Vec<String>> {
    let root = workspace::root()?;
    let phrases = load_phrases(&root)?;
    ensure_committed(&root)?;
    process::run("committed", &[range], &root)?;
    let format = format!("--format=%H{FIELD_SEPARATOR}%B{COMMIT_SEPARATOR}");
    let log = process::output("git", &["log", &format, range], &root)?;
    let mut problems = Vec::new();
    for entry in log.split(COMMIT_SEPARATOR) {
        let Some((hash, message)) = entry.split_once(FIELD_SEPARATOR) else {
            continue;
        };
        for problem in check_own(message.trim_end(), &phrases) {
            problems.push(format!("{hash}: {problem}"));
        }
    }
    Ok(problems)
}

/// One message in a file, the shape `git commit --file` and the CI job hand over:
/// `committed` first, then the xtask rules on the text with git's own cleanup
/// (comment lines dropped, trailing whitespace ignored) applied up front.
pub(crate) fn message_file(path: &str) -> Outcome<Vec<String>> {
    let root = workspace::root()?;
    let phrases = load_phrases(&root)?;
    let text =
        std::fs::read_to_string(path).map_err(|error| format!("cannot read {path}: {error}"))?;
    ensure_committed(&root)?;
    process::run("committed", &["--commit-file", path], &root)?;
    Ok(check_own(&normalize_file(&text), &phrases)
        .into_iter()
        .map(|problem| format!("{path}: {problem}"))
        .collect())
}

/// The xtask-only rules: they never restate what `committed` already checks.
fn check_own(message: &str, phrases: &[String]) -> Vec<String> {
    let mut problems = Vec::new();
    let mut lines = message.lines();
    let subject = lines.next().unwrap_or_default();
    if requires_body(subject) && !(lines.next() == Some("") && lines.any(|line| !line.is_empty())) {
        problems
            .push("a feat, fix or perf commit needs a body after a blank second line".to_owned());
    }
    if let Some(phrase) = banned::find_banned(message, phrases) {
        problems.push(format!("the message contains the banned phrase `{phrase}`"));
    }
    if !message.is_ascii() {
        problems.push("the message must be ASCII".to_owned());
    }
    problems
}

/// `true` when the subject's conventional type is one that owes a body. A malformed
/// subject is left to `committed`; matching it loosely keeps these rules honest
/// about the scope and `!` forms.
fn requires_body(subject: &str) -> bool {
    let head = subject.split(':').next().unwrap_or(subject);
    let head = head.trim_end().trim_end_matches('!');
    let type_name = head.split('(').next().unwrap_or(head).trim_end();
    matches!(type_name, "feat" | "fix" | "perf")
}

/// Git's cleanup for a message file: drop comment lines, ignore trailing whitespace
/// and trailing blank lines.
fn normalize_file(text: &str) -> String {
    let kept: Vec<&str> = text
        .lines()
        .filter(|line| !line.starts_with('#'))
        .map(str::trim_end)
        .collect();
    kept.join("\n").trim_end().to_owned()
}

fn load_phrases(root: &Path) -> Outcome<Vec<String>> {
    Ok(banned::parse(&workspace::read(root, banned::PATH)?))
}

/// `committed` is an external binary, installed by `just setup` and pinned there and
/// in CI; a missing one is an operational error, not a violation.
fn ensure_committed(root: &Path) -> Outcome {
    process::output("committed", &["--version"], root)
        .map(|_| ())
        .map_err(|_| "the `committed` binary is missing; run `just setup`".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn phrases() -> Vec<String> {
        vec!["TODO".to_owned()]
    }

    /// The message of the phase 0a commit, verbatim from `git log -1 --format=%B main`.
    const PHASE_0A: &str = "feat: Lay down the workspace and the kernel contracts

Every module is written against the kernel: registration, signals
between modules, ports that consumers must tolerate being absent, a
structured logger whose event names are checked when the code compiles,
and failures with stable codes. The binary registers each module behind
its own cargo feature, so the server builds and runs without any of them.";

    #[test]
    fn the_phase_0a_message_passes_the_own_rules() {
        assert_eq!(check_own(PHASE_0A, &phrases()), Vec::<String>::new());
    }

    #[test]
    fn a_feat_without_a_body_fails() {
        let problems = check_own("feat: Add the thing", &phrases());
        assert_eq!(problems.len(), 1);
        assert!(problems[0].contains("body"));
    }

    #[test]
    fn a_banned_phrase_fails() {
        let problems = check_own("fix: Repair it\n\nTODO: explain why later.", &phrases());
        assert!(
            problems
                .iter()
                .any(|problem| problem.contains("banned phrase `TODO`")),
            "{problems:?}"
        );
    }

    #[test]
    fn a_non_ascii_message_fails() {
        let problems = check_own("chore: Rename the parser \u{2192} faster", &phrases());
        assert!(
            problems.iter().any(|problem| problem.contains("ASCII")),
            "{problems:?}"
        );
    }
}
