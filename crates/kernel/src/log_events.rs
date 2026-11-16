//! The kernel's own log events.

use crate::log::LogEvent;

/// A signal handler returned an error; the other handlers still ran.
pub const HANDLER_FAILED: LogEvent = LogEvent::new("kernel.handler_failed");
