//! Timbre kernel: the contracts every module is written against.
//!
//! The kernel knows no module. It provides:
//!
//! - [`Registry`]: the registration surface a module declares routes, jobs,
//!   commands, permissions, credential providers, migrations, signal handlers
//!   and port implementations on, finished into the shared [`Kernel`];
//! - [`Signal`] and [`Bus`]: facts one module publishes to any number of
//!   listeners, delivered synchronously in registration order;
//! - [`Port`] and [`Ports`]: interfaces one module provides and others
//!   consume, where consumers must tolerate the provider being absent;
//! - [`Logger`] and [`log!`]: structured logging with compile-time-checked
//!   event names and a runtime level filter;
//! - [`Failure`] and [`FailureCode`]: errors that cross module boundaries
//!   with a stable code, an HTTP status and a user-safe message;
//! - [`Secret`]: values that must never reach a log or a `Debug` dump;
//! - [`Runtime`] with [`ThreadRuntime`]: where async work runs;
//!   [`block_on`] drives one future on a thread without an executor crate;
//! - [`http`], [`route`], [`page`], [`identity`], [`credential`] and
//!   [`migration`](crate::Migration): the shared shapes of requests, results,
//!   who is asking, what they may do, and how a module evolves its stored data.

#![warn(missing_docs)]

mod bus;
pub mod command;
pub mod credential;
mod ctx;
mod failure;
mod future;
pub mod identity;
mod jobs;
mod log;
pub mod migration;
mod names;
pub mod page;
mod port;
mod registry;
pub mod route;
mod runtime;
mod secret;

pub mod failures;
pub mod http;
pub mod log_events;
pub mod signals;

pub use bus::{Bus, Handler, Signal};
pub use command::{Command, CommandHandler};
pub use credential::{
    AuthOutcome, Credential, CredentialKind, CredentialProvider, RejectReason, Throttle, Verdict,
};
pub use ctx::{Ctx, RequestId};
pub use failure::{Failure, FailureCode, Fallible};
pub use failures::KernelFailure;
pub use future::{BoxFuture, block_on};
pub use identity::{Permission, PermissionEntry, Principal, UserId};
pub use jobs::JobHandler;
pub use log::{Clock, Format, Level, LogEvent, LogFilter, LogValue, Logger, Value};
pub use migration::Migration;
pub use page::{Cursor, Page};
pub use port::{Port, Ports};
pub use registry::{Kernel, Registry};
pub use route::{RouteHandler, RouteMatch};
pub use runtime::{Runtime, ThreadRuntime};
pub use secret::Secret;

#[cfg(test)]
mod tests {
    use crate::{Ctx, Kernel, Logger, ThreadRuntime};

    fn assert_send_sync<T: Send + Sync>() {}

    #[test]
    fn the_shared_types_cross_threads() {
        assert_send_sync::<Kernel>();
        assert_send_sync::<Ctx>();
        assert_send_sync::<Logger>();
        assert_send_sync::<ThreadRuntime>();
    }
}
