//! Failure codes and typed errors for the `activity` module.

use timbre_kernel::{Failure, FailureCode};

/// A `[activity]` configuration value is outside its supported bounds.
pub const INVALID_CONFIG: FailureCode = FailureCode::new("activity.invalid_config");
/// A caller supplied an invalid id, rating, timestamp, text field or queue index.
pub const INVALID_INPUT: FailureCode = FailureCode::new("activity.invalid_input");
/// The acting principal does not match the target user.
pub const FORBIDDEN: FailureCode = FailureCode::new("activity.forbidden");
/// The referenced item or bookmark does not exist.
pub const NOT_FOUND: FailureCode = FailureCode::new("activity.not_found");
/// The requested update would exceed a configured per-user cap.
pub const LIMIT_REACHED: FailureCode = FailureCode::new("activity.limit_reached");
/// The host installed no shared database resource or skipped config validation.
pub const NOT_CONFIGURED: FailureCode = FailureCode::new("activity.not_configured");

/// Errors produced by the `activity` module.
#[derive(Debug)]
pub enum ActivityFailure {
    /// A setting under `[activity]` failed range validation.
    InvalidConfig(&'static str),
    /// A caller supplied a malformed argument.
    InvalidInput(&'static str),
    /// A present principal attempted to act on another user's activity.
    Forbidden,
    /// The catalog item or bookmark was not found.
    NotFound(&'static str),
    /// A configured per-user cap was exceeded.
    LimitReached(&'static str),
    /// The shared database resource or startup configuration is absent.
    NotConfigured,
}

impl ActivityFailure {
    /// Box this failure for `Fallible` returns.
    pub(crate) fn boxed(self) -> Box<dyn Failure> {
        Box::new(self)
    }
}

impl Failure for ActivityFailure {
    fn code(&self) -> FailureCode {
        match self {
            Self::InvalidConfig(_) => INVALID_CONFIG,
            Self::InvalidInput(_) => INVALID_INPUT,
            Self::Forbidden => FORBIDDEN,
            Self::NotFound(_) => NOT_FOUND,
            Self::LimitReached(_) => LIMIT_REACHED,
            Self::NotConfigured => NOT_CONFIGURED,
        }
    }

    fn status(&self) -> u16 {
        match self {
            Self::InvalidConfig(_) => 500,
            Self::InvalidInput(_) => 400,
            Self::Forbidden => 403,
            Self::NotFound(_) => 404,
            Self::LimitReached(_) => 422,
            Self::NotConfigured => 503,
        }
    }

    fn public_message(&self) -> &str {
        match self {
            Self::InvalidConfig(reason)
            | Self::InvalidInput(reason)
            | Self::NotFound(reason)
            | Self::LimitReached(reason) => reason,
            Self::Forbidden => "You may only access or modify your own activity.",
            Self::NotConfigured => "Activity storage is not configured.",
        }
    }
}
