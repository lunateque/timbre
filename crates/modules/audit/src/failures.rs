//! What `audit` can fail with.

use timbre_kernel::{Failure, FailureCode};

/// A configuration bound was not met.
pub const INVALID_CONFIG: FailureCode = FailureCode::new("audit.invalid_config");
/// The host installed no database to journal into.
pub const NOT_CONFIGURED: FailureCode = FailureCode::new("audit.not_configured");

/// The failures of this module.
#[derive(Debug)]
pub enum AuditFailure {
    /// A configured limit or interval was zero.
    InvalidConfig,
    /// There is nowhere to write the journal.
    NotConfigured,
}

impl AuditFailure {
    /// The boxed form handlers return.
    #[must_use]
    pub fn boxed(self) -> Box<dyn Failure> {
        Box::new(self)
    }
}

impl Failure for AuditFailure {
    fn code(&self) -> FailureCode {
        match self {
            Self::InvalidConfig => INVALID_CONFIG,
            Self::NotConfigured => NOT_CONFIGURED,
        }
    }

    fn status(&self) -> u16 {
        match self {
            Self::InvalidConfig => 400,
            Self::NotConfigured => 503,
        }
    }

    fn public_message(&self) -> &str {
        match self {
            Self::InvalidConfig => {
                "queue_capacity must be at least 2, keep_days and drain_seconds at least 1"
            }
            Self::NotConfigured => "The server has no journal.",
        }
    }
}
