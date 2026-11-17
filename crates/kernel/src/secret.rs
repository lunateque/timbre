//! A wrapper that keeps a value out of every accidental rendering.

/// The single mask every redaction writes, so logs, edge output and `Debug`
/// dumps hide values the same way.
pub const REDACTED: &str = "***";

/// Wraps tokens, passwords and keys so `Debug`, `Display` and logging print `***`.
/// No `PartialEq`: comparing secrets wants constant-time equality, which this
/// type must not pretend to offer.
#[derive(Clone)]
pub struct Secret<T>(T);

impl<T> Secret<T> {
    /// Wrap a value.
    pub fn new(value: T) -> Self {
        Self(value)
    }

    /// Borrow the hidden value; naming it `expose` keeps call sites honest.
    pub fn expose(&self) -> &T {
        &self.0
    }

    /// Take back the hidden value.
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T> std::fmt::Debug for Secret<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(REDACTED)
    }
}

impl<T> std::fmt::Display for Secret<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(REDACTED)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_and_display_hide_the_value() {
        assert_eq!(format!("{:?}", Secret::new("abc")), "***");
        assert_eq!(format!("{}", Secret::new("abc")), "***");
    }
}
