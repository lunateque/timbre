//! Timbre kernel: the contracts every module is written against.
//!
//! The kernel knows no module. It provides:
//!
//! - [`Registry`]: the registration surface a module declares routes, jobs,
//!   commands, permissions, credential providers, migrations, signal handlers
//!   and port implementations on, finished into the shared [`Kernel`];
//! - [`Signal`]: facts one module publishes to any number of listeners,
//!   delivered synchronously in registration order over the kernel's bus;
//! - [`Port`] and [`Ports`]: interfaces one module provides and others
//!   consume, where consumers must tolerate the provider being absent;
//! - [`Logger`] and [`log!`]: structured logging with compile-time-checked
//!   event names and a runtime level filter;
//! - [`Failure`] and [`FailureCode`]: errors that cross module boundaries
//!   with a stable code, an HTTP status and a user-safe message;
//! - [`Secret`]: values that must never reach a log or a `Debug` dump;
//! - [`Runtime`] with [`ThreadRuntime`]: where async work runs;
//!   [`block_on`] drives one future on a thread without an executor crate;
//! - [`http`], [`route`], [`guard`], [`page`], [`identity`], [`credential`] and
//!   [`migration`](crate::Migration): the shared shapes of requests, results,
//!   who is asking, what they may do, and how a module evolves its stored data.
//!
//! Deny by default: every [`route`] carries a [`Guard`]; the edge hands the
//! route match to [`Kernel::guarded`], which runs its check (extract,
//! authenticate, permission) and answers with an opaque [`Authorized`] whose
//! [`Authorized::run`](Authorized::run) is the only way to invoke a route
//! handler, so a protocol edge can neither skip the guard nor call a handler
//! it found on its own. The kernel names no wire protocol: a
//! [`credential::SchemeName`] is an opaque owner key, and the kernel only
//! routes by it and never interprets the fields that come with it.

#![warn(missing_docs)]

pub mod artwork;
mod bus;
pub mod catalog;
pub mod clock;
pub mod command;
pub mod credential;
mod ctx;
pub mod directory;
mod failure;
pub mod folders;
mod future;
pub mod guard;
pub mod identity;
mod jobs;
pub mod library_tree;
mod log;
pub mod migration;
mod names;
pub mod page;
mod port;
mod registry;
mod resources;
pub mod route;
mod runtime;
mod secret;

pub mod failures;
pub mod http;
pub mod log_events;
pub mod signals;

#[cfg(any(test, feature = "testing"))]
pub mod testing;

pub use bus::{Handler, Signal};
pub use command::{Command, CommandHandler};
pub use credential::{
    AuthOutcome, Credential, CredentialKind, CredentialProvider, RejectDetail, RejectReason,
    SchemeName, Throttle, Verdict,
};
pub use ctx::{Ctx, RequestId};
pub use failure::{Failure, FailureCode, Fallible};
pub use future::{BoxFuture, block_on};
pub use guard::{Authorized, Denial, Extraction, Guard};
pub use identity::{Identity, Permission, PermissionEntry, Principal, UserName};
pub use jobs::{JobHandler, run_periodic};
pub use log::{Clock, Format, Level, LogEvent, LogFilter, LogValue, Logger, Value};
pub use migration::Migration;
pub use page::{Cursor, MAX_PAGE_LIMIT, Page, PageRequest};
pub use port::{Port, Ports};
pub use registry::{Kernel, Registry};
pub use resources::Resources;
pub use route::{Matched, RouteHandler, RouteMatch};
pub use runtime::{Runtime, ThreadRuntime};
pub use secret::{REDACTED, Secret};

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
        // The HTTP edge borrows the request across the guard's authentication
        // await inside a spawned future, so `&Request` must be `Send`: the
        // streamed body is the only field that could lose that.
        assert_send_sync::<crate::http::Request>();
        assert_send_sync::<crate::http::Response>();
    }
}
