//! The HTTP edge's log events.

use timbre_kernel::LogEvent;

/// The server listens on an address; `addr` is the bound address, so an
/// ephemeral port is reported as the number it got.
pub const LISTENING: LogEvent = LogEvent::new("http.listening");
/// One request was answered: method, path, status, bytes, duration, client.
pub const REQUEST: LogEvent = LogEvent::new("http.request");
/// A route handler returned a failure; the code names it.
pub const HANDLER_FAILED: LogEvent = LogEvent::new("http.handler_failed");
/// A request body arrived slower than `body_read_timeout` allows; the client
/// is answered 408.
pub const BODY_TIMEOUT: LogEvent = LogEvent::new("http.body_timeout");
