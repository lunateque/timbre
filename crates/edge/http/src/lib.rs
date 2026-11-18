//! The HTTP edge: the server that turns hyper connections into kernel
//! requests and kernel responses back into the wire.
//!
//! [`serve`] binds every configured address, speaks HTTP/1.1 and HTTP/2
//! cleartext, enforces the header/body/idle limits, resolves the client
//! address through [`HttpConfig::trusted_proxies`], assigns a request id,
//! and looks the route up in the kernel *before a single request body byte
//! is buffered*: 404 and 405 answers never read the body, and a
//! [`Guard::Requires`](timbre_kernel::Guard::Requires) route whose
//! credential rides the headers is checked first too, so a denied request
//! is refused without the body being held in memory. The exceptions are
//! public routes and `application/x-www-form-urlencoded` POSTs, whose
//! credentials may travel in the body: their body is read and merged into
//! the query (under `max_body_bytes` and `body_read_timeout`) before the
//! check runs. The kernel then runs the route's guard through
//! [`Kernel::guarded`](timbre_kernel::Kernel::guarded) under
//! `ctx.with_request(..)` with `guard_timeout` bounding the check, hands
//! the handler the authenticated context (whose canonical user name the
//! request log line then carries) and logs every answered request. A
//! denied guarded request is answered with its route's own `deny` response
//! and its handler never runs; a check that outlives `guard_timeout` is
//! answered 504 and logged as `http.guard_timeout`.
//! A handler `Err` becomes its failure's status with the public message as
//! the body, a handler that outlives `handler_timeout` is answered 504, and
//! a connection silent through protocol detection or header read is closed.
//! Shutdown stops accepting, drains in-flight responses for ten seconds and
//! returns.

#![warn(missing_docs)]

mod adapter;
mod body;
mod config;
mod detect;
mod form;
mod params;
mod proxy;
mod request;
mod serve;
mod write_guard;

#[cfg(feature = "fuzzing")]
#[doc(hidden)]
pub mod fuzzing;

pub mod failures;
pub mod log_events;

pub use config::HttpConfig;
pub use serve::serve;

/// The header names whose values never belong in a log (`authorization`,
/// `cookie`, `proxy-authorization`, `x-api-key`, `set-cookie`) and the check
/// against them: the edge itself logs no header value at all, and everything
/// that later adds one starts from this contract.
pub use params::{SECRET_HEADER_NAMES, is_secret_header};
