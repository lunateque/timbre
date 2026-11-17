//! The example module: it exists to exercise the kernel contracts (a log
//! event, a failure, a signal handler, a route, a CLI command, a declared
//! permission) and to prove that a module can be deleted with its directory.
//! It goes away when the first real module lands.

pub mod failures;
pub mod log_events;

use log_events::GREETED;
use timbre_kernel::http::{Body, Method, Request, Response};
use timbre_kernel::signals::Started;
use timbre_kernel::{BoxFuture, Ctx, Fallible, Permission, Registry, log};

/// The permission `hello` declares for granting; the check that enforces it
/// lands with route guards.
pub const GREET: Permission = Permission::new("hello.greet");

/// Register the module: one handler for `Started`, a route, a command and a
/// declared permission.
///
/// # Errors
///
/// Propagates the kernel's refusal of a duplicate registration.
pub fn register(registry: &mut Registry) -> Fallible {
    registry.on::<Started>(greet);
    registry.route(Method::Get, "/hello", hello_route)?;
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

/// `timbre hello`: greets through the logger, like the `Started` handler.
fn run_command(ctx: Ctx, _: Vec<String>) -> BoxFuture<Fallible> {
    Box::pin(async move {
        log!(ctx, info, GREETED, greeting = "hello");
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};
    use std::sync::Arc;
    use timbre_kernel::testing::{Shared, epoch};
    use timbre_kernel::{Format, Kernel, LogFilter, Logger, RouteMatch, ThreadRuntime, block_on};

    fn finished(sink: Shared) -> Arc<Kernel> {
        let registry = Registry::new().module(register).expect("hello registers");
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

    #[test]
    fn the_route_answers_200_hello() {
        let kernel = finished(Shared::default());
        let ctx = Ctx::root(&kernel);
        let found = match kernel.find_route(Method::Get, "/hello") {
            RouteMatch::Found { handler, param } => {
                assert_eq!(param, None);
                handler
            }
            other => panic!("expected a found route, got {other:?}"),
        };
        let response =
            block_on(found(ctx, request(Method::Get, "/hello"))).expect("the route answers");
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
}
