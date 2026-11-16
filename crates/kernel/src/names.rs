//! Shared validation of dotted names: log events and failure codes.

/// A valid name is exactly two segments joined by one `.`, each segment
/// starting with `a-z` and continuing with `a-z`, `0-9` or `_`.
pub(crate) const fn is_valid(name: &str) -> bool {
    let mut rest = name.as_bytes();
    let mut seen_dot = false;
    let mut at_start = true;
    while let [byte, tail @ ..] = rest {
        if *byte == b'.' {
            if at_start || seen_dot {
                return false;
            }
            seen_dot = true;
            at_start = true;
        } else if at_start {
            if !byte.is_ascii_lowercase() {
                return false;
            }
            at_start = false;
        } else if !is_tail_byte(*byte) {
            return false;
        }
        rest = tail;
    }
    seen_dot && !at_start
}

/// A valid single segment: starts with `a-z`, continues with `a-z0-9_`.
pub(crate) const fn is_valid_segment(name: &str) -> bool {
    let mut rest = name.as_bytes();
    let mut at_start = true;
    while let [byte, tail @ ..] = rest {
        if at_start {
            if !byte.is_ascii_lowercase() {
                return false;
            }
            at_start = false;
        } else if !is_tail_byte(*byte) {
            return false;
        }
        rest = tail;
    }
    !at_start
}

/// A byte allowed after the leading letter of a name segment.
const fn is_tail_byte(byte: u8) -> bool {
    byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_'
}

/// The module part of a valid `<module>.<name>` string.
pub(crate) fn module_of(name: &'static str) -> &'static str {
    name.split_once('.').map_or(name, |(module, _)| module)
}

#[cfg(test)]
mod tests {
    use super::is_valid;
    use crate::FailureCode;

    const VALID: [&str; 2] = ["hello.greeted", "a.b_c9"];
    const INVALID: [&str; 9] = [
        "",
        "hello",
        "Hello.x",
        "hello.",
        ".x",
        "hello.x.y",
        "hello.9x",
        "hello.x-y",
        "hello..x",
    ];

    #[test]
    fn is_valid_accepts_and_rejects_the_table() {
        for name in VALID {
            assert!(is_valid(name), "{name} must be valid");
        }
        for name in INVALID {
            assert!(!is_valid(name), "{name} must be invalid");
        }
    }

    #[test]
    fn failure_code_uses_the_same_table() {
        for name in VALID {
            assert!(
                FailureCode::try_new(name).is_some(),
                "{name} must be accepted"
            );
        }
        for name in INVALID {
            assert!(
                FailureCode::try_new(name).is_none(),
                "{name} must be rejected"
            );
        }
    }

    #[test]
    fn module_is_the_prefix() {
        let code = FailureCode::try_new("hello.greeted").expect("valid code");
        assert_eq!(code.module(), "hello");
    }
}
