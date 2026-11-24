//! The per-call context: kernel handles plus an optional request id.

mod auth;

use crate::bus::Signal;
use crate::failure::{Failure, Fallible};
use crate::failures::KernelFailure;
use crate::future::BoxFuture;
use crate::identity::{Permission, Principal, UserName};
use crate::log::{Level, LogEvent, LogValue};
use crate::log_events::{ACCESS_DENIED, BLOCKING_PANICKED};
use crate::port::Ports;
use crate::registry::Kernel;
use crate::runtime::Slot;
use crate::signals::AccessDenied;
use std::net::IpAddr;
use std::panic::AssertUnwindSafe;
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
    principal: Option<Arc<Principal>>,
    client: Option<IpAddr>,
    cli: bool,
}

impl Ctx {
    /// The context of work no request caused: startup and background jobs.
    pub fn root(kernel: &Arc<Kernel>) -> Ctx {
        Ctx {
            kernel: Arc::clone(kernel),
            request: None,
            principal: None,
            client: None,
            cli: false,
        }
    }

    /// A copy of this context carrying `request`.
    #[must_use]
    pub fn with_request(&self, request: RequestId) -> Ctx {
        Ctx {
            request: Some(request),
            ..self.clone()
        }
    }

    /// A copy of this context acting as `principal`. Only the guard check
    /// builds a principal (from a provider's [`Identity`](crate::Identity)),
    /// so no handler can hand one to itself.
    ///
    /// ```compile_fail
    /// fn smuggle(ctx: timbre_kernel::Ctx, who: timbre_kernel::Principal) -> timbre_kernel::Ctx {
    ///     ctx.with_principal(who)
    /// }
    /// ```
    pub(crate) fn with_principal(&self, principal: Principal) -> Ctx {
        Ctx {
            principal: Some(Arc::new(principal)),
            ..self.clone()
        }
    }

    /// A copy of this context attributed to `client`; only
    /// [`Ctx::authenticate`] builds one, so a provider learns the address of
    /// the attempt it is checking without the credential having to carry it.
    fn with_client(&self, client: IpAddr) -> Ctx {
        Ctx {
            client: Some(client),
            ..self.clone()
        }
    }

    /// The request this context belongs to, if any.
    pub fn request(&self) -> Option<RequestId> {
        self.request
    }

    /// Mark work dispatched by the standalone CLI, not by the server.
    #[must_use]
    pub fn for_cli(mut self) -> Self {
        self.cli = true;
        self
    }

    /// Whether the work belongs to a standalone command.
    pub fn is_cli(&self) -> bool {
        self.cli
    }

    /// The address the credential check running on this context is
    /// attributed to; `None` outside a check. A credential provider uses it
    /// to record where a secret was last presented from.
    pub fn client(&self) -> Option<IpAddr> {
        self.client
    }

    /// The principal the work acts as, set by the authentication layer.
    pub fn principal(&self) -> Option<&Principal> {
        self.principal.as_deref()
    }

    /// The port table of the kernel.
    pub fn ports(&self) -> &Ports {
        self.kernel.ports()
    }

    /// Read a concrete runtime resource installed by the host, if present.
    pub fn resource<T: Send + Sync + 'static>(&self) -> Option<Arc<T>> {
        self.kernel.resources.get()
    }

    /// The kernel this context was built from: what the HTTP edge needs to
    /// reach the route table while owning only a context.
    #[must_use]
    pub fn kernel(&self) -> &Arc<Kernel> {
        &self.kernel
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

    /// Warn about a security event even when the log filter would hide it.
    pub fn log_security(&self, event: &LogEvent, fields: &[(&'static str, &dyn LogValue)]) {
        self.kernel
            .logger()
            .write_always(self.request, Level::Warn, event, fields);
    }

    /// Run `task` on the runtime without waiting for it.
    pub fn spawn(&self, task: BoxFuture<()>) {
        self.kernel.runtime().spawn(task);
    }

    /// A future resolving when the server is asked to stop.
    pub fn shutdown(&self) -> BoxFuture<()> {
        self.kernel.runtime().shutdown()
    }

    /// Run the blocking closure `work` off the async path and await its
    /// value. `T` is erased through a one-shot slot so [`Runtime`](crate::Runtime) stays
    /// object-safe; SQLite, file reads and decoding run this way.
    ///
    /// # Errors
    ///
    /// When `work` panics: the panic is caught, a `kernel.blocking_panicked`
    /// line is logged, and the awaiter resolves with that failure instead of
    /// waiting forever.
    pub fn blocking<T: Send + 'static>(
        &self,
        work: impl FnOnce() -> T + Send + 'static,
    ) -> BoxFuture<Fallible<T>> {
        let slot = Slot::<Option<T>>::new();
        let completion = slot.clone();
        let waiting = slot.wait();
        let ctx = self.clone();
        let running = self.kernel.runtime().run_blocking(Box::new(move || {
            let outcome = std::panic::catch_unwind(AssertUnwindSafe(work));
            if outcome.is_err() {
                // The default panic hook has already printed the panic itself.
                crate::log!(
                    ctx,
                    error,
                    BLOCKING_PANICKED,
                    code = KernelFailure::BlockingPanicked.code().as_str()
                );
            }
            completion.fill(outcome.ok());
        }));
        // Await the runtime's completion first: a runtime that starts the work
        // only when polled must be polled before the slot is read.
        Box::pin(async move {
            running.await;
            match waiting.await {
                Some(value) => Ok(value),
                None => Err(Box::new(KernelFailure::BlockingPanicked) as Box<dyn Failure>),
            }
        })
    }

    /// The audit line of a forbidden request: a recognized identity that did
    /// not hold the route's permission. Written unconditionally like the auth
    /// failure line, and publishes [`AccessDenied`] so limiting and auditing
    /// modules see what the wire answer hides.
    pub(crate) fn report_forbidden(&self, client: IpAddr, user: &UserName, permission: Permission) {
        self.kernel.logger().write_always(
            self.request,
            Level::Warn,
            &ACCESS_DENIED,
            &[
                ("client", &client.to_string() as &dyn LogValue),
                ("user", &user.as_str() as &dyn LogValue),
                ("permission", &permission.as_str() as &dyn LogValue),
            ],
        );
        self.publish(&AccessDenied {
            client,
            user: user.clone(),
            permission,
        });
    }
}
