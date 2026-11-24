//! The kernel's own log events.

use crate::log::LogEvent;

/// A signal handler returned an error or panicked; the other handlers still
/// ran. `panicked` says which.
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
/// The number of checks refused by the limiter for a client this interval.
pub const AUTH_THROTTLED: LogEvent = LogEvent::new("kernel.auth_throttled");
/// Refusals beyond the per-interval cap on distinct client summaries.
pub const AUTH_THROTTLED_UNTRACKED: LogEvent = LogEvent::new("kernel.auth_throttled_untracked");
/// No credential provider owns the kind presented by a client.
pub const AUTH_UNSUPPORTED: LogEvent = LogEvent::new("kernel.auth_unsupported");
/// A recognized identity lacked the permission a guarded route requires.
pub const ACCESS_DENIED: LogEvent = LogEvent::new("kernel.access_denied");
/// The writer dropped log lines while the sink lagged; `count` says how many.
pub const LOG_DROPPED: LogEvent = LogEvent::new("kernel.log_dropped");
/// The writer dropped audit lines (`write_always`) while the sink lagged;
/// `count` says how many. Reported apart from `kernel.log_dropped` so an
/// operator watching auth failures sees when the audit trail itself gapped.
pub const AUDIT_DROPPED: LogEvent = LogEvent::new("kernel.audit_dropped");
