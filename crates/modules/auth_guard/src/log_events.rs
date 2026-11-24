//! The log events of `auth_guard`.

use timbre_kernel::LogEvent;

/// A window just filled up: further attempts against it are refused until
/// the window slides. The kernel logs each refused attempt itself, so this
/// says only when limiting started.
pub const WINDOW_FULL: LogEvent = LogEvent::new("auth_guard.window_full");

/// A completed throttle interval in which the table could not track new keys.
pub const TABLE_FULL: LogEvent = LogEvent::new("auth_guard.table_full");

/// Authentication was refused because the startup check has not run.
pub const NOT_CONFIGURED: LogEvent = LogEvent::new("auth_guard.not_configured");
