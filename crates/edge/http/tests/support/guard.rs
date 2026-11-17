//! The guard fixtures: a fake credential scheme with its provider, the
//! extract/deny pair of the fake protocol, the guarded routes and the port
//! that counts how often a guarded handler actually ran.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use timbre_kernel::http::{Body, Request, Response};
use timbre_kernel::{
    BoxFuture, Credential, CredentialProvider, Ctx, Denial, Extraction, Fallible, Identity,
    Permission, Port, RejectReason, SchemeName, Secret, UserName, Verdict,
};

/// The fake protocol's credential scheme and the permission its guarded
/// route requires; the kernel only ever routes the scheme by this name.
pub(crate) const TEST_SCHEME: SchemeName = SchemeName::new("test.key");
pub(crate) const READ: Permission = Permission::new("test.read");

/// How many times the guarded test handler actually ran: the deny tests
/// prove the handler never saw a failed request.
pub(crate) trait Counter: Port {
    fn hit(&self);
    fn calls(&self) -> usize;
}

#[derive(Default)]
pub(crate) struct SharedCounter(AtomicUsize);

impl Port for SharedCounter {}

impl Counter for SharedCounter {
    fn hit(&self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }

    fn calls(&self) -> usize {
        self.0.load(Ordering::SeqCst)
    }
}

/// Reads the fake protocol's credential out of the request: `x-test-user`
/// names the account (raw wire text, the kernel canonicalizes it),
/// `x-test-key` carries the secret. No key, no credential.
pub(crate) fn extract(request: &Request) -> Extraction {
    let named = |name: &str| {
        request
            .headers
            .iter()
            .find(|(header, _)| header == name)
            .map(|(_, value)| String::from_utf8_lossy(value).into_owned())
    };
    let Some(secret) = named("x-test-key") else {
        return Extraction::None;
    };
    Extraction::One(Credential::Scheme {
        name: TEST_SCHEME,
        user: named("x-test-user").unwrap_or_else(|| "anonymous".to_owned()),
        fields: vec![("key".to_owned(), Secret::new(secret))],
    })
}

/// The same credential read from the merged form fields instead of the
/// headers: this is why a guarded form POST must have its body buffered
/// before the guard runs - the secret may travel in the body.
pub(crate) fn extract_form(request: &Request) -> Extraction {
    let secret = match request.query_first("key") {
        Some(secret) => secret.to_owned(),
        None => return Extraction::None,
    };
    Extraction::One(Credential::Scheme {
        name: TEST_SCHEME,
        user: request
            .query_first("user")
            .unwrap_or("anonymous")
            .to_owned(),
        fields: vec![("key".to_owned(), Secret::new(secret))],
    })
}

/// The fake protocol's own refusal shape: its status plus a body naming
/// the denial, and a wrong `content-length` the edge must strip like any
/// other framing header before the response reaches the wire.
pub(crate) fn deny(_: &Request, denial: Denial) -> Response {
    let (status, label) = match denial {
        Denial::Missing => (401, "missing".to_owned()),
        Denial::Conflict => (400, "conflict".to_owned()),
        Denial::Unsupported(kind) => (501, format!("unsupported:{kind:?}")),
        Denial::Rejected => (401, "rejected".to_owned()),
        Denial::Throttled => (429, "throttled".to_owned()),
        Denial::Forbidden(permission) => (403, format!("forbidden:{}", permission.as_str())),
    };
    Response::new(status)
        .with_header("content-length", b"999")
        .with_body(Body::text(&format!("deny:{label}")))
}

/// The guarded route's handler: counts its own runs (through the port)
/// and echoes the principal the guard handed it, so a path that skipped
/// the guard is provable from the body and the counter.
#[expect(
    clippy::expect_used,
    reason = "test fixture: the counter port is registered beside this route"
)]
pub(crate) fn guarded_echo(ctx: Ctx, _: Request) -> BoxFuture<Fallible<Response>> {
    Box::pin(async move {
        ctx.ports()
            .get::<dyn Counter>()
            .expect("the counter port")
            .hit();
        let who = ctx
            .principal()
            .map_or_else(|| "nobody".to_owned(), |p| p.user().to_string());
        Ok(Response::new(200).with_body(Body::text(&format!("ran:{who}"))))
    })
}

/// A guarded handler that reports the body size it received: proves the
/// edge still hands an authorized request its body, buffered after the
/// check passed.
pub(crate) fn guarded_body_len(ctx: Ctx, request: Request) -> BoxFuture<Fallible<Response>> {
    Box::pin(async move {
        let seen = request.body.as_bytes().map_or(0, <[u8]>::len);
        let who = ctx
            .principal()
            .map_or_else(|| "nobody".to_owned(), |p| p.user().to_string());
        Ok(Response::new(200).with_body(Body::text(&format!("ran:{who}:{seen}"))))
    })
}

/// The owner of the fake scheme: `good-secret` names `alice` with `READ`,
/// `hang-secret` never answers at all, so the guard deadline bites.
pub(crate) struct FakeProvider;

#[expect(
    clippy::expect_used,
    reason = "test fixture: authenticate canonicalized the user before this check"
)]
impl CredentialProvider for FakeProvider {
    fn check(&self, _: Ctx, credential: Credential) -> BoxFuture<Verdict> {
        let offer = match &credential {
            Credential::Scheme { name, fields, .. } if *name == TEST_SCHEME => fields
                .first()
                .map_or("", |(_, secret)| secret.expose().as_str()),
            _ => "",
        };
        // `hang-secret` never gets an answer: a guard deadline can be
        // observed against a provider that ignores its request entirely.
        if offer == "hang-secret" {
            return Box::pin(std::future::pending::<Verdict>());
        }
        let verdict = if offer == "good-secret" {
            Verdict::Recognized(Identity {
                user: UserName::normalize(credential.user().unwrap_or_default())
                    .expect("the kernel canonicalized the user already"),
                permissions: vec![READ],
            })
        } else {
            Verdict::Rejected(RejectReason::Invalid)
        };
        Box::pin(async move { verdict })
    }
}

/// The provider registration, kept beside the scheme it owns.
pub(crate) fn provider() -> Arc<dyn CredentialProvider> {
    Arc::new(FakeProvider)
}

/// The guard of `/guarded`: the permission the fake identity holds.
pub(crate) fn read_guard() -> timbre_kernel::Guard {
    requires(READ)
}

/// The guard of `/admin`: a permission the fake identity does not hold.
pub(crate) fn admin_guard() -> timbre_kernel::Guard {
    requires(Permission::new("test.admin"))
}

/// The guard of `/guarded-form`: the same scheme, but the credential is
/// read from the merged form fields, so the edge must buffer the body
/// before this check can run.
pub(crate) fn form_guard() -> timbre_kernel::Guard {
    timbre_kernel::Guard::Requires {
        permission: READ,
        extract: extract_form,
        deny,
    }
}

fn requires(permission: Permission) -> timbre_kernel::Guard {
    timbre_kernel::Guard::Requires {
        permission,
        extract,
        deny,
    }
}
