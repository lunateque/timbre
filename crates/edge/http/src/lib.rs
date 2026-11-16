//! The HTTP edge: the server that turns hyper connections into kernel
//! requests and kernel responses back into the wire.
//!
//! [`serve`] binds every configured address, speaks HTTP/1.1 and HTTP/2
//! cleartext, enforces the header/body/idle limits, resolves the client
//! address through [`HttpConfig::trusted_proxies`], assigns a request id,
//! looks the route up in the kernel, runs the handler under
//! `ctx.with_request(..).with_client(..)` and logs every answered request.
//! A handler `Err` becomes its failure's status with the public message as
//! the body. Shutdown stops accepting, drains in-flight responses for ten
//! seconds and returns.

#![warn(missing_docs)]

mod adapter;
mod config;
mod failures;
mod proxy;
mod serve;

pub mod log_events;

pub use config::HttpConfig;
pub use failures::{BIND_FAILED, BODY_FAILED, BODY_TIMEOUT, HttpFailure, PROXY_INVALID};
pub use serve::serve;

/// The header names whose values never belong in a log (`authorization`,
/// `cookie`, `proxy-authorization`) and the check against them: the edge
/// itself logs no header value at all, and everything that later adds one
/// starts from this contract.
pub use adapter::{SECRET_HEADER_NAMES, is_secret_header};
