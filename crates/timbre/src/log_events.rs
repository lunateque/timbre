//! The binary's own log events.

use timbre_kernel::LogEvent;

/// The server finished starting.
pub(crate) const STARTED: LogEvent = LogEvent::new("timbre.started");
/// The `TIMBRE_LOG` filter could not be parsed; startup stops.
pub(crate) const LOG_FILTER_INVALID: LogEvent = LogEvent::new("timbre.log_filter_invalid");
/// A module refused to register; startup stops.
pub(crate) const REGISTRATION_FAILED: LogEvent = LogEvent::new("timbre.registration_failed");
