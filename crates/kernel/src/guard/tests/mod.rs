use crate::block_on;
use crate::credential::{
    Credential, CredentialKind, CredentialProvider, RejectReason, SchemeName, Throttle, Verdict,
};
use crate::guard::{Denial, Extraction, Guard};
use crate::http::{Body, Method, Request, Response};
use crate::identity::{Identity, Permission, UserName};
use crate::port::Port;
use crate::route::RouteMatch;
use crate::testing::Shared;
use crate::{Ctx, Failure, Fallible, Format, Kernel, LogFilter, Logger, Registry, ThreadRuntime};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

mod flow;
mod owners;

const GREET: Permission = Permission::new("hello.greet");
const WRITE: Permission = Permission::new("hello.write");
const LEGACY: SchemeName = SchemeName::new("other.sig");

fn user(text: &str) -> UserName {
    UserName::normalize(text).unwrap_or_else(|rejected| panic!("`{rejected}` is not canonical"))
}

fn granted(permission: Permission) -> Identity {
    Identity {
        user: user("tester"),
        permissions: vec![permission],
    }
}

/// What the fake protocol says a request carries: a `k` query pair is an api
/// key, `k` together with `p` or `t` a conflict, a `t` pair the named scheme,
/// a `p` pair a password, nothing none.
fn extract(request: &Request) -> Extraction {
    let key = request.query_first("k");
    let pass = request.query_first("p");
    let sig = request.query_first("t");
    match (key, pass, sig) {
        (Some(_), Some(_), _) | (Some(_), _, Some(_)) => Extraction::Conflict,
        (Some(secret), None, None) => {
            Extraction::One(Credential::ApiKey(crate::Secret::new(secret.to_owned())))
        }
        (None, _, Some(sig)) => Extraction::One(Credential::Scheme {
            name: LEGACY,
            user: request.query_first("u").unwrap_or_default().to_owned(),
            fields: vec![("t".to_owned(), crate::Secret::new(sig.to_owned()))],
        }),
        (None, Some(secret), None) => Extraction::One(Credential::Password {
            user: request.query_first("u").unwrap_or_default().to_owned(),
            secret: crate::Secret::new(secret.to_owned()),
        }),
        (None, None, None) => Extraction::None,
    }
}

/// The fake protocol's answer: one status per denial shape, its text in the
/// body so a test can tell the shapes apart.
fn deny(_: &Request, denial: Denial) -> Response {
    let (status, label) = match denial {
        Denial::Missing => (401, "missing".to_owned()),
        Denial::Conflict => (400, "conflict".to_owned()),
        Denial::Unsupported(kind) => (501, format!("unsupported:{kind:?}")),
        Denial::Rejected => (401, "rejected".to_owned()),
        Denial::Throttled => (429, "throttled".to_owned()),
        Denial::Forbidden(permission) => (403, format!("forbidden:{}", permission.as_str())),
    };
    Response::new(status).with_body(Body::Bytes(label.into_bytes()))
}

/// The provider under test: answers `verdict` for its kind, counting asks.
pub(super) struct Fake {
    verdict: Verdict,
    asked: Arc<AtomicUsize>,
}

/// A fake that always rejects; the owner-clash tests only need its identity.
pub(super) fn rejecting() -> Arc<dyn CredentialProvider> {
    Arc::new(Fake {
        verdict: Verdict::Rejected(RejectReason::Invalid, None),
        asked: Arc::new(AtomicUsize::new(0)),
    })
}

impl CredentialProvider for Fake {
    fn check(&self, _ctx: Ctx, _credential: Credential) -> crate::BoxFuture<Verdict> {
        self.asked.fetch_add(1, Ordering::SeqCst);
        let verdict = clone_verdict(&self.verdict);
        Box::pin(async move { verdict })
    }
}

/// Rebuild a test verdict: identities and reasons are cheap to clone here.
fn clone_verdict(verdict: &Verdict) -> Verdict {
    match verdict {
        Verdict::Recognized(found) => Verdict::Recognized(Identity {
            user: found.user.clone(),
            permissions: found.permissions.clone(),
        }),
        Verdict::Rejected(reason, detail) => Verdict::Rejected(*reason, *detail),
    }
}

pub(super) struct DenyAll;

impl Throttle for DenyAll {
    fn reserve(&self, _: &Ctx, _: IpAddr, _: Option<&UserName>) -> Option<[bool; 2]> {
        None
    }

    fn finish(
        &self,
        _: &Ctx,
        _: IpAddr,
        _: Option<&UserName>,
        _: [bool; 2],
        _: Option<RejectReason>,
    ) {
        panic!("a refused check never reserves")
    }
}

/// The guarded routes the harness registers: `/me` requires `GREET`,
/// `/write` requires `WRITE`, `/public` requires nothing.
fn route_guard(permission: Permission) -> Guard {
    Guard::Requires {
        permission,
        extract,
        deny,
    }
}

/// The handler every harness route runs: it reports the principal the guard
/// handed it, so a test sees through the response body exactly what a real
/// handler would, and nothing before a passed guard reaches it.
fn echo(ctx: Ctx, _: Request) -> crate::BoxFuture<crate::Fallible<Response>> {
    Box::pin(async move {
        let body = ctx.principal().map_or_else(
            || "ran:nobody".to_owned(),
            |who| format!("ran:{}:greet={}", who.user(), who.has(GREET)),
        );
        Ok(Response::new(200).with_body(Body::text(&body)))
    })
}

/// The audit trail of forbidden requests, kept behind a port so the
/// `AccessDenied` handler running on the bus can reach it.
pub(super) trait Denials: Port {
    fn note(&self, entry: String);
    fn entries(&self) -> Vec<String>;
}

#[derive(Default)]
pub(super) struct DenialLog(std::sync::Mutex<Vec<String>>);

impl Port for DenialLog {}

impl Denials for DenialLog {
    fn note(&self, entry: String) {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(entry);
    }

    fn entries(&self) -> Vec<String> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

// The signature is fixed by `Handler<AccessDenied>`.
#[expect(
    clippy::unnecessary_wraps,
    reason = "the signature is Handler<AccessDenied>"
)]
fn remember_denied(ctx: &Ctx, signal: &crate::signals::AccessDenied) -> crate::Fallible {
    if let Some(journal) = ctx.ports().get::<dyn Denials>() {
        journal.note(format!(
            "{}|{}|{}",
            signal.client,
            signal.user,
            signal.permission.as_str()
        ));
    }
    Ok(())
}

/// A kernel registering each `(module, kind, verdict)` as a named module's
/// credential owner, on a JSON sink; `throttle` names the module that gets
/// the deny-everything limiter. The three routes above are registered too, so
/// every guard test runs the same `find_route` + `guarded` path the edges do:
/// the returned kernel also carries the denial journal.
fn harness(
    providers: &[(&'static str, CredentialKind, Verdict)],
    throttle: Option<&'static str>,
    filter: LogFilter,
) -> (Arc<Kernel>, Shared, Arc<AtomicUsize>, Arc<DenialLog>) {
    let sink = Shared::default();
    let asked = Arc::new(AtomicUsize::new(0));
    let denials = Arc::new(DenialLog::default());
    let mut registry = Registry::new();
    registry
        .provide::<dyn Denials>(Arc::clone(&denials) as Arc<dyn Denials>)
        .expect("the journal");
    for (module, kind, verdict) in providers {
        let asked = Arc::clone(&asked);
        let verdict = clone_verdict(verdict);
        let kind = *kind;
        registry = registry
            .module(module, move |r| {
                r.credential_provider(
                    kind,
                    Arc::new(Fake {
                        verdict,
                        asked: Arc::clone(&asked),
                    }),
                )
            })
            .expect("a first owner registers");
    }
    if let Some(module) = throttle {
        registry = registry
            .module(module, |r| {
                r.throttle(Arc::new(DenyAll) as Arc<dyn Throttle>)
            })
            .expect("a first throttle registers");
    }
    for (path, guard) in [
        ("/me", route_guard(GREET)),
        ("/write", route_guard(WRITE)),
        ("/public", Guard::Public),
    ] {
        registry
            .route(Method::Get, path, guard, echo)
            .expect("a harness route");
    }
    registry.on::<crate::signals::AccessDenied>(remember_denied);
    let kernel = registry.finish(
        Logger::new(
            filter,
            Format::Json,
            Box::new(sink.clone()),
            crate::testing::fixed,
        ),
        Arc::new(ThreadRuntime::new()),
    );
    (kernel, sink, asked, denials)
}

fn request(query: &[(&str, &str)]) -> Request {
    Request {
        method: Method::Get,
        path: "/me".to_owned(),
        param: None,
        query: query
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect(),
        headers: Vec::new(),
        peer: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 4533),
        client: IpAddr::V4(Ipv4Addr::LOCALHOST),
        body: Body::Empty,
    }
}

fn body_text(response: &Response) -> String {
    String::from_utf8(
        response
            .body
            .as_bytes()
            .expect("an in-memory deny body")
            .to_vec(),
    )
    .expect("utf-8 marker")
}

// Neither `Ctx` nor `Response` is `Debug`, so the guard outcomes come back
// through these helpers instead of `expect`/`expect_err`.
fn matched(kernel: &Kernel, path: &str) -> crate::route::Matched {
    match kernel.find_route(Method::Get, path) {
        RouteMatch::Found(found) => found,
        other => panic!("the harness registers {path}, got {other:?}"),
    }
}

fn authorized(kernel: &Arc<Kernel>, path: &str, req: &Request) -> crate::Authorized {
    match block_on(kernel.guarded(Ctx::root(kernel), &matched(kernel, path), req)) {
        Ok(found) => found,
        Err(response) => panic!("the guard must accept, denied with {}", response.status),
    }
}

fn denied(kernel: &Arc<Kernel>, path: &str, req: &Request) -> Response {
    match block_on(kernel.guarded(Ctx::root(kernel), &matched(kernel, path), req)) {
        Ok(_) => panic!("the guard must deny"),
        Err(response) => response,
    }
}

/// Run a passed guard the only way the handler can run, and return the
/// body the handler echoed: the test sees the principal through the
/// handler's eyes, the way a real edge does.
fn ran(authorized: crate::Authorized, req: &Request) -> String {
    let owned = Request {
        method: req.method,
        path: req.path.clone(),
        param: req.param.clone(),
        query: req.query.clone(),
        headers: req.headers.clone(),
        peer: req.peer,
        client: req.client,
        body: Body::Empty,
    };
    let response = block_on(authorized.run(owned)).expect("the harness handler answers");
    body_text(&response)
}

pub(super) fn refused(outcome: Fallible<Registry>) -> Box<dyn Failure> {
    match outcome {
        Ok(_) => panic!("the registration must fail"),
        Err(failure) => failure,
    }
}
