//! Hiding config values in failure reasons: the log carries the reason
//! verbatim, and the value of a config key can be a secret.

/// Replace the offending value in a deserialization message with `<value>`.
/// serde quotes it as ``integer `2024` ``, ``string "abc"``, ``boolean `true` ``
/// and so on; an unrecognized enum spelling arrives as ``unknown variant
/// `s3cr3t` ``, and a value some field rejects outright as
/// ``invalid value: `s3cr3t` ``. The section-name and key-context fragments
/// (``the section `server` ``, ``in `data_dir` ``) and the list of expected
/// variants that follows a rejected one are not value markers at those
/// positions, so they stay.
pub(crate) fn hide_value(text: &str) -> String {
    const MARKERS: &[&str] = &[
        "integer `",
        "floating point `",
        "boolean `",
        "character `",
        "string \"",
        "unknown variant `",
        "invalid value: `",
    ];
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some((at, open)) = MARKERS
        .iter()
        .filter_map(|marker| rest.find(marker).map(|found| (found, marker.len())))
        .min()
    {
        out.push_str(&rest[..at + open]);
        out.push_str("<value>");
        let quote = rest.as_bytes().get(at + open - 1).copied().unwrap_or(b'"');
        let tail = &rest[at + open..];
        rest = match find_close(tail, quote) {
            Some(end) => {
                out.push(quote as char);
                &rest[at + open + end + 1..]
            }
            None => "",
        };
    }
    out.push_str(rest);
    out
}

/// The index of `quote` closing an escaped run, `None` when the token is
/// unterminated (serde escapes inner quotes with a backslash).
fn find_close(text: &str, quote: u8) -> Option<usize> {
    let mut escaped = false;
    for (index, byte) in text.bytes().enumerate() {
        if escaped {
            escaped = false;
        } else if byte == b'\\' {
            escaped = true;
        } else if byte == quote {
            return Some(index);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::hide_value;

    #[test]
    fn every_serde_value_form_becomes_the_marker() {
        assert_eq!(
            hide_value("invalid type: integer `2024`, expected a string"),
            "invalid type: integer `<value>`, expected a string"
        );
        assert_eq!(
            hide_value(r#"invalid type: string "hunter2", expected u16"#),
            r#"invalid type: string "<value>", expected u16"#
        );
        assert_eq!(
            hide_value(r#"invalid type: string "a\"b", expected u16"#),
            r#"invalid type: string "<value>", expected u16"#
        );
        assert_eq!(
            hide_value("invalid type: floating point `1.5`, expected an integer"),
            "invalid type: floating point `<value>`, expected an integer"
        );
    }

    #[test]
    fn key_and_section_context_stay() {
        let hidden = hide_value(
            "the section `server` is malformed: invalid type: string \"s3cr3t\", expected a string\nin `data_dir`\n",
        );
        assert!(hidden.contains("the section `server`"), "{hidden}");
        assert!(hidden.contains("in `data_dir`"), "{hidden}");
        assert!(!hidden.contains("s3cr3t"), "the value is gone: {hidden}");
    }

    #[test]
    fn an_unknown_variant_hides_its_value_and_keeps_the_expected_list() {
        let hidden = hide_value("unknown variant `s3cr3t-mode`, expected `dev` or `prod`");
        assert_eq!(
            hidden, "unknown variant `<value>`, expected `dev` or `prod`",
            "the rejected spelling is gone, the allowed variants stay"
        );
    }

    #[test]
    fn an_invalid_value_hides_its_backticked_form() {
        let hidden = hide_value("invalid value: `s3cr3t`, expected a port");
        assert_eq!(hidden, "invalid value: `<value>`, expected a port");
    }
}
