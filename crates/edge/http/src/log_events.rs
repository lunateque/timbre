//! The HTTP edge's log events.

use timbre_kernel::LogEvent;

/// The server listens on an address; `addr` is the bound address, so an
/// ephemeral port is reported as the number it got.
pub const LISTENING: LogEvent = LogEvent::new("http.listening");
/// One request was answered: method, path, status, bytes, duration, client.
pub const REQUEST: LogEvent = LogEvent::new("http.request");
/// A route handler returned a failure; the code names it.
pub const HANDLER_FAILED: LogEvent = LogEvent::new("http.handler_failed");
/// A request body stream failed; the client is answered 400 with
/// `http.body_failed` as the code of the failure that says so.
pub const BODY_FAILED: LogEvent = LogEvent::new("http.body_failed");
/// A request body arrived slower than `body_read_timeout` allows; the client
/// is answered 408.
pub const BODY_TIMEOUT: LogEvent = LogEvent::new("http.body_timeout");
/// The URL or form carried more query+form pairs than the edge accepts, or a
/// longer key than it allows; `source` says url or form, `reason` which limit
/// was crossed and `pairs` how many arrived.
pub const PARAMS_REFUSED: LogEvent = LogEvent::new("http.params_refused");
/// A handler did not produce its response within `handler_timeout`; the
/// client is answered 504.
pub const HANDLER_TIMEOUT: LogEvent = LogEvent::new("http.handler_timeout");
/// A route's guard check did not finish within `guard_timeout`; the client
/// is answered 504.
pub const GUARD_TIMEOUT: LogEvent = LogEvent::new("http.guard_timeout");
/// A streaming response body failed after its head was already on the wire;
/// the body ends where it broke, so only the log says so.
pub const STREAM_FAILED: LogEvent = LogEvent::new("http.stream_failed");
/// A handler put a `content-length` the edge cannot trust on a streamed
/// response, so it was dropped and hyper framed the body instead; `value` is
/// the received text.
pub const CONTENT_LENGTH_INVALID: LogEvent = LogEvent::new("http.content_length_invalid");
/// A peer already holds `max_connections_per_client` connections; its new
/// connection is closed at once. `peer` is the address, `limit` the cap.
pub const CONNECTIONS_FULL: LogEvent = LogEvent::new("http.connections_full");
/// A response write made no progress for `send_stall_timeout`: the peer took
/// its bytes away without closing the connection (or the network quietly
/// died), so the connection is failed and its slot released. `peer` is the
/// address, `ms` the bound that expired.
pub const SEND_STALL: LogEvent = LogEvent::new("http.send_stall");
