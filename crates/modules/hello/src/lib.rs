//! The example module: it exists to exercise the kernel contracts (a log
//! event, a failure, a signal handler, a route, a CLI command, a declared
//! permission) and to prove that a module can be deleted with its directory.
//! It goes away when the first real module lands.

pub mod failures;
pub mod log_events;

use log_events::GREETED;
use timbre_kernel::http::{Body, Method, Request, Response};
use timbre_kernel::signals::Started;
use timbre_kernel::{
    BoxFuture, Credential, Ctx, Denial, Extraction, Fallible, Guard, Permission, Registry, Secret,
    log,
};

/// The permission `hello` declares for granting; guarded routes name it.
pub const GREET: Permission = Permission::new("hello.greet");

/// Register the module: one handler for `Started`, two routes, a command and
/// a declared permission.
///
/// # Errors
///
/// Propagates the kernel's refusal of a duplicate registration.
pub fn register(registry: &mut Registry) -> Fallible {
    registry.on::<Started>(greet);
    registry.route(Method::Get, "/hello", Guard::Public, hello_route)?;
    registry.route(
        Method::Get,
        "/hello/me",
        Guard::Requires {
            permission: GREET,
            extract: extract_bearer,
            deny: deny_plain,
        },
        me_route,
    )?;
    registry.command(
        "hello",
        "print the greeting through the logger",
        run_command,
    )?;
    registry.permission(GREET, "let a user greet through the hello module")?;
    Ok(())
}

// Both signatures are fixed by the contract types: the bus hands a reference
// and every handler answers with `Fallible`.
#[expect(
    clippy::trivially_copy_pass_by_ref,
    clippy::unnecessary_wraps,
    reason = "the signature is Handler<Started>"
)]
fn greet(ctx: &Ctx, _: &Started) -> Fallible {
    log!(ctx, info, GREETED, greeting = "hello");
    Ok(())
}

/// `GET /hello`: always 200 `text/plain` "hello".
fn hello_route(_: Ctx, _: Request) -> BoxFuture<Fallible<Response>> {
    Box::pin(async {
        Ok(Response::new(200)
            .with_header("content-type", b"text/plain; charset=utf-8")
            .with_body(Body::text("hello")))
    })
}

/// The credential the request's `Authorization` lines carry. Exactly one line
/// is read, `Bearer <secret>` with the scheme word case-insensitive and the
/// secret trimmed of surrounding spaces. Any other scheme carries no bearer
/// credential, so the guard answers `Missing` rather than guessing. Two lines,
/// or one line whose secret holds a comma, offer several values at once: that
/// is a conflict, since no bearer token contains a comma.
fn extract_bearer(request: &Request) -> Extraction {
    let lines: Vec<&[u8]> = request
        .headers
        .iter()
        .filter(|(name, _)| name.eq_ignore_ascii_case("authorization"))
        .map(|(_, value)| value.as_slice())
        .collect();
    match lines.as_slice() {
        [] => Extraction::None,
        [line] => bearer_credential(line),
        _ => Extraction::Conflict,
    }
}

/// The bearer credential of one `Authorization` value; see [`extract_bearer`].
fn bearer_credential(value: &[u8]) -> Extraction {
    // A secret that is not UTF-8 is no token the server could have issued, so
    // it is refused, not rewritten lossily into a string that might match one.
    let Ok(text) = std::str::from_utf8(value) else {
        return Extraction::None;
    };
    let Some((scheme, secret)) = text.split_once(' ') else {
        return Extraction::None;
    };
    if !scheme.eq_ignore_ascii_case("bearer") {
        return Extraction::None;
    }
    let secret = secret.trim();
    if secret.is_empty() {
        return Extraction::None;
    }
    if secret.contains(',') {
        return Extraction::Conflict;
    }
    Extraction::One(Credential::Bearer(Secret::new(secret.to_owned())))
}

/// The plain-text answer to a denial: 401 when the caller offered nothing the
/// server accepts (missing, rejected, or a scheme no module owns), 429 when the
/// throttle refused to ask, 403 for a recognized caller without `hello.greet`,
/// and 400 when the request offers more than one credential.
fn deny_plain(_: &Request, denial: Denial) -> Response {
    let (status, text) = match denial {
        Denial::Missing => (401, "this greeting needs a bearer token\n".to_owned()),
        Denial::Rejected => (401, "the greeting did not accept that token\n".to_owned()),
        Denial::Throttled => (429, "the greeting is asked for too fast\n".to_owned()),
        Denial::Unsupported(_) => (401, "this server accepts no bearer tokens\n".to_owned()),
        Denial::Forbidden(permission) => (
            403,
            format!("the caller does not hold {}\n", permission.as_str()),
        ),
        Denial::Conflict => (
            400,
            "the request offers more than one credential\n".to_owned(),
        ),
    };
    Response::new(status)
        .with_header("content-type", b"text/plain; charset=utf-8")
        .with_body(Body::text(&text))
}

/// `GET /hello/me`: guarded by `hello.greet`, so the guard has established a
/// principal, and that user is greeted by name.
fn me_route(ctx: Ctx, _: Request) -> BoxFuture<Fallible<Response>> {
    Box::pin(async move { greet_principal(&ctx) })
}

/// Greet the principal the guard established. A route whose guard passed
/// always has one; if it does not, the guard flow is broken and the request
/// fails rather than greeting a name hello made up.
fn greet_principal(ctx: &Ctx) -> Fallible<Response> {
    let Some(principal) = ctx.principal() else {
        return Err(Box::new(failures::HelloFailure::PrincipalMissing));
    };
    Ok(Response::new(200)
        .with_header("content-type", b"text/plain; charset=utf-8")
        .with_body(Body::text(&format!("hello, {}\n", principal.user()))))
}

/// `timbre hello`: greets through the logger, like the `Started` handler.
fn run_command(ctx: Ctx, _: Vec<String>) -> BoxFuture<Fallible> {
    Box::pin(async move {
        log!(ctx, info, GREETED, greeting = "hello");
        Ok(())
    })
}

#[cfg(test)]
mod tests;
