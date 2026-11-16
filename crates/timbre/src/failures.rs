//! The binary's own failure codes.

use timbre_kernel::{Failure, FailureCode};

/// The async executor could not start; the server does not run.
pub(crate) const RUNTIME_FAILED: FailureCode = FailureCode::new("timbre.runtime_failed");

/// The async executor could not start. `main` logs the operating system's
/// reason beside this code, since the failure itself carries none.
#[derive(Debug)]
pub(crate) struct RuntimeFailure;

impl Failure for RuntimeFailure {
    fn code(&self) -> FailureCode {
        RUNTIME_FAILED
    }

    fn status(&self) -> u16 {
        500
    }

    fn public_message(&self) -> &'static str {
        "The server failed to start its runtime."
    }
}
