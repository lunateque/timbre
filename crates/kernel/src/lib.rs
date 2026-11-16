//! Timbre kernel: the contracts every module is written against.
//!
//! The kernel knows no module. It provides:
//!
//! - [`Registry`]: the registration surface a module declares handlers and
//!   port implementations on, finished into the shared [`Kernel`];
//! - [`Signal`] and [`Bus`]: facts one module publishes to any number of
//!   listeners, delivered synchronously in registration order;
//! - [`Port`] and [`Ports`]: interfaces one module provides and others
//!   consume, where consumers must tolerate the provider being absent;
//! - [`Logger`] and [`log!`]: structured logging with compile-time-checked
//!   event names and a runtime level filter;
//! - [`Failure`] and [`FailureCode`]: errors that cross module boundaries
//!   with a stable code, an HTTP status and a user-safe message;
//! - [`Secret`]: values that must never reach a log or a `Debug` dump.

#![warn(missing_docs)]

mod bus;
mod ctx;
mod failure;
mod log;
mod names;
mod port;
mod registry;
mod secret;

pub mod failures;
pub mod log_events;
pub mod signals;

pub use bus::{Bus, Handler, Signal};
pub use ctx::{Ctx, RequestId};
pub use failure::{Failure, FailureCode, Fallible};
pub use failures::KernelFailure;
pub use log::{Clock, Format, Level, LogEvent, LogFilter, LogValue, Logger, Value};
pub use port::{Port, Ports};
pub use registry::{Kernel, Registry};
pub use secret::Secret;

#[cfg(test)]
mod tests {
    use crate::{Ctx, Kernel, Logger};

    fn assert_send_sync<T: Send + Sync>() {}

    #[test]
    fn the_shared_types_cross_threads() {
        assert_send_sync::<Kernel>();
        assert_send_sync::<Ctx>();
        assert_send_sync::<Logger>();
    }
}
