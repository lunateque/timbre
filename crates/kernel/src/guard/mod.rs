//! Route guards: what the kernel checks before a handler may run.
//!
//! Deny by default: every route declares a [`Guard`] when it registers, and
//! the kernel runs extract -> authenticate -> permission-check before the
//! handler. A guarded handler never runs on a failed check; the protocol's
//! own `deny` renders the answer instead. The kernel routes credentials by
//! kind only and never interprets them, so extraction and the wire shape of
//! a refusal belong to the protocol module that registers the route.

use crate::credential::{AuthOutcome, Credential, CredentialKind, RejectReason};
use crate::ctx::Ctx;
use crate::failure::Fallible;
use crate::future::BoxFuture;
use crate::http::{Request, Response};
use crate::identity::{Permission, Principal, UserName};
use crate::registry::Kernel;
use crate::route::{Matched, RouteHandler};

/// What a protocol's extractor found in one request.
#[derive(Debug)]
pub enum Extraction {
    /// The request carries no credential.
    None,
    /// Exactly one credential, as the protocol read it.
    One(Credential),
    /// More than one credential was offered at once; which one the client
    /// meant is ambiguous, so none of them is checked.
    Conflict,
}

/// Why a guarded request was refused, for the protocol's `deny`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Denial {
    /// The request carried no credential.
    Missing,
    /// The request carried conflicting credentials.
    Conflict,
    /// No module owns the credential's kind; the server cannot answer it at
    /// all. The kind names what the client offered.
    Unsupported(CredentialKind),
    /// The owning provider refused the credential. Whether the secret was
    /// wrong or the account disabled is deliberately not said here: a denial
    /// must not leak account state to an outsider, so protocols answer every
    /// `Rejected` exactly the same way.
    Rejected,
    /// The throttle refused to ask the provider at all; the client is sending
    /// attempts faster than the server is willing to answer.
    Throttled,
    /// The recognized principal does not hold `permission`.
    Forbidden(Permission),
}

/// A request whose route's guard the kernel has passed. It cannot be built
/// outside the kernel's guard flow, and [`Authorized::run`] is the only way
/// to invoke the handler: nothing outside the kernel can obtain a route's
/// handler as a value, so no call site can reach a handler around its guard.
pub struct Authorized {
    ctx: Ctx,
    handler: RouteHandler,
}

impl Authorized {
    /// The canonical user name the guard authenticated, `None` on a route
    /// the guard left without a principal (every public route). The HTTP
    /// edge puts it on the request log line.
    #[must_use]
    pub fn user(&self) -> Option<&UserName> {
        self.ctx.principal().map(Principal::user)
    }

    /// Invoke the handler the guard passed, on the context carrying the
    /// principal it established.
    pub fn run(self, request: Request) -> BoxFuture<Fallible<Response>> {
        (self.handler)(self.ctx, request)
    }
}

/// What a route declares about who may call it. The two functions are plain
/// pointers written by the protocol module that registers the route: the
/// kernel never reads a wire format, so it cannot pick a credential out of a
/// request or answer a refusal in the protocol's own shape.
#[derive(Clone, Copy, Debug)]
pub enum Guard {
    /// Anyone may call this route; the handler's context carries no
    /// principal, and nothing is authenticated.
    Public,
    /// Before the handler runs, the kernel calls `extract` on the request,
    /// authenticates the credential it found, and requires the recognized
    /// identity to hold `permission`. On any failure the kernel answers with
    /// `deny(&request, denial)` and the handler never runs.
    Requires {
        /// The permission the caller's identity must hold.
        permission: Permission,
        /// Reads the credential (or its absence, or a conflict) out of the
        /// request.
        extract: fn(&Request) -> Extraction,
        /// Renders the protocol's own answer to a denial.
        deny: fn(&Request, Denial) -> Response,
    },
}

impl Kernel {
    /// Run the guard of `matched` for `request`, attributed to
    /// `request.client`, and return what only this call can produce: an
    /// [`Authorized`] pairing the context the handler must run on (the same
    /// context plus, for a passed [`Guard::Requires`], the authenticated
    /// principal) with the matched route's handler. A [`Guard::Public`]
    /// route goes through the same path and yields an `Authorized` without a
    /// principal, so no handler can be invoked except through a passed guard.
    ///
    /// # Errors
    ///
    /// The response rendered by the guard's `deny` for the failure; on
    /// [`Guard::Public`], never.
    pub async fn guarded(
        &self,
        ctx: Ctx,
        matched: &Matched,
        request: &Request,
    ) -> Result<Authorized, Response> {
        let handler = matched.handler();
        let Guard::Requires {
            permission,
            extract,
            deny,
        } = matched.guard()
        else {
            return Ok(Authorized { ctx, handler });
        };
        let credential = match extract(request) {
            Extraction::None => return Err(deny(request, Denial::Missing)),
            Extraction::Conflict => return Err(deny(request, Denial::Conflict)),
            Extraction::One(credential) => credential,
        };
        // The kind is read before `credential` moves into `authenticate`, so
        // an unsupported answer can name what the client offered.
        let kind = credential.kind();
        match ctx.authenticate(credential, request.client).await {
            AuthOutcome::Recognized(identity) => {
                if identity.permissions.contains(&permission) {
                    Ok(Authorized {
                        ctx: ctx.with_principal(Principal::from_identity(&identity)),
                        handler,
                    })
                } else {
                    // A recognized identity that lacks the permission is an
                    // audit event, not just a wire answer: the protocol's
                    // `deny` says what the client hears, the line and the
                    // signal say what happened.
                    ctx.report_forbidden(request.client, &identity.user, permission);
                    Err(deny(request, Denial::Forbidden(permission)))
                }
            }
            // The wire never learns whether the secret was wrong or the
            // account disabled: `Rejected` collapses both so a probe cannot
            // tell an existing user from a wrong password. `Throttled` stays
            // distinct because the protocol must ask the client to slow down.
            AuthOutcome::Rejected(RejectReason::Throttled) => Err(deny(request, Denial::Throttled)),
            AuthOutcome::Rejected(RejectReason::Invalid | RejectReason::Disabled) => {
                Err(deny(request, Denial::Rejected))
            }
            AuthOutcome::Unsupported => Err(deny(request, Denial::Unsupported(kind))),
        }
    }
}

#[cfg(test)]
mod tests;
