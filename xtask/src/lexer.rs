//! Rust source tokenization: an adapter over `rustc_lexer` (the compiler's own lexer).
//!
//! The adapter keeps only the four token classes the checks distinguish - comments,
//! literals, names and punctuation - drops whitespace, and adds 1-based line numbers, so
//! the checks never branch on rustc's token taxonomy directly.

use ra_ap_rustc_lexer::{FrontmatterAllowed, LiteralKind, TokenKind};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    LineComment,
    BlockComment,
    Str,
    Char,
    Ident,
    Lifetime,
    Number,
    Punct,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Token<'a> {
    pub(crate) kind: Kind,
    /// The exact source slice of the token.
    pub(crate) text: &'a str,
    /// 1-based line where the token starts.
    pub(crate) line: usize,
}

pub(crate) fn tokenize(source: &str) -> Result<Vec<Token<'_>>, String> {
    let mut tokens = Vec::new();
    let mut offset = 0;
    let mut line = 1;
    for item in ra_ap_rustc_lexer::tokenize(source, FrontmatterAllowed::No) {
        let len = usize::try_from(item.len).unwrap_or(source.len() - offset);
        let end = offset
            .checked_add(len)
            .ok_or_else(|| format!("token past the end of input at line {line}"))?;
        let text = source
            .get(offset..end)
            .ok_or_else(|| format!("token boundary inside a character at line {line}"))?;
        let kind = classify(item.kind, line)?;
        if let Some(kind) = kind {
            tokens.push(Token { kind, text, line });
        }
        line += text.matches('\n').count();
        offset = end;
    }
    Ok(tokens)
}

/// `None` for tokens the checks do not see (whitespace, end of input).
fn classify(kind: TokenKind, line: usize) -> Result<Option<Kind>, String> {
    let error = |what: &str| Err(format!("{what} is unterminated (line {line})"));
    match kind {
        TokenKind::Whitespace | TokenKind::Eof | TokenKind::Frontmatter { .. } => Ok(None),
        TokenKind::LineComment { .. } => Ok(Some(Kind::LineComment)),
        TokenKind::BlockComment { terminated, .. } => {
            if terminated {
                Ok(Some(Kind::BlockComment))
            } else {
                error("block comment")
            }
        }
        TokenKind::Ident
        | TokenKind::RawIdent
        | TokenKind::ForcedKeywordIdent
        | TokenKind::InvalidIdent
        | TokenKind::UnknownPrefix => Ok(Some(Kind::Ident)),
        TokenKind::Lifetime { .. } | TokenKind::RawLifetime | TokenKind::UnknownPrefixLifetime => {
            Ok(Some(Kind::Lifetime))
        }
        TokenKind::Literal { kind, .. } => match kind {
            LiteralKind::Int { .. } | LiteralKind::Float { .. } => Ok(Some(Kind::Number)),
            LiteralKind::Char { terminated } | LiteralKind::Byte { terminated } => terminated
                .then_some(Kind::Char)
                .map(Some)
                .ok_or_else(|| format!("char literal is unterminated (line {line})")),
            LiteralKind::Str { terminated }
            | LiteralKind::ByteStr { terminated }
            | LiteralKind::CStr { terminated } => terminated
                .then_some(Kind::Str)
                .map(Some)
                .ok_or_else(|| format!("string literal is unterminated (line {line})")),
            LiteralKind::RawStr { n_hashes }
            | LiteralKind::RawByteStr { n_hashes }
            | LiteralKind::RawCStr { n_hashes } => n_hashes
                .map(|_| Kind::Str)
                .map(Some)
                .ok_or_else(|| format!("raw string literal is unterminated (line {line})")),
        },
        _ => Ok(Some(Kind::Punct)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(source: &str) -> Vec<(Kind, &str)> {
        tokenize(source)
            .unwrap()
            .into_iter()
            .map(|token| (token.kind, token.text))
            .collect()
    }

    #[test]
    fn a_raw_string_is_one_token_not_a_comment() {
        let tokens = tokenize(r#"const S: &str = r"// not a comment" "#).unwrap();
        let strings: Vec<&Token> = tokens
            .iter()
            .filter(|token| token.kind == Kind::Str)
            .collect();
        assert_eq!(strings.len(), 1);
        assert_eq!(strings[0].text, "r\"// not a comment\"");
        assert!(
            !tokens
                .iter()
                .any(|token| matches!(token.kind, Kind::LineComment | Kind::BlockComment))
        );
    }

    #[test]
    fn chars_and_lifetimes_are_told_apart() {
        assert_eq!(kinds("'a'"), [(Kind::Char, "'a'")]);
        assert_eq!(kinds("'a"), [(Kind::Lifetime, "'a")]);
        assert_eq!(kinds("'static"), [(Kind::Lifetime, "'static")]);
    }

    #[test]
    fn lines_continue_after_a_multiline_raw_string() {
        let tokens = tokenize("let a = r#\"\ntwo\n\"#;\nlet b = 1;").unwrap();
        let b = tokens.iter().find(|token| token.text == "b").unwrap();
        assert_eq!(b.line, 4);
    }

    #[test]
    fn unterminated_tokens_are_errors_naming_the_line() {
        let comment = tokenize("let a = 1;\n/* open\nmore").unwrap_err();
        assert!(comment.contains("line 2"), "{comment}");
        let string = tokenize("\"open\n").unwrap_err();
        assert!(string.contains("line 1"), "{string}");
    }
}
