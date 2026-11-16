//! The per-call context: kernel handles plus an optional request id.

use crate::bus::Signal;
use crate::log::{Level, LogEvent, LogValue};
use crate::port::Ports;
use crate::registry::Kernel;
use std::sync::Arc;

/// Identifies one request so every line it causes can be found together.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RequestId(u64);

impl RequestId {
    /// Wrap a raw identifier chosen by the edge that accepted the request.
    pub const fn new(value: u64) -> Self {
        Self(value)
    }
}

impl std::fmt::Display for RequestId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // 16 hex digits: the full u64, zero-padded so ids sort lexicographically.
        write!(f, "{:016x}", self.0)
    }
}

/// What every handler and port call receives: access to the kernel's services
/// and the request that caused the work, when there was one.
#[derive(Clone)]
pub struct Ctx {
    kernel: Arc<Kernel>,
    request: Option<RequestId>,
}

impl Ctx {
    /// The context of work no request caused: startup and background jobs.
    pub fn root(kernel: &Arc<Kernel>) -> Ctx {
        Ctx {
            kernel: Arc::clone(kernel),
            request: None,
        }
    }

    /// A copy of this context carrying `request`.
    #[must_use]
    pub fn with_request(&self, request: RequestId) -> Ctx {
        Ctx {
            kernel: Arc::clone(&self.kernel),
            request: Some(request),
        }
    }

    /// The request this context belongs to, if any.
    pub fn request(&self) -> Option<RequestId> {
        self.request
    }

    /// The port table of the kernel.
    pub fn ports(&self) -> &Ports {
        self.kernel.ports()
    }

    /// Deliver `signal` to every registered handler of its type.
    pub fn publish<S: Signal>(&self, signal: &S) {
        self.kernel.bus().publish(self, signal);
    }

    /// Whether a line for `event` at `level` would be written.
    pub fn log_enabled(&self, level: Level, event: &LogEvent) -> bool {
        self.kernel.logger().enabled(level, event.module())
    }

    /// Write one already-checked line. Prefer the `log!` macro, which skips
    /// evaluating fields for disabled lines.
    pub fn log(&self, level: Level, event: &LogEvent, fields: &[(&'static str, &dyn LogValue)]) {
        self.kernel
            .logger()
            .write(self.request, level, event, fields);
    }
}
