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

/// Failures raised while serving HTTP.
#[derive(Debug)]
pub enum HttpFailure {
    /// `address` is the listen entry that failed; `reason` says how.
    Bind {
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
    /// The incoming body could not be read.
    BodyUnavailable,
    /// The body arrived slower than `HttpConfig::body_read_timeout` allows.
    BodyTimeout,
}

impl Failure for HttpFailure {
    fn code(&self) -> FailureCode {
        match self {
            Self::Bind { .. } => BIND_FAILED,
            Self::ProxyInvalid { .. } => PROXY_INVALID,
            Self::BodyUnavailable => BODY_FAILED,
            Self::BodyTimeout => BODY_TIMEOUT,
        }
    }

    fn status(&self) -> u16 {
        match self {
            Self::Bind { .. } | Self::ProxyInvalid { .. } => 500,
            Self::BodyUnavailable => 400,
            Self::BodyTimeout => 408,
        }
    }

    fn public_message(&self) -> &str {
        match self {
            Self::Bind { .. } | Self::ProxyInvalid { .. } => "The server failed to start.",
            Self::BodyUnavailable => "The request body could not be read.",
            Self::BodyTimeout => "The request body did not arrive in time.",
        }
    }
}
