//! The hello module's tests: its routes, the guard's extraction of a bearer
//! credential and the log lines the module writes.
use super::*;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use timbre_kernel::testing::{Shared, epoch};
use timbre_kernel::{Format, Kernel, LogFilter, Logger, RouteMatch, ThreadRuntime, block_on};

fn finished(sink: Shared) -> Arc<Kernel> {
    let registry = Registry::new()
        .module("hello", register)
        .expect("hello registers");
    registry.finish(
        Logger::new(LogFilter::default(), Format::Json, Box::new(sink), epoch),
        Arc::new(ThreadRuntime::new()),
    )
}

#[test]
fn started_publishes_one_greeting_line() {
    let sink = Shared::default();
    let kernel = finished(sink.clone());
    Ctx::root(&kernel).publish(&Started);
    // Dropping the last kernel joins the log writer, so every line has reached the sink.
    drop(kernel);
    assert_eq!(
        sink.text(),
        "{\"ts\":\"1970-01-01T00:00:00.000Z\",\"level\":\"info\",\"event\":\"hello.greeted\",\"greeting\":\"hello\"}\n"
    );
}

fn request(method: Method, path: &str) -> Request {
    Request {
        method,
        path: path.to_owned(),
        param: None,
        query: Vec::new(),
        headers: Vec::new(),
        peer: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 4533),
        client: IpAddr::V4(Ipv4Addr::LOCALHOST),
        body: Body::Empty,
    }
}

/// The guard `GET /hello/me` registered with.
fn me_match(kernel: &Kernel) -> timbre_kernel::Matched {
    match kernel.find_route(Method::Get, "/hello/me") {
        RouteMatch::Found(found) => found,
        other => panic!("expected a found route, got {other:?}"),
    }
}

/// `GET /hello/me` with `headers` runs through the kernel's guard check,
/// exactly as the HTTP edge does before the handler.
fn guarded_me(kernel: &Arc<Kernel>, headers: Vec<(String, Vec<u8>)>) -> Response {
    let mut req = request(Method::Get, "/hello/me");
    req.headers = headers;
    match block_on(kernel.guarded(Ctx::root(kernel), &me_match(kernel), &req)) {
        // `Authorized` is not `Debug`, so the outcome comes back through a match.
        Ok(_) => panic!("the guard must deny"),
        Err(response) => response,
    }
}

fn text(response: &Response) -> String {
    String::from_utf8(
        response
            .body
            .as_bytes()
            .expect("a plain-text deny body")
            .to_vec(),
    )
    .expect("utf-8 marker")
}

#[test]
fn hello_me_denies_a_request_without_a_bearer_header() {
    let kernel = finished(Shared::default());
    let denied = guarded_me(&kernel, Vec::new());
    assert_eq!(denied.status, 401);
    assert_eq!(
        denied.header("content-type"),
        Some(b"text/plain; charset=utf-8".as_slice())
    );
    assert_eq!(text(&denied), "this greeting needs a bearer token\n");
}

#[test]
fn hello_me_denies_a_bearer_header_without_an_owner() {
    // No module in this tree owns `CredentialKind::Bearer`, so even a
    // well-formed token lands on the unsupported denial, which hello also
    // answers 401.
    let kernel = finished(Shared::default());
    let with_token = vec![("authorization".to_owned(), b"Bearer hunter2".to_vec())];
    let denied = guarded_me(&kernel, with_token);
    assert_eq!(denied.status, 401);
    assert_eq!(text(&denied), "this server accepts no bearer tokens\n");
}

#[test]
fn the_route_answers_200_hello() {
    let kernel = finished(Shared::default());
    // The handler runs the only way an edge can run it: through a
    // passed guard of the route match.
    let req = request(Method::Get, "/hello");
    let matched = match kernel.find_route(Method::Get, "/hello") {
        RouteMatch::Found(found) => {
            assert_eq!(found.param(), None);
            found
        }
        other => panic!("expected a found route, got {other:?}"),
    };
    let authorized = match block_on(kernel.guarded(Ctx::root(&kernel), &matched, &req)) {
        Ok(authorized) => authorized,
        Err(response) => panic!(
            "a public route is always authorized, got {}",
            response.status
        ),
    };
    let response = block_on(authorized.run(req)).expect("the route answers");
    assert_eq!(response.status, 200);
    assert_eq!(
        response.header("content-type"),
        Some(b"text/plain; charset=utf-8".as_slice())
    );
    assert_eq!(response.body.as_bytes(), Some(&b"hello"[..]));
    // The route does not answer other paths or methods.
    assert!(matches!(
        kernel.find_route(Method::Post, "/hello"),
        RouteMatch::MethodNotAllowed
    ));
    assert!(matches!(
        kernel.find_route(Method::Get, "/goodbye"),
        RouteMatch::NotFound
    ));
}

#[test]
fn the_command_greets_through_the_logger() {
    let sink = Shared::default();
    let kernel = finished(sink.clone());
    let ctx = Ctx::root(&kernel);
    let (handler, rest) = kernel
        .find_command(&["hello".to_owned()])
        .expect("the hello command is registered");
    assert!(rest.is_empty(), "no words follow the command name");
    block_on(handler(ctx, rest)).expect("the command succeeds");
    drop(kernel);
    assert_eq!(
        sink.text(),
        "{\"ts\":\"1970-01-01T00:00:00.000Z\",\"level\":\"info\",\"event\":\"hello.greeted\",\"greeting\":\"hello\"}\n"
    );
}

#[test]
fn the_permission_is_declared_for_granting() {
    let kernel = finished(Shared::default());
    let declared: Vec<Permission> = kernel
        .permissions()
        .iter()
        .map(|entry| entry.permission)
        .collect();
    assert_eq!(declared, [GREET]);
}

fn extracted(headers: Vec<(String, Vec<u8>)>) -> Extraction {
    let mut req = request(Method::Get, "/hello/me");
    req.headers = headers;
    extract_bearer(&req)
}

fn authorization(lines: &[&[u8]]) -> Vec<(String, Vec<u8>)> {
    lines
        .iter()
        .map(|line| ("authorization".to_owned(), line.to_vec()))
        .collect()
}

#[test]
fn a_bearer_line_yields_its_secret() {
    match extracted(authorization(&[b"bearer hunter2"])) {
        Extraction::One(Credential::Bearer(secret)) => assert_eq!(secret.expose(), "hunter2"),
        other => panic!("expected one bearer credential, got {other:?}"),
    }
}

#[test]
fn an_empty_bearer_secret_carries_no_credential() {
    // hyper trims trailing spaces, but a secret of only whitespace must
    // still not reach the provider as an empty token.
    for line in [&b"Bearer"[..], &b"Bearer "[..], &b"Bearer \t "[..]] {
        assert!(
            matches!(extracted(authorization(&[line])), Extraction::None),
            "{line:?} must carry no credential"
        );
    }
}

#[test]
fn joined_or_repeated_authorization_values_conflict() {
    // hyper joins repeated lines with ", ", so the joined value is one line.
    assert!(matches!(
        extracted(authorization(&[b"Bearer a, Bearer b"])),
        Extraction::Conflict
    ));
    assert!(matches!(
        extracted(authorization(&[b"Bearer a,"])),
        Extraction::Conflict
    ));
    // A request that carries two separate lines is the same offer.
    assert!(matches!(
        extracted(authorization(&[b"Bearer a", b"Bearer b"])),
        Extraction::Conflict
    ));
    assert!(matches!(
        extracted(authorization(&[b"Bearer a", b"Basic b"])),
        Extraction::Conflict
    ));
}

#[test]
fn a_secret_that_is_not_utf8_carries_no_credential() {
    assert!(matches!(
        extracted(authorization(&[b"Bearer hu\xffnter2"])),
        Extraction::None
    ));
}

#[test]
fn hello_me_answers_400_when_the_request_offers_two_credentials() {
    let kernel = finished(Shared::default());
    let denied = guarded_me(&kernel, authorization(&[b"Bearer a, Bearer b"]));
    assert_eq!(denied.status, 400);
    assert_eq!(
        text(&denied),
        "the request offers more than one credential\n"
    );
}

#[test]
fn hello_me_fails_without_a_principal_instead_of_naming_one() {
    let kernel = finished(Shared::default());
    let outcome = block_on(me_route(
        Ctx::root(&kernel),
        request(Method::Get, "/hello/me"),
    ));
    match outcome {
        Ok(response) => panic!("greeted without a principal, status {}", response.status),
        Err(failure) => {
            assert_eq!(failure.code(), failures::PRINCIPAL_MISSING);
            assert_eq!(failure.status(), 500);
        }
    }
}
