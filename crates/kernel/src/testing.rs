//! Test support for asserting on log output, behind the `testing` feature:
//! the kernel's own tests use it, and modules and edge crates adopt it instead
//! of copying a sink and a clock into every crate.
//!
//! Lines reach the sink from the writer thread, so read the sink only after
//! [`crate::Logger::flush`] returned `true` (or after dropping the logger,
//! which joins the writer within its bound).

use std::io::Write;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The fixed timestamp every test line carries: 2026-11-16T12:34:56.789Z.
pub const FIXED_MILLIS: u64 = 1_794_832_496_789;

/// A clock that always returns [`FIXED_MILLIS`].
#[must_use]
pub fn fixed() -> SystemTime {
    UNIX_EPOCH + Duration::from_millis(FIXED_MILLIS)
}

/// A clock frozen at the Unix epoch, for tests that only compare line order.
#[must_use]
pub fn epoch() -> SystemTime {
    UNIX_EPOCH
}

/// A sink that collects every byte behind a mutex; cheap to share with tests.
#[derive(Clone, Default)]
pub struct Shared(Arc<Mutex<Vec<u8>>>);

impl Write for Shared {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        // Lock errors mean another test panicked while writing; recovering
        // keeps this test's own assertion the one that matters.
        let mut guard = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        guard.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Shared {
    /// Everything written so far.
    #[must_use]
    pub fn text(&self) -> String {
        let guard = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        String::from_utf8_lossy(&guard).into_owned()
    }

    /// The written lines, without their newlines.
    #[must_use]
    pub fn lines(&self) -> Vec<String> {
        self.text().lines().map(str::to_owned).collect()
    }

    /// The written lines whose JSON `"event"` is `event`.
    #[must_use]
    pub fn lines_for(&self, event: &str) -> Vec<String> {
        let needle = format!("\"event\":\"{event}\"");
        self.text()
            .lines()
            .filter(|line| line.contains(&needle))
            .map(str::to_owned)
            .collect()
    }
}

/// The value of one JSON string field of `line`, located by name so tests do
/// not depend on field order. Returns `None` when the field is absent or is
/// not a string.
#[must_use]
pub fn json_string_field(line: &str, key: &str) -> Option<String> {
    let after = json_field_rest(line, key)?;
    let rest = after.strip_prefix('"')?;
    let mut value = String::new();
    let mut escaped = false;
    for ch in rest.chars() {
        if escaped {
            value.push(ch);
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else if ch == '"' {
            return Some(value);
        } else {
            value.push(ch);
        }
    }
    None
}

/// The raw (unquoted) text of one JSON field of `line` up to the next `,` or
/// `}`; also returns bare tokens such as numbers, `true` and `null` (string
/// values come back without their quotes). Returns `None` when absent.
#[must_use]
pub fn json_field(line: &str, key: &str) -> Option<String> {
    let after = json_field_rest(line, key)?;
    if after.starts_with('"') {
        return json_string_field(line, key);
    }
    // A truncated line without any terminator yields the whole remainder.
    let end = after.find([',', '}']).unwrap_or(after.len());
    let (head, _) = after.split_at(end);
    Some(head.to_owned())
}

/// The text right after `"<key>":` in `line`. The needle carries the opening
/// quote, so a match cannot land inside a longer key such as `f_level`.
fn json_field_rest<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let needle = format!("\"{key}\":");
    line.split_once(&needle).map(|(_, after)| after)
}
