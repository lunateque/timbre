//! The config edge's failures.

use timbre_kernel::{Failure, FailureCode};

/// The config file or one of its sections is unusable.
pub const INVALID: FailureCode = FailureCode::new("config.invalid");

/// Failures raised while reading configuration.
#[derive(Debug)]
pub enum ConfigFailure {
    /// `file` is the path that was read; `line`, when present, is where the
    /// problem is in it.
    Invalid {
        /// The offending file.
        file: String,
        /// One-based line of the problem, when the parser reported a span.
        line: Option<usize>,
        /// What is wrong, in the parser's own words.
        reason: String,
    },
}

impl Failure for ConfigFailure {
    fn code(&self) -> FailureCode {
        INVALID
    }

    fn status(&self) -> u16 {
        // Bad configuration stops startup; nothing was served.
        500
    }

    fn public_message(&self) -> &'static str {
        "The server failed to start."
    }
}
