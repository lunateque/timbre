//! Routes: the method-plus-pattern table a protocol edge looks up per request.

use crate::ctx::Ctx;
use crate::failure::Fallible;
use crate::failures::KernelFailure;
use crate::future::BoxFuture;
use crate::guard::Guard;
use crate::http::{Method, Request, Response};
use crate::names;

/// The handler shape of a route: takes ownership of the request and answers
/// with a response future.
pub type RouteHandler = fn(Ctx, Request) -> BoxFuture<Fallible<Response>>;

/// One pattern segment: exact text or a `{name}` capture.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Segment {
    Literal(String),
    Param,
}

struct Entry {
    segments: Vec<Segment>,
    method: Method,
    guard: Guard,
    handler: RouteHandler,
}

/// One route the table matched to a request, with everything the edge may
/// see about it. The handler itself is private: only
/// [`Kernel::guarded`](crate::Kernel::guarded) can turn this match into an
/// [`Authorized`](crate::Authorized), and only
/// [`Authorized::run`](crate::Authorized::run) can invoke the handler, so no
/// call site can reach a handler around its guard.
// No `PartialEq`: comparing `RouteHandler` function pointers is unreliable
// across codegen units, so callers never see one.
#[derive(Clone, Debug)]
pub struct Matched {
    guard: Guard,
    param: Option<String>,
    handler: RouteHandler,
}

impl Matched {
    /// The guard the kernel must run before the handler; every route has one,
    /// because a route without a declared guard cannot register.
    #[must_use]
    pub fn guard(&self) -> Guard {
        self.guard
    }

    /// The captured trailing segment, if the pattern ends in `{name}`.
    #[must_use]
    pub fn param(&self) -> Option<&str> {
        self.param.as_deref()
    }

    pub(crate) fn handler(&self) -> RouteHandler {
        self.handler
    }
}

/// What the route table knows about one request path.
#[derive(Clone, Debug)]
pub enum RouteMatch {
    /// A route answers this method and path.
    Found(Matched),
    /// The path has routes, but none for this method (the HTTP edge answers 405).
    MethodNotAllowed,
    /// No route has this path (the HTTP edge answers 404).
    NotFound,
}

/// The route table; owned by the kernel, filled by `Registry::route`.
#[derive(Default)]
pub(crate) struct RouteTable {
    entries: Vec<Entry>,
}

impl RouteTable {
    pub(crate) fn add(
        &mut self,
        method: Method,
        pattern: &'static str,
        guard: Guard,
        handler: RouteHandler,
    ) -> Fallible {
        let Some(segments) = parse(pattern) else {
            return Err(Box::new(KernelFailure::RouteInvalid { pattern }));
        };
        if self
            .entries
            .iter()
            .any(|entry| entry.segments == segments && entry.method == method)
        {
            return Err(Box::new(KernelFailure::RouteTaken {
                method: method.as_str(),
                pattern,
            }));
        }
        self.entries.push(Entry {
            segments,
            method,
            guard,
            handler,
        });
        Ok(())
    }

    /// Match `method` against `path`. Literal routes win over a capture at the
    /// same depth, so `/health` beats `/rest/{method}` on that path.
    pub(crate) fn find(&self, method: Method, path: &str) -> RouteMatch {
        let parts: Vec<&str> = path.split('/').filter(|part| !part.is_empty()).collect();
        // Two passes, literal routes first: a literal shadows a capture at the
        // same depth, and any match beats 405/404 regardless of registration order.
        let mut seen_path = false;
        for literal_pass in [true, false] {
            for entry in &self.entries {
                let captures = matches!(entry.segments.last(), Some(Segment::Param));
                if captures == literal_pass {
                    continue;
                }
                let Some(found) = match_entry(entry, &parts) else {
                    continue;
                };
                if entry.method == method {
                    return found;
                }
                seen_path = true;
            }
        }
        if seen_path {
            RouteMatch::MethodNotAllowed
        } else {
            RouteMatch::NotFound
        }
    }
}

/// The `Found` outcome when `parts` matches this entry, `None` otherwise.
fn match_entry(entry: &Entry, parts: &[&str]) -> Option<RouteMatch> {
    let captures = matches!(entry.segments.last(), Some(Segment::Param));
    if parts.len() != entry.segments.len() {
        return None;
    }
    let literals = entry.segments.len() - usize::from(captures);
    let exact =
        entry
            .segments
            .iter()
            .zip(parts)
            .take(literals)
            .all(|(segment, part)| match segment {
                Segment::Literal(text) => text == part,
                // Only the last segment captures, and that pair is not compared.
                Segment::Param => false,
            });
    if !exact {
        return None;
    }
    let param = captures
        .then(|| parts.get(literals))
        .flatten()
        .map(|captured| (*captured).to_owned());
    Some(RouteMatch::Found(Matched {
        guard: entry.guard,
        param,
        handler: entry.handler,
    }))
}

/// Split a pattern into segments, `None` when malformed: it must start with
/// `/`, have no empty segments, and `{name}` captures only as the last segment.
fn parse(pattern: &str) -> Option<Vec<Segment>> {
    let body = pattern.strip_prefix('/')?;
    let segments = if body.is_empty() {
        Vec::new()
    } else {
        let parts: Vec<&str> = body.split('/').collect();
        let last = parts.len() - 1;
        let mut segments = Vec::with_capacity(parts.len());
        for (index, part) in parts.into_iter().enumerate() {
            if let Some(name) = part.strip_prefix('{') {
                let name = name.strip_suffix('}')?;
                if index != last || !names::is_valid_segment(name) {
                    return None;
                }
                segments.push(Segment::Param);
            } else if part.is_empty() || part.contains(['{', '}']) {
                return None;
            } else {
                segments.push(Segment::Literal(part.to_owned()));
            }
        }
        segments
    };
    Some(segments)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Ctx, Format, LogFilter, Logger, ThreadRuntime, block_on, http::Body};
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};
    use std::sync::Arc;

    // Each handler echoes its own name in the body, so tests identify the
    // matched route by what it answers, never by comparing function pointers.
    fn alpha(_: Ctx, _: Request) -> BoxFuture<Fallible<Response>> {
        Box::pin(async { Ok(Response::new(200).with_body(Body::text("alpha"))) })
    }

    fn beta(_: Ctx, _: Request) -> BoxFuture<Fallible<Response>> {
        Box::pin(async { Ok(Response::new(200).with_body(Body::text("beta"))) })
    }

    fn table(patterns: &[(Method, &'static str, Guard, RouteHandler)]) -> RouteTable {
        let mut table = RouteTable::default();
        for (method, pattern, guard, handler) in patterns {
            table
                .add(*method, pattern, *guard, *handler)
                .unwrap_or_else(|_| panic!("pattern {pattern} must register"));
        }
        table
    }

    fn answer(table: &RouteTable, method: Method, path: &str) -> (String, Option<String>) {
        match table.find(method, path) {
            RouteMatch::Found(found) => {
                let handler = found.handler();
                let kernel = RegistryStub::kernel();
                let request = Request {
                    method,
                    path: path.to_owned(),
                    param: None,
                    query: Vec::new(),
                    headers: Vec::new(),
                    peer: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 1),
                    client: IpAddr::V4(Ipv4Addr::LOCALHOST),
                    body: Body::Empty,
                };
                let response = block_on(handler(Ctx::root(&kernel), request)).expect("handler ok");
                let bytes = response.body.as_bytes().expect("in-memory body").to_vec();
                (String::from_utf8(bytes).expect("utf-8 marker"), found.param)
            }
            RouteMatch::MethodNotAllowed => ("405".to_owned(), None),
            RouteMatch::NotFound => ("404".to_owned(), None),
        }
    }

    // A minimal kernel to hand a `Ctx` to a handler.
    struct RegistryStub;

    impl RegistryStub {
        fn kernel() -> Arc<crate::Kernel> {
            crate::Registry::new().finish(
                Logger::new(
                    LogFilter::default(),
                    Format::Json,
                    Box::new(Vec::<u8>::new()),
                    || std::time::SystemTime::UNIX_EPOCH,
                ),
                Arc::new(ThreadRuntime::new()),
            )
        }
    }

    #[test]
    fn literal_routes_match_and_captures_return_their_segment() {
        let table = table(&[
            (Method::Get, "/hello", Guard::Public, alpha),
            (Method::Get, "/rest/{method}", Guard::Public, beta),
        ]);
        assert_eq!(
            answer(&table, Method::Get, "/hello"),
            ("alpha".to_owned(), None)
        );
        assert_eq!(
            answer(&table, Method::Get, "/rest/ping"),
            ("beta".to_owned(), Some("ping".to_owned()))
        );
    }

    #[test]
    fn a_literal_wins_over_a_capture_at_the_same_depth() {
        // Registered later, so first-match-by-order would pick the capture.
        let table = table(&[
            (Method::Get, "/rest/{method}", Guard::Public, beta),
            (Method::Get, "/rest/ping", Guard::Public, alpha),
        ]);
        assert_eq!(
            answer(&table, Method::Get, "/rest/ping"),
            ("alpha".to_owned(), None)
        );
    }

    #[test]
    fn other_methods_and_paths_answer_405_and_404() {
        let table = table(&[(Method::Get, "/hello", Guard::Public, alpha)]);
        assert_eq!(
            answer(&table, Method::Post, "/hello"),
            ("405".to_owned(), None)
        );
        assert_eq!(
            answer(&table, Method::Get, "/goodbye"),
            ("404".to_owned(), None)
        );
        assert_eq!(
            answer(&table, Method::Get, "/hello/deep"),
            ("404".to_owned(), None)
        );
    }

    #[test]
    fn a_duplicate_route_is_taken_and_a_malformed_pattern_is_invalid() {
        let mut table = RouteTable::default();
        table
            .add(Method::Get, "/hello", Guard::Public, alpha)
            .expect("first");
        let duplicate = table
            .add(Method::Get, "/hello", Guard::Public, beta)
            .expect_err("the same method and pattern twice");
        assert_eq!(duplicate.code(), crate::failures::ROUTE_TAKEN);
        // Another method on the same pattern is a different route.
        table
            .add(Method::Post, "/hello", Guard::Public, beta)
            .expect("other method");
        let bad: [&'static str; 6] = [
            "hello",
            "/hello//world",
            "/{x}/y",
            "/{Bad}",
            "{}",
            "/hello/{x}y",
        ];
        for pattern in bad {
            let error = table
                .add(Method::Get, pattern, Guard::Public, alpha)
                .expect_err("malformed pattern");
            assert_eq!(
                error.code(),
                crate::failures::ROUTE_INVALID,
                "for {pattern}"
            );
        }
    }
}
