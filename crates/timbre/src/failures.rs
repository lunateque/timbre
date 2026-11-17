//! The binary's own failure codes.

use timbre_kernel::{Failure, FailureCode};

/// The async executor could not start; the server does not run.
pub(crate) const RUNTIME_FAILED: FailureCode = FailureCode::new("timbre.runtime_failed");
/// `[server].data_dir` could not be created or kept owner-only; startup stops.
pub(crate) const DATA_DIR_FAILED: FailureCode = FailureCode::new("timbre.data_dir_failed");
/// A usage error whose `timbre.usage` line the handler already logged; exit 2.
pub(crate) const USAGE: FailureCode = FailureCode::new("timbre.usage");
/// A failed command whose explanatory line the handler already logged;
/// exit 1 without a second `timbre.command_failed` line.
pub(crate) const REPORTED: FailureCode = FailureCode::new("timbre.reported");

/// Failures the binary itself raises.
#[derive(Debug)]
pub(crate) enum TimbreFailure {
    /// The async executor could not start; `main` logs the operating
    /// system's reason beside this code, since the failure carries none.
    Runtime,
    /// The data directory could not be created or made owner-only;
    /// `reason` names the path and the operating system's complaint.
    DataDir {
        /// The failed directory and error.
        // Read through the failure's `Debug`, which the `database_failed`
        // line carries; dead-code analysis ignores derived `Debug`.
        #[expect(dead_code, reason = "reaches the log through Debug")]
        reason: String,
    },
    /// An unusable command line; the `timbre.usage` line is already logged.
    Usage,
    /// A failure already reported by the code raising this one; the exit
    /// is 1 and no further line belongs to it.
    Reported,
}

impl Failure for TimbreFailure {
    fn code(&self) -> FailureCode {
        match self {
            Self::Runtime => RUNTIME_FAILED,
            Self::DataDir { .. } => DATA_DIR_FAILED,
            Self::Usage => USAGE,
            Self::Reported => REPORTED,
        }
    }

    fn status(&self) -> u16 {
        match self {
            Self::Usage => 400,
            Self::Runtime | Self::DataDir { .. } | Self::Reported => 500,
        }
    }

    fn public_message(&self) -> &'static str {
        match self {
            Self::Runtime => "The server failed to start its runtime.",
            Self::DataDir { .. } | Self::Reported => "The server failed to start.",
            Self::Usage => "The command line is not usable.",
        }
    }
}
