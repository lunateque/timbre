//! The single place xtask writes to: stdout stays free for nothing else.

use std::io::Write;

/// Print one line. Write errors are dropped: a closed stderr must not change the
/// exit status of a command that has already decided.
pub(crate) fn line(text: &str) {
    let stderr = std::io::stderr();
    let mut sink = stderr.lock();
    let _ = writeln!(sink, "{text}");
}
