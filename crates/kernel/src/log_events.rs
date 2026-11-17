//! The kernel's own log events.

use crate::log::LogEvent;

/// A signal handler returned an error; the other handlers still ran.
pub const HANDLER_FAILED: LogEvent = LogEvent::new("kernel.handler_failed");
/// A blocking closure panicked; the awaiter receives a `kernel.blocking_panicked` failure.
pub const BLOCKING_PANICKED: LogEvent = LogEvent::new("kernel.blocking_panicked");
/// A background job started.
pub const JOB_STARTED: LogEvent = LogEvent::new("kernel.job_started");
/// A background job finished, on its own or because the server shut down.
pub const JOB_FINISHED: LogEvent = LogEvent::new("kernel.job_finished");
/// A background job returned a failure; the code names it.
pub const JOB_FAILED: LogEvent = LogEvent::new("kernel.job_failed");
/// A credential check was rejected; the secret itself is never logged.
pub const AUTH_FAILED: LogEvent = LogEvent::new("kernel.auth_failed");
/// The writer dropped log lines while the sink lagged; `count` says how many.
pub const LOG_DROPPED: LogEvent = LogEvent::new("kernel.log_dropped");
