//! The HTTP edge: the server that turns hyper connections into kernel
//! requests and kernel responses back into the wire.
//!
//! [`serve`] binds every configured address, speaks HTTP/1.1 and HTTP/2
//! cleartext, enforces the header/body/idle limits, resolves the client
//! address through [`HttpConfig::trusted_proxies`], assigns a request id,
//! looks the route up in the kernel, runs the handler under
//! `ctx.with_request(..).with_client(..)` and logs every answered request.
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
mod params;
mod proxy;
mod request;
mod serve;

pub mod failures;
pub mod log_events;

pub use config::HttpConfig;
pub use serve::serve;

/// The header names whose values never belong in a log (`authorization`,
/// `cookie`, `proxy-authorization`, `x-api-key`, `set-cookie`) and the check
/// against them: the edge itself logs no header value at all, and everything
/// that later adds one starts from this contract.
pub use params::{SECRET_HEADER_NAMES, is_secret_header};
