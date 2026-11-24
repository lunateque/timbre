//! Authentication as `Ctx` offers it: canonicalize the name, reserve in the
//! throttle, ask the provider that owns the kind, settle exactly once.

use super::Ctx;
use crate::credential::{AuthOutcome, Credential, RejectDetail, RejectReason, Throttle, Verdict};
use crate::future::BoxFuture;
use crate::identity::UserName;
use crate::log::LogValue;
use crate::log_events::{AUTH_FAILED, AUTH_UNSUPPORTED};
use crate::signals::AuthFailed;
use std::net::IpAddr;

impl Ctx {
    /// Check `credential` against the module that owns its kind, or report
    /// `Unsupported` when the kind has none. `client` is the address the
    /// attempt is attributed to, required so a CLI path cannot silently
    /// blame an unspecified host. A registered `Throttle` is consulted first
    /// and can refuse without the provider being asked. A name the credential
    /// carries is canonicalized before either sees it; one that cannot
    /// canonicalize is rejected without asking a provider, as is an empty
    /// scheme user whose owner did not opt in, but both still pass through
    /// the address throttle. A check dropped before its verdict arrives
    /// counts as a failure, so a closed connection buys no free guess.
    /// Rejections publish [`AuthFailed`] and log `kernel.auth_failed` without
    /// the secret, except throttle refusals, which are counted instead.
    pub fn authenticate(&self, credential: Credential, client: IpAddr) -> BoxFuture<AuthOutcome> {
        // Everything below this point runs attributed to the client, so the
        // provider can record where a secret was presented from.
        let client = client.to_canonical();
        let ctx = self.with_client(client);
        let kind = credential.kind();
        let userless = matches!(&credential, Credential::Scheme { user, .. } if user.is_empty())
            && !self.kernel.accepts_userless(kind);
        Box::pin(async move {
            // An empty user names nobody, so there is nothing to canonicalize
            // and no provider to ask; the refusal still goes through the
            // address window, or repeating it would cost an attacker nothing.
            let normalized = if userless {
                Err((String::new(), RejectReason::Invalid))
            } else {
                canonicalize(credential)
            };
            let user = normalized.as_ref().ok().and_then(|(_, user)| user.as_ref());
            let throttle = if let Some(limiter) = ctx.kernel.throttle() {
                let Some(slots) = limiter.reserve(&ctx, client, user) else {
                    ctx.kernel.throttled.record(client);
                    return AuthOutcome::Rejected(RejectReason::Throttled);
                };
                Some((limiter.as_ref(), slots))
            } else {
                None
            };
            match normalized {
                Err((candidate, reason)) => {
                    Reservation {
                        throttle,
                        ctx: &ctx,
                        client,
                        user: None,
                        reason: Some(reason),
                    };
                    let candidate = (!candidate.is_empty()).then_some(candidate);
                    ctx.report_rejection(client, None, reason, candidate.as_deref(), None);
                    AuthOutcome::Rejected(reason)
                }
                Ok((credential, user)) => {
                    // The reservation starts out counting as a failure: a
                    // future dropped mid-check settles through `Drop`, where
                    // no verdict will ever arrive to say otherwise.
                    let mut reservation = Reservation {
                        throttle,
                        ctx: &ctx,
                        client,
                        user: user.as_ref(),
                        reason: Some(RejectReason::Invalid),
                    };
                    let Some(provider) = ctx.kernel.credential_provider(kind) else {
                        reservation.reason = None;
                        drop(reservation);
                        crate::log!(ctx, info, AUTH_UNSUPPORTED, kind = kind.as_str().as_ref());
                        return AuthOutcome::Unsupported;
                    };
                    match provider.check(ctx.clone(), credential).await {
                        Verdict::Recognized(identity) => {
                            reservation.reason = None;
                            drop(reservation);
                            AuthOutcome::Recognized(identity)
                        }
                        Verdict::Rejected(reason, detail) => {
                            reservation.reason = Some(reason);
                            drop(reservation);
                            ctx.report_rejection(client, user.as_ref(), reason, None, detail);
                            AuthOutcome::Rejected(reason)
                        }
                    }
                }
            }
        })
    }

    fn report_rejection(
        &self,
        client: IpAddr,
        user: Option<&UserName>,
        reason: RejectReason,
        candidate: Option<&str>,
        detail: Option<RejectDetail>,
    ) {
        if reason == RejectReason::Throttled {
            self.kernel.throttled.record(client);
            return;
        }
        let shown = match (user, candidate) {
            (Some(name), _) => Some(name.as_str()),
            (None, Some(candidate)) => Some(candidate),
            (None, None) => None,
        };
        let client_text = client.to_string();
        let user_text = shown.unwrap_or_default();
        let reason_text = reason.as_str();
        let detail_text = detail.map_or("", RejectDetail::as_str);
        let fields: [(&str, &dyn LogValue); 4] = [
            ("client", &client_text),
            ("user", &user_text),
            ("reason", &reason_text),
            ("detail", &detail_text),
        ];
        let count = if detail.is_some() {
            fields.len()
        } else {
            fields.len() - 1
        };
        self.log_security(&AUTH_FAILED, fields.get(..count).unwrap_or(&fields));
        self.publish(&AuthFailed {
            client,
            user: user.cloned(),
            reason,
            detail,
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

struct Reservation<'a> {
    throttle: Option<(&'a dyn Throttle, [bool; 2])>,
    ctx: &'a Ctx,
    client: IpAddr,
    user: Option<&'a UserName>,
    reason: Option<RejectReason>,
}

impl Drop for Reservation<'_> {
    fn drop(&mut self) {
        if let Some((throttle, slots)) = self.throttle {
            throttle.finish(self.ctx, self.client, self.user, slots, self.reason);
        }
    }
}
