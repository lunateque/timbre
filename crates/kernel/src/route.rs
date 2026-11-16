//! Routes: the method-plus-pattern table a protocol edge looks up per request.

use crate::ctx::Ctx;
use crate::failure::Fallible;
use crate::failures::KernelFailure;
use crate::future::BoxFuture;
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
    handler: RouteHandler,
}

/// What the route table knows about one request path.
// No `PartialEq`: comparing `RouteHandler` function pointers is unreliable
// across codegen units, so callers match on the variant and run the handler.
#[derive(Clone, Debug)]
pub enum RouteMatch {
    /// A route answers this method and path; `param` carries the value of a
    /// `{name}` capture, when the pattern has one.
    Found {
        /// The handler to run.
        handler: RouteHandler,
        /// The captured trailing segment, if the pattern ends in `{name}`.
        param: Option<String>,
    },
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
    Some(RouteMatch::Found {
        handler: entry.handler,
        param,
    })
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

    fn table(patterns: &[(Method, &'static str, RouteHandler)]) -> RouteTable {
        let mut table = RouteTable::default();
        for (method, pattern, handler) in patterns {
            table
                .add(*method, pattern, *handler)
                .unwrap_or_else(|_| panic!("pattern {pattern} must register"));
        }
        table
    }

    fn answer(table: &RouteTable, method: Method, path: &str) -> (String, Option<String>) {
        match table.find(method, path) {
            RouteMatch::Found { handler, param } => {
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
                (String::from_utf8(bytes).expect("utf-8 marker"), param)
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
            (Method::Get, "/hello", alpha),
            (Method::Get, "/rest/{method}", beta),
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
            (Method::Get, "/rest/{method}", beta),
            (Method::Get, "/rest/ping", alpha),
        ]);
        assert_eq!(
            answer(&table, Method::Get, "/rest/ping"),
            ("alpha".to_owned(), None)
        );
    }

    #[test]
    fn other_methods_and_paths_answer_405_and_404() {
        let table = table(&[(Method::Get, "/hello", alpha)]);
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
        table.add(Method::Get, "/hello", alpha).expect("first");
        let duplicate = table
            .add(Method::Get, "/hello", beta)
            .expect_err("the same method and pattern twice");
        assert_eq!(duplicate.code(), crate::failures::ROUTE_TAKEN);
        // Another method on the same pattern is a different route.
        table
            .add(Method::Post, "/hello", beta)
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
                .add(Method::Get, pattern, alpha)
                .expect_err("malformed pattern");
            assert_eq!(
                error.code(),
                crate::failures::ROUTE_INVALID,
                "for {pattern}"
            );
        }
    }
}
