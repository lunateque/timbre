//! Shared std-only validation helpers for safe identifiers, single-line
//! labels and multi-line comments.
//!
//! Rejects Unicode control (`Cc`), invisible format (`Cf`, including bidi
//! overrides, zero-width spaces and tag characters) and line/paragraph
//! separators so user-supplied text cannot forge logs or visually spoof text.

/// Returned when a text field contains forbidden characters or exceeds its cap.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidText;

/// Returns `true` when `ch` is a Unicode format (`Cf`) character.
#[must_use]
pub const fn is_format_char(ch: char) -> bool {
    matches!(
        ch,
        '\u{ad}'
            | '\u{600}'..='\u{605}'
            | '\u{61c}'
            | '\u{6dd}'
            | '\u{70f}'
            | '\u{890}'..='\u{891}'
            | '\u{8e2}'
            | '\u{180e}'
            | '\u{200b}'..='\u{200f}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{206f}'
            | '\u{feff}'
            | '\u{fff9}'..='\u{fffb}'
            | '\u{110bd}'
            | '\u{110cd}'
            | '\u{13430}'..='\u{1343f}'
            | '\u{1bca0}'..='\u{1bca3}'
            | '\u{1d173}'..='\u{1d17a}'
            | '\u{e0001}'
            | '\u{e0020}'..='\u{e007f}'
    )
}

/// Returns `true` when `ch` is forbidden in user-supplied text.
///
/// When `allow_newline` is `true`, ASCII `\n` is permitted for multi-line
/// comments; all other control, format and Unicode separator characters remain
/// rejected.
#[must_use]
pub const fn is_forbidden_char(ch: char, allow_newline: bool) -> bool {
    if allow_newline && ch == '\n' {
        return false;
    }
    ch.is_control() || is_format_char(ch) || matches!(ch, '\u{2028}' | '\u{2029}')
}

/// Returns `true` when `value` is a non-empty opaque identifier within
/// `max_bytes`, free of whitespace, control, format and separator characters.
#[must_use]
pub fn is_valid_id(value: &str, max_bytes: usize) -> bool {
    !value.is_empty()
        && value.len() <= max_bytes
        && !value
            .chars()
            .any(|ch| ch.is_whitespace() || is_forbidden_char(ch, false))
}

/// Trim and validate a single-line label (1..=`max_chars`, no forbidden chars).
#[must_use]
pub fn clean_label(value: &str, max_chars: usize) -> Option<&str> {
    if value.chars().any(|ch| is_forbidden_char(ch, false)) {
        return None;
    }
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.chars().count() > max_chars {
        return None;
    }
    Some(trimmed)
}

/// Trim and validate an optional multi-line comment (`<= max_chars`, `\n`
/// permitted). Empty or whitespace-only input collapses to `Ok(None)`.
///
/// # Errors
/// Returns [`InvalidText`] when the comment exceeds `max_chars` or contains a
/// forbidden character.
pub fn clean_comment(value: Option<&str>, max_chars: usize) -> Result<Option<&str>, InvalidText> {
    let Some(raw) = value else {
        return Ok(None);
    };
    if raw.chars().any(|ch| is_forbidden_char(ch, true)) {
        return Err(InvalidText);
    }
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    if trimmed.chars().count() > max_chars {
        return Err(InvalidText);
    }
    Ok(Some(trimmed))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_reject_whitespace_controls_and_bidi() {
        assert!(is_valid_id("track-42", 256));
        for bad in [
            "",
            " ",
            " 42",
            "42 ",
            "\u{00a0}",
            "a\nb",
            "a\u{202e}b",
            "a\u{200b}b",
        ] {
            assert!(!is_valid_id(bad, 256), "accepted {bad:?}");
        }
    }

    #[test]
    fn labels_and_comments_enforce_bounds_and_forbidden_chars() {
        assert_eq!(clean_label("  Phone  ", 64), Some("Phone"));
        assert_eq!(clean_label("   ", 64), None);
        assert_eq!(clean_label("a\nb", 64), None);
        assert_eq!(clean_label("ok\r\n", 64), None);
        assert_eq!(clean_label("a\u{feff}b", 64), None);

        assert_eq!(
            clean_comment(Some("  line 1\nline 2  "), 64),
            Ok(Some("line 1\nline 2"))
        );
        assert_eq!(clean_comment(Some("   "), 64), Ok(None));
        assert_eq!(clean_comment(Some("bad\r\n"), 64), Err(InvalidText));
        assert_eq!(clean_comment(Some("bad\u{202e}"), 64), Err(InvalidText));
    }
}
