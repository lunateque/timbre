//! The log events of `auth`.

use timbre_kernel::LogEvent;

/// A user was created.
pub const USER_ADDED: LogEvent = LogEvent::new("auth.user_added");
/// A user and everything of theirs was deleted.
pub const USER_REMOVED: LogEvent = LogEvent::new("auth.user_removed");
/// Permissions were granted to a user.
pub const PERMISSION_GRANTED: LogEvent = LogEvent::new("auth.permission_granted");
/// Permissions were taken from a user.
pub const PERMISSION_REVOKED: LogEvent = LogEvent::new("auth.permission_revoked");
/// A device was issued; the secret is printed, never logged.
pub const DEVICE_ISSUED: LogEvent = LogEvent::new("auth.device_issued");
/// A device kept its identity and got a new secret.
pub const DEVICE_REISSUED: LogEvent = LogEvent::new("auth.device_reissued");
/// A device can no longer authenticate.
pub const DEVICE_REVOKED: LogEvent = LogEvent::new("auth.device_revoked");
/// A command argument does not have the required shape.
pub const INVALID_ARGUMENT: LogEvent = LogEvent::new("auth.invalid_argument");
/// A user, device or declared permission lookup rejected the supplied value.
pub const LOOKUP_FAILED: LogEvent = LogEvent::new("auth.lookup_failed");
/// A stored account was skipped because its name cannot identify a user.
pub const INVALID_STORED_NAME: LogEvent = LogEvent::new("auth.invalid_stored_name");
/// A credential could not be checked because verification failed internally.
pub const VERIFICATION_FAILED: LogEvent = LogEvent::new("auth.verification_failed");
/// The last-use job could not write; the next pass tries again.
pub const LAST_USE_FAILED: LogEvent = LogEvent::new("auth.last_use_failed");
/// The last-use job wrote the pending device activity to storage.
pub const LAST_USE_WRITTEN: LogEvent = LogEvent::new("auth.last_use_written");
