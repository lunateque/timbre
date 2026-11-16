//! The per-call context: kernel handles plus an optional request id.

use crate::bus::Signal;
use crate::credential::{AuthOutcome, Credential, RejectReason, Throttle, Verdict};
use crate::failure::{Failure, Fallible};
use crate::failures::KernelFailure;
use crate::future::BoxFuture;
use crate::identity::Principal;
use crate::log::{Level, LogEvent, LogValue};
use crate::log_events::{AUTH_FAILED, BLOCKING_PANICKED};
use crate::port::Ports;
use crate::registry::Kernel;
use crate::runtime::Slot;
use crate::signals::AuthFailed;
use std::net::{IpAddr, Ipv4Addr};
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
    client: Option<IpAddr>,
    principal: Option<Arc<Principal>>,
}

impl Ctx {
    /// The context of work no request caused: startup and background jobs.
    pub fn root(kernel: &Arc<Kernel>) -> Ctx {
        Ctx {
            kernel: Arc::clone(kernel),
            request: None,
            client: None,
            principal: None,
        }
    }

    /// A copy of this context carrying `request`.
    #[must_use]
    pub fn with_request(&self, request: RequestId) -> Ctx {
        Ctx {
            kernel: Arc::clone(&self.kernel),
            request: Some(request),
            client: self.client,
            principal: self.principal.clone(),
        }
    }

    /// A copy of this context knowing the client address the HTTP edge
    /// resolved for the request (peer, or the untrusted proxy hop behind a
    /// trusted one). Authentication reports rejections against this address.
    #[must_use]
    pub fn with_client(&self, client: IpAddr) -> Ctx {
        Ctx {
            kernel: Arc::clone(&self.kernel),
            request: self.request,
            client: Some(client),
            principal: self.principal.clone(),
        }
    }

    /// A copy of this context acting as `principal`.
    #[must_use]
    pub fn with_principal(&self, principal: Principal) -> Ctx {
        Ctx {
            kernel: Arc::clone(&self.kernel),
            request: self.request,
            client: self.client,
            principal: Some(Arc::new(principal)),
        }
    }

    /// The request this context belongs to, if any.
    pub fn request(&self) -> Option<RequestId> {
        self.request
    }

    /// The principal the work acts as, set by the authentication layer.
    pub fn principal(&self) -> Option<&Principal> {
        self.principal.as_deref()
    }

    /// The port table of the kernel.
    pub fn ports(&self) -> &Ports {
        self.kernel.ports()
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

    /// Check `credential` against the module that owns its kind, or report
    /// `Unsupported` when the kind has none. A `Throttle` port is consulted
    /// first and can refuse without the provider being asked. Every rejection
    /// publishes [`AuthFailed`] and logs one line naming client, user and
    /// reason; the secret itself never reaches the log.
    pub fn authenticate(&self, credential: Credential) -> BoxFuture<AuthOutcome> {
        let ctx = self.clone();
        // Without `with_client` there is no HTTP peer to attribute attempts to.
        let client = ctx.client.unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED));
        let user = credential.user().map(str::to_owned);
        Box::pin(async move {
            let outcome = decide(ctx.clone(), credential, client, user.as_deref()).await;
            if let AuthOutcome::Rejected(reason) = outcome {
                crate::log!(
                    ctx,
                    warn,
                    AUTH_FAILED,
                    client = client.to_string(),
                    user = user.clone().unwrap_or_default(),
                    reason = reason.as_str()
                );
                ctx.publish(&AuthFailed {
                    client,
                    user,
                    reason,
                });
            }
            outcome
        })
    }
}

/// Throttle first, then the kind's provider; no logging or signals here.
async fn decide(
    ctx: Ctx,
    credential: Credential,
    client: IpAddr,
    user: Option<&str>,
) -> AuthOutcome {
    if ctx
        .ports()
        .get::<dyn Throttle>()
        .is_some_and(|throttle| !throttle.allow(client, user))
    {
        return AuthOutcome::Rejected(RejectReason::Throttled);
    }
    let Some(provider) = ctx.kernel.credential_provider(credential.kind()).cloned() else {
        return AuthOutcome::Unsupported;
    };
    // The Arc is cloned out first: `check` takes `ctx` by value, and a borrow
    // of the kernel table would keep `ctx` alive while it moves.
    match provider.check(ctx, credential).await {
        Verdict::Recognized(principal) => AuthOutcome::Recognized(principal),
        Verdict::Rejected(reason) => AuthOutcome::Rejected(reason),
    }
}
