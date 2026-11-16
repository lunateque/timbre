//! `size`: no `.rs` file over 400 lines.
//!
//! The limit is the point where scrolling stops being navigation; any unit past it,
//! the xtask lexer included, is split into files.

use super::{Source, Violation};

pub(crate) const MAX_LINES: usize = 400;

pub(crate) fn check(sources: &[Source<'_>]) -> Vec<Violation> {
    let mut violations = Vec::new();
    for file in sources {
        // The last line counts even without a trailing newline; `lines()` already
        // ignores the empty remainder after a final newline.
        let counted = file.text.lines().count();
        if counted > MAX_LINES {
            violations.push(Violation::new(
                "size",
                file.path,
                Some(counted),
                format!("{counted} lines, at most {MAX_LINES}"),
            ));
        }
    }
    violations
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::check::source;

    #[test]
    fn just_over_the_limit_is_reported_and_exactly_it_is_not() {
        let big_text = "fn f() {}\n".repeat(401);
        let exact_text = "fn f() {}\n".repeat(400);
        let big = source("a.rs", &big_text);
        let exact = source("b.rs", &exact_text);
        assert_eq!(check(&[big]).len(), 1);
        assert!(check(&[exact]).is_empty());
    }
}
