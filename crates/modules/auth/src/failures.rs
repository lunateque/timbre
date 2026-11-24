//! What `auth` can fail with.

use timbre_kernel::{Failure, FailureCode};

/// A configuration bound was not met.
pub const INVALID_CONFIG: FailureCode = FailureCode::new("auth.invalid_config");
/// A stored name cannot identify a user.
pub const STORED_USER_INVALID: FailureCode = FailureCode::new("auth.stored_user_invalid");
/// The named user or device does not exist.
pub const NOT_FOUND: FailureCode = FailureCode::new("auth.not_found");
/// A user with that name already exists.
pub const EXISTS: FailureCode = FailureCode::new("auth.exists");
/// The permission is not declared by any registered module.
pub const UNKNOWN_PERMISSION: FailureCode = FailureCode::new("auth.unknown_permission");
/// The host installed neither configuration nor a database.
pub const NOT_CONFIGURED: FailureCode = FailureCode::new("auth.not_configured");

/// The failures of this module.
#[derive(Debug)]
pub enum AuthFailure {
    /// The configured default expiry is out of range.
    InvalidConfig(u32),
    /// The stored user name did not canonicalize.
    StoredUserInvalid(String),
    /// Nothing is stored under that name or id.
    NotFound {
        /// Whether the lookup was for a user or a device.
        kind: &'static str,
        /// The value that was looked up, only for logs.
        value: String,
    },
    /// The name is taken.
    Exists(String),
    /// No registered module declares this permission, so granting it would
    /// store a string nothing ever checks.
    UnknownPermission(String),
    /// The host did not install the resources this module needs.
    NotConfigured,
}

impl AuthFailure {
    /// The boxed form handlers return.
    #[must_use]
    pub fn boxed(self) -> Box<dyn Failure> {
        Box::new(self)
    }
}

impl Failure for AuthFailure {
    fn code(&self) -> FailureCode {
        match self {
            Self::InvalidConfig(_) => INVALID_CONFIG,
            Self::StoredUserInvalid(_) => STORED_USER_INVALID,
            Self::NotFound { .. } => NOT_FOUND,
            Self::Exists(_) => EXISTS,
            Self::UnknownPermission(_) => UNKNOWN_PERMISSION,
            Self::NotConfigured => NOT_CONFIGURED,
        }
    }

    fn status(&self) -> u16 {
        match self {
            Self::NotFound { .. } => 404,
            Self::Exists(_) => 409,
            Self::NotConfigured => 503,
            Self::StoredUserInvalid(_) => 500,
            _ => 400,
        }
    }

    fn public_message(&self) -> &str {
        match self {
            Self::InvalidConfig(_) => "device_default_expiry_days must be between 1 and 36500.",
            Self::StoredUserInvalid(_) => "The stored user name is invalid.",
            Self::NotFound { kind: "user", .. } => "The user does not exist.",
            Self::NotFound { .. } => "The device does not exist.",
            Self::Exists(_) => "The user already exists.",
            Self::UnknownPermission(_) => "No registered module declares that permission.",
            Self::NotConfigured => "The server is not configured for users.",
        }
    }
}
