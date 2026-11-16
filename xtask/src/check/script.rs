//! `script`: comments and identifiers are ASCII.
//!
//! Non-ASCII belongs in string literals (user-facing text) and under `fixtures/`
//! (recorded data). A Cyrillic or curly quote in a comment is invisible to most tools
//! and hostile to diffs, so it fails the gate instead of being normalized.

use super::Source;
use super::Violation;
use crate::lexer::Kind;

pub(crate) fn check(sources: &[Source<'_>]) -> Vec<Violation> {
    let mut violations = Vec::new();
    for file in sources {
        if under_fixtures(file.path) {
            continue;
        }
        for token in &file.tokens {
            if !matches!(
                token.kind,
                Kind::LineComment | Kind::BlockComment | Kind::Ident
            ) {
                continue;
            }
            if !token.text.is_ascii() {
                violations.push(Violation::new(
                    "script",
                    file.path,
                    Some(token.line),
                    "comments and identifiers must be ASCII".to_owned(),
                ));
            }
        }
    }
    violations
}

fn under_fixtures(path: &str) -> bool {
    path.split('/').any(|segment| segment == "fixtures")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::check::source;

    #[test]
    fn a_non_ascii_comment_is_reported() {
        let file = source("a.rs", "// \u{43f}\u{440}\u{438}\u{432}\u{435}\u{442}\n");
        let violations = check(&[file]);
        assert_eq!(violations.len(), 1);
        assert_eq!(violations[0].line, Some(1));
    }

    #[test]
    fn a_non_ascii_string_literal_is_fine() {
        let file = source(
            "a.rs",
            "const GREETING: &str = \"\u{43f}\u{440}\u{438}\u{432}\u{435}\u{442}\";\n",
        );
        assert!(check(&[file]).is_empty());
    }

    #[test]
    fn fixtures_are_excepted() {
        let file = source(
            "crates/x/tests/fixtures/golden.rs",
            "// \u{43f}\u{440}\u{438}\u{432}\u{435}\u{442}\n",
        );
        assert!(check(&[file]).is_empty());
    }
}
