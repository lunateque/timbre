//! The hello module's failures.

use timbre_kernel::{Failure, FailureCode};

/// Hello cannot answer.
pub const UNAVAILABLE: FailureCode = FailureCode::new("hello.unavailable");

/// Failures raised by the hello module.
#[derive(Debug)]
pub enum HelloFailure {
    /// Hello is not able to greet right now.
    Unavailable,
}

impl Failure for HelloFailure {
    fn code(&self) -> FailureCode {
        match self {
            Self::Unavailable => UNAVAILABLE,
        }
    }

    fn status(&self) -> u16 {
        503
    }

    fn public_message(&self) -> &'static str {
        "Hello is unavailable."
    }
}
