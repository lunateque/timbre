//! Repository checks (`cargo xtask check`): pure functions over inputs gathered once.
//!
//! Every check gets already-read data - tracked files, their contents, tokens and the
//! parsed cargo metadata - so each one is unit-tested without touching the disk. A
//! violation never stops the others: the report is the whole list at once.

pub(crate) mod banned;
mod docs;
mod graph;
mod metadata;
mod names;
mod root;
mod script;
mod size;
mod tiers;

use crate::Outcome;
use crate::lexer::Token;
use crate::{process, workspace};

/// One problem with the repository, addressed to whoever has to fix it.
#[derive(Debug)]
pub(crate) struct Violation {
    pub(crate) check: &'static str,
    pub(crate) path: String,
    pub(crate) line: Option<usize>,
    pub(crate) message: String,
}

impl Violation {
    fn new(
        check: &'static str,
        path: impl Into<String>,
        line: Option<usize>,
        message: impl Into<String>,
    ) -> Self {
        Violation {
            check,
            path: path.into(),
            line,
            message: message.into(),
        }
    }

    pub(crate) fn render(&self) -> String {
        match self.line {
            Some(line) => format!("{}:{line}: {}: {}", self.path, self.check, self.message),
            None => format!("{}: {}: {}", self.path, self.check, self.message),
        }
    }
}

/// A tracked `.rs` file with its contents and tokens. A file whose lexer error is kept
/// out of here is reported as a `lexer` violation instead of being scanned by the rest.
pub(crate) struct Source<'a> {
    pub(crate) path: &'a str,
    pub(crate) text: &'a str,
    pub(crate) tokens: Vec<Token<'a>>,
}

pub(crate) fn run() -> Outcome<Vec<Violation>> {
    let root = workspace::root()?;
    let files = workspace::tracked_files(&root)?;
    let metadata_text = process::output(
        "cargo",
        &["metadata", "--format-version", "1", "--no-deps"],
        &root,
    )?;
    let value: serde_json::Value = serde_json::from_str(&metadata_text)
        .map_err(|error| format!("cannot parse the cargo metadata: {error}"))?;
    let packages = metadata::parse(&value, &root)?;
    let allow_text = workspace::read(&root, "deps.allow")?;
    let banned_list = banned::parse(&workspace::read(&root, banned::PATH)?);

    let mut rust: Vec<(String, String)> = Vec::new();
    let mut agents_owned: Vec<(String, String)> = Vec::new();
    // Byte-exact `.rs`: the checks define Rust sources by the tracked lower-case
    // extension, and a differently cased name is a different file to git.
    #[expect(
        clippy::case_sensitive_file_extension_comparisons,
        reason = "the rule is the exact suffix, not the file type"
    )]
    for path in &files {
        if path.ends_with(".rs") {
            rust.push((path.clone(), workspace::read(&root, path)?));
        } else if path.ends_with("AGENTS.md") {
            agents_owned.push((path.clone(), workspace::read(&root, path)?));
        }
    }
    let agents: Vec<(&str, &str)> = agents_owned
        .iter()
        .map(|(path, text)| (path.as_str(), text.as_str()))
        .collect();

    let mut violations = Vec::new();
    let mut sources: Vec<Source> = Vec::new();
    for (path, text) in &rust {
        match crate::lexer::tokenize(text) {
            Ok(tokens) => sources.push(Source { path, text, tokens }),
            Err(message) => violations.push(Violation::new("lexer", path.clone(), None, message)),
        }
    }

    violations.extend(root::check(&files));
    violations.extend(size::check(&sources));
    violations.extend(graph::check(&packages, &sources));
    violations.extend(tiers::check(&packages, &allow_text));
    violations.extend(docs::check(&agents, &packages));
    violations.extend(names::check(&packages, &sources));
    violations.extend(banned::check(&banned_list, &sources));
    violations.extend(script::check(&sources));

    violations.sort_by(|a, b| {
        a.path
            .cmp(&b.path)
            .then(a.line.unwrap_or(0).cmp(&b.line.unwrap_or(0)))
    });
    Ok(violations)
}

/// A `Source` for the unit tests of the checks: panics only when a fixture is malformed.
#[cfg(test)]
pub(crate) fn source<'a>(path: &'a str, text: &'a str) -> Source<'a> {
    Source {
        path,
        text,
        tokens: crate::lexer::tokenize(text).unwrap(),
    }
}
