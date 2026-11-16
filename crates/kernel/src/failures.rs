//! The kernel's own failure codes, mirroring the per-module `failures.rs` convention.

use crate::failure::{Failure, FailureCode};

/// A port was provided twice.
pub const PORT_TAKEN: FailureCode = FailureCode::new("kernel.port_taken");
/// The log filter string could not be parsed.
pub const LOG_FILTER_INVALID: FailureCode = FailureCode::new("kernel.log_filter_invalid");

/// Failures raised by the kernel itself.
#[derive(Debug)]
pub enum KernelFailure {
    /// `port` is the `type_name` of the port with two providers.
    PortTaken {
        /// The type name of the port that was provided twice.
        port: &'static str,
    },
    /// The `TIMBRE_LOG` (or config) filter string is malformed.
    LogFilterInvalid,
}

impl Failure for KernelFailure {
    fn code(&self) -> FailureCode {
        match self {
            Self::PortTaken { .. } => PORT_TAKEN,
            Self::LogFilterInvalid => LOG_FILTER_INVALID,
        }
    }

    fn status(&self) -> u16 {
        // Both arise during startup, where nothing has been served yet.
        500
    }

    fn public_message(&self) -> &'static str {
        "The server failed to start."
    }
}
