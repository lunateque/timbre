//! `banned`: comments and commit messages carry no residue of the conversation that
//! produced them.
//!
//! The phrase list lives in `xtask/banned.txt`, one phrase per line, `#` lines and
//! blank lines skipped. `find_banned` is shared with `commits`; string literals are not
//! scanned, so fixtures and user-facing text stay free.

use super::Source;
use super::Violation;
use crate::lexer::Kind;

pub(crate) const PATH: &str = "xtask/banned.txt";

/// The accepted phrases, kept verbatim: matching lowercases both sides itself.
pub(crate) fn parse(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_owned)
        .collect()
}

pub(crate) fn check(phrases: &[String], sources: &[Source<'_>]) -> Vec<Violation> {
    let mut violations = Vec::new();
    for file in sources {
        for token in &file.tokens {
            if !matches!(token.kind, Kind::LineComment | Kind::BlockComment) {
                continue;
            }
            if let Some(found) = find_banned(token.text, phrases) {
                violations.push(Violation::new(
                    "banned",
                    file.path,
                    Some(token.line),
                    format!("the comment contains the banned phrase `{found}`"),
                ));
            }
        }
    }
    violations
}

/// The first banned phrase occurring in `text`, or `None`. ASCII-case-insensitive, and
/// only as whole words: the characters just before and after the match are not word
/// characters (ASCII alphanumeric or `_`), or the match sits at a text edge, so a
/// phrase embedded in a longer identifier such as `todo_list` never matches.
///
/// Shared with `commits`, which passes commit subjects and bodies through it.
pub(crate) fn find_banned<'a>(text: &str, phrases: &'a [String]) -> Option<&'a str> {
    let lower = text.to_lowercase();
    let bytes = lower.as_bytes();
    let word = |byte: u8| byte.is_ascii_alphanumeric() || byte == b'_';
    for phrase in phrases {
        let wanted = phrase.to_lowercase();
        let needle = wanted.replace(char::is_whitespace, " ");
        if needle.is_empty() {
            continue;
        }
        let mut from = 0;
        while let Some(offset) = lower
            .get(from..)
            .and_then(|rest| rest.find(needle.as_str()))
        {
            let start = from + offset;
            let end = start + needle.len();
            let before_ok = start == 0 || !word(bytes.get(start - 1).copied().unwrap_or(b' '));
            let after_ok = !bytes.get(end).is_some_and(|byte| word(*byte));
            // `find` over `str` only returns char boundaries; a non-ASCII neighbour is a
            // word boundary because the rule bounds matches with ASCII word characters.
            if before_ok && after_ok {
                return Some(phrase.as_str());
            }
            from = start + 1;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::check::source;

    fn phrases() -> Vec<String> {
        vec![
            "TODO".to_owned(),
            "now uses".to_owned(),
            "Co-Authored-By: Claude".to_owned(),
        ]
    }

    #[test]
    fn a_banned_word_in_a_comment_is_reported() {
        let file = source("a.rs", "fn f() {}\n// TODO: x\n");
        let violations = check(&phrases(), &[file]);
        assert_eq!(violations.len(), 1);
        assert_eq!(violations[0].line, Some(2));
        assert!(violations[0].message.contains("TODO"));
    }

    #[test]
    fn word_boundaries_are_respected() {
        let file = source("a.rs", "// todo_list stays\n");
        assert!(check(&phrases(), &[file]).is_empty());
    }

    #[test]
    fn string_literals_are_not_scanned() {
        let file = source("a.rs", "const S: &str = \"TODO inside a string\";\n");
        assert!(check(&phrases(), &[file]).is_empty());
    }

    #[test]
    fn a_multi_word_phrase_matches_any_case() {
        let file = source("a.rs", "// Now Uses the new design\n");
        let violations = check(&phrases(), &[file]);
        assert_eq!(violations.len(), 1);
        assert!(violations[0].message.contains("now uses"));
    }

    #[test]
    fn parse_skips_comments_and_blanks() {
        let parsed = parse("# header\n\n  TODO  \nnow uses\n");
        assert_eq!(parsed, ["TODO", "now uses"]);
    }
}
