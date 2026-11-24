//! The per-call context: kernel handles plus an optional request id.

use crate::bus::Signal;
use crate::credential::{AuthOutcome, Credential, RejectReason, Verdict};
use crate::failure::{Failure, Fallible};
use crate::failures::KernelFailure;
use crate::future::BoxFuture;
use crate::identity::{Permission, Principal, UserName};
use crate::log::{Level, LogEvent, LogValue};
use crate::log_events::{ACCESS_DENIED, AUTH_FAILED, BLOCKING_PANICKED};
use crate::port::Ports;
use crate::registry::Kernel;
use crate::runtime::Slot;
use crate::signals::{AccessDenied, AuthFailed};
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
}

impl Ctx {
    /// The context of work no request caused: startup and background jobs.
    pub fn root(kernel: &Arc<Kernel>) -> Ctx {
        Ctx {
            kernel: Arc::clone(kernel),
            request: None,
            principal: None,
        }
    }

    /// A copy of this context carrying `request`.
    #[must_use]
    pub fn with_request(&self, request: RequestId) -> Ctx {
        Ctx {
            kernel: Arc::clone(&self.kernel),
            request: Some(request),
            principal: self.principal.clone(),
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
            kernel: Arc::clone(&self.kernel),
            request: self.request,
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
    /// `Unsupported` when the kind has none. `client` is the address the
    /// attempt is attributed to, required so a CLI path cannot silently
    /// blame an unspecified host. A registered `Throttle` is consulted first
    /// and can refuse without the provider being asked. A name the credential
    /// carries is canonicalized before either sees it; one that cannot
    /// canonicalize is rejected as `Invalid` without asking anyone. Every
    /// rejection publishes [`AuthFailed`] and logs one `kernel.auth_failed`
    /// line naming client, canonical user (or the clipped candidate when the
    /// given name was not a user name at all) and reason; the secret itself
    /// never reaches the log.
    pub fn authenticate(&self, credential: Credential, client: IpAddr) -> BoxFuture<AuthOutcome> {
        let ctx = self.clone();
        let (credential, named) = match canonicalize(credential) {
            Ok(passed) => passed,
            Err((candidate, reason)) => {
                return Box::pin(async move {
                    ctx.report_rejection(client, None, reason, Some(candidate.as_str()));
                    AuthOutcome::Rejected(reason)
                });
            }
        };
        Box::pin(async move {
            let outcome = decide(ctx.clone(), credential, client, named.as_ref()).await;
            if let AuthOutcome::Rejected(reason) = outcome {
                ctx.report_rejection(client, named.as_ref(), reason, None);
            }
            outcome
        })
    }

    /// The one audit line every rejection gets. Written at `warn`
    /// unconditionally, like `kernel.log_dropped`: fail2ban and the audit
    /// trail must not be blinded by a log filter that hides `warn` from
    /// `kernel`, or a deployment that runs quieter than `warn` would hide
    /// brute-force attempts entirely. `user` is the canonical name when the
    /// credential named a valid one, `candidate` (already clipped to
    /// [`UserName::MAX_LENGTH`]) when the given text was not a user name.
    fn report_rejection(
        &self,
        client: IpAddr,
        user: Option<&UserName>,
        reason: RejectReason,
        candidate: Option<&str>,
    ) {
        let shown = match (user, candidate) {
            (Some(name), _) => Some(name.as_str()),
            (None, Some(candidate)) => Some(candidate),
            (None, None) => None,
        };
        self.kernel.logger().write_always(
            self.request,
            Level::Warn,
            &AUTH_FAILED,
            &[
                ("client", &client.to_string() as &dyn LogValue),
                ("user", &shown.unwrap_or_default() as &dyn LogValue),
                ("reason", &reason.as_str() as &dyn LogValue),
            ],
        );
        self.publish(&AuthFailed {
            client,
            user: user.cloned(),
            reason,
        });
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

/// Canonicalize the credential's user name, the one spelling throttle,
/// provider and log share. Anything outside `[a-z0-9._-]` after trimming and
/// lowercasing is refused as `Invalid` before any module sees the attempt,
/// with the trimmed-and-lowercased candidate returned (clipped to
/// [`UserName::MAX_LENGTH`] bytes at a char boundary) for the audit line.
/// On success the credential carries the canonical text and the second field
/// is that name, so the throttle and the log never re-parse it.
fn canonicalize(
    mut credential: Credential,
) -> Result<(Credential, Option<UserName>), (String, RejectReason)> {
    let raw = credential.user().map(str::to_owned);
    if let Some(raw) = raw {
        match crate::identity::UserName::normalize(&raw) {
            Ok(name) => {
                credential.set_user(name.as_str());
                Ok((credential, Some(name)))
            }
            Err(candidate) => Err((clipped(&candidate), RejectReason::Invalid)),
        }
    } else {
        Ok((credential, None))
    }
}

/// The longest text an audit line may carry for a rejected name candidate.
fn clipped(candidate: &str) -> String {
    let bound = crate::identity::clip_boundary(candidate, UserName::MAX_LENGTH);
    candidate.get(..bound).unwrap_or(candidate).to_owned()
}

/// Throttle first, then the kind's provider; no logging or signals here.
async fn decide(
    ctx: Ctx,
    credential: Credential,
    client: IpAddr,
    user: Option<&UserName>,
) -> AuthOutcome {
    if ctx
        .kernel
        .throttle()
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
        Verdict::Recognized(identity) => AuthOutcome::Recognized(identity),
        Verdict::Rejected(reason) => AuthOutcome::Rejected(reason),
    }
}
