//! The HTTP edge's failures.

use timbre_kernel::{Failure, FailureCode};

/// A listen address could not be parsed or bound.
pub const BIND_FAILED: FailureCode = FailureCode::new("http.bind_failed");
/// A trusted proxy entry is not an address or a CIDR prefix.
pub const PROXY_INVALID: FailureCode = FailureCode::new("http.proxy_invalid");
/// The client sent a request body that ended in error before the handler
/// could consume it.
pub const BODY_FAILED: FailureCode = FailureCode::new("http.body_failed");
/// The client sent its request body slower than
/// [`HttpConfig::body_read_timeout`](crate::HttpConfig::body_read_timeout).
pub const BODY_TIMEOUT: FailureCode = FailureCode::new("http.body_timeout");
/// A route handler did not produce its response within
/// [`HttpConfig::handler_timeout`](crate::HttpConfig::handler_timeout).
pub const HANDLER_TIMEOUT: FailureCode = FailureCode::new("http.handler_timeout");
/// A route's guard check did not finish within
/// [`HttpConfig::guard_timeout`](crate::HttpConfig::guard_timeout).
pub const GUARD_TIMEOUT: FailureCode = FailureCode::new("http.guard_timeout");

/// Failures raised while serving HTTP.
#[derive(Debug)]
pub enum HttpFailure {
    /// `address` is the listen entry that failed; `reason` says how.
    BindFailed {
        /// The configured address that could not be used.
        address: String,
        /// The operating system's or parser's complaint.
        reason: String,
    },
    /// `entry` is the unparsable trusted proxy.
    ProxyInvalid {
        /// The rejected proxy entry.
        entry: String,
    },
    /// The incoming body could not be read; the client is answered 400.
    BodyFailed,
    /// The body arrived slower than `HttpConfig::body_read_timeout` allows.
    BodyTimeout,
    /// The handler was still running when `HttpConfig::handler_timeout`
    /// expired; the response it eventually produces is dropped.
    HandlerTimeout,
    /// The guard check was still running when `HttpConfig::guard_timeout`
    /// expired; the future outcome of the check (including a response a
    /// handler it authorized eventually produces) is dropped.
    GuardTimeout,
}

impl Failure for HttpFailure {
    fn code(&self) -> FailureCode {
        match self {
            Self::BindFailed { .. } => BIND_FAILED,
            Self::ProxyInvalid { .. } => PROXY_INVALID,
            Self::BodyFailed => BODY_FAILED,
            Self::BodyTimeout => BODY_TIMEOUT,
            Self::HandlerTimeout => HANDLER_TIMEOUT,
            Self::GuardTimeout => GUARD_TIMEOUT,
        }
    }

    fn status(&self) -> u16 {
        match self {
            Self::BindFailed { .. } | Self::ProxyInvalid { .. } => 500,
            Self::BodyFailed => 400,
            Self::BodyTimeout => 408,
            Self::HandlerTimeout | Self::GuardTimeout => 504,
        }
    }

    fn public_message(&self) -> &str {
        match self {
            Self::BindFailed { .. } | Self::ProxyInvalid { .. } => "The server failed to start.",
            Self::BodyFailed => "The request body could not be read.",
            Self::BodyTimeout => "The request body did not arrive in time.",
            Self::HandlerTimeout | Self::GuardTimeout => "The request took too long to answer.",
        }
    }
}
