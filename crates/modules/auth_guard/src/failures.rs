//! What `auth_guard` can fail with.

use timbre_kernel::{Failure, FailureCode};

/// A setting in `[auth_guard]` is outside its usable range.
pub const INVALID_CONFIG: FailureCode = FailureCode::new("auth_guard.invalid_config");
/// The throttle has not completed startup configuration.
pub const NOT_CONFIGURED: FailureCode = FailureCode::new("auth_guard.not_configured");

/// The failures of this module.
#[derive(Debug)]
pub enum GuardFailure {
    /// A limit outside its range would throttle everything, nothing, or
    /// promise more memory than the table may use.
    InvalidConfig(&'static str),
    /// The throttle's startup state is unavailable.
    NotConfigured,
}

impl GuardFailure {
    /// The boxed form handlers return.
    #[must_use]
    pub fn boxed(self) -> Box<dyn Failure> {
        Box::new(self)
    }
}

impl Failure for GuardFailure {
    fn code(&self) -> FailureCode {
        match self {
            Self::InvalidConfig(_) => INVALID_CONFIG,
            Self::NotConfigured => NOT_CONFIGURED,
        }
    }

    fn status(&self) -> u16 {
        match self {
            Self::InvalidConfig(_) => 500,
            Self::NotConfigured => 503,
        }
    }

    fn public_message(&self) -> &str {
        match self {
            Self::InvalidConfig(message) => message,
            Self::NotConfigured => "The throttle is not configured.",
        }
    }
}
