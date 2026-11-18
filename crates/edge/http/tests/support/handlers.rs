//! Plain test handlers shared by every test file: bodies, streams, delays
//! and failures a route can answer with, plus the stream sources behind
//! them.

use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll};
use std::time::Duration;
use timbre_kernel::http::{Body, Chunks, Request, Response};
use timbre_kernel::{BoxFuture, Ctx, Failure, Fallible};

pub(crate) fn echo(_: Ctx, _: Request) -> BoxFuture<Fallible<Response>> {
    Box::pin(async {
        Ok(Response::new(200)
            .with_header("content-type", b"text/plain; charset=utf-8")
            .with_body(Body::text("hello")))
    })
}

/// The merged query, in wire order: `key=value` pairs joined by `;`.
pub(crate) fn echo_query(_: Ctx, request: Request) -> BoxFuture<Fallible<Response>> {
    Box::pin(async move {
        let text = request
            .query
            .iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect::<Vec<String>>()
            .join(";");
        Ok(Response::new(200)
            .with_header("content-type", b"text/plain; charset=utf-8")
            .with_body(Body::text(&text)))
    })
}

/// A two-chunk stream body: `Body::Stream` must reach the wire whole.
struct TwoChunks(VecDeque<Vec<u8>>);

impl Chunks for TwoChunks {
    fn poll_chunk(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Fallible<Vec<u8>>>> {
        match self.get_mut().0.pop_front() {
            Some(chunk) => Poll::Ready(Some(Ok(chunk))),
            None => Poll::Ready(None),
        }
    }
}

pub(crate) fn stream(_: Ctx, _: Request) -> BoxFuture<Fallible<Response>> {
    Box::pin(async {
        let chunks = VecDeque::from(vec![b"first-".to_vec(), b"second".to_vec()]);
        Ok(Response::new(200)
            .with_header("content-type", b"text/plain; charset=utf-8")
            .with_body(Body::Stream(Box::pin(TwoChunks(chunks)))))
    })
}

/// The captured `{name}` path parameter.
pub(crate) fn param(_: Ctx, request: Request) -> BoxFuture<Fallible<Response>> {
    Box::pin(async move {
        let text = request.param.unwrap_or_default();
        Ok(Response::new(200).with_body(Body::text(&text)))
    })
}

/// A stream whose first chunk arrives only after 400 ms: the body outlives
/// a handler timeout configured much shorter, so the deadline must not cut
/// it.
struct Delayed {
    sleep: Option<Pin<Box<tokio::time::Sleep>>>,
    sent: bool,
}

impl Chunks for Delayed {
    fn poll_chunk(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Fallible<Vec<u8>>>> {
        let this = self.get_mut();
        if this
            .sleep
            .as_mut()
            .is_some_and(|sleep| sleep.as_mut().poll(cx).is_pending())
        {
            return Poll::Pending;
        }
        this.sleep = None;
        if this.sent {
            return Poll::Ready(None);
        }
        this.sent = true;
        Poll::Ready(Some(Ok(b"late".to_vec())))
    }
}

pub(crate) fn slow_stream(_: Ctx, _: Request) -> BoxFuture<Fallible<Response>> {
    Box::pin(async {
        let sleep = Box::pin(tokio::time::sleep(Duration::from_millis(400)));
        Ok(Response::new(200).with_body(Body::Stream(Box::pin(Delayed {
            sleep: Some(sleep),
            sent: false,
        }))))
    })
}

/// A stream that never runs out: 64 KiB per chunk, forever, so a client that
/// stops reading leaves the server with bytes it cannot write. The body only
/// ends when the connection does.
pub(crate) fn endless_stream(_: Ctx, _: Request) -> BoxFuture<Fallible<Response>> {
    Box::pin(async {
        Ok(Response::new(200)
            .with_header("content-type", b"application/octet-stream")
            .with_body(Body::Stream(Box::pin(Endless { written: 0 }))))
    })
}

struct Endless {
    written: usize,
}

impl Chunks for Endless {
    fn poll_chunk(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Fallible<Vec<u8>>>> {
        let this = self.get_mut();
        this.written = this.written.wrapping_add(1);
        Poll::Ready(Some(Ok(vec![b'x'; 64 * 1024])))
    }
}

/// A handler that answers only after 2 s: past every timeout a test sets.
pub(crate) fn slow(_: Ctx, _: Request) -> BoxFuture<Fallible<Response>> {
    Box::pin(async {
        tokio::time::sleep(Duration::from_millis(2000)).await;
        Ok(Response::new(200).with_body(Body::text("eventually")))
    })
}

/// A response that carries its own wrong framing headers: the in-memory
/// body's `content-length` must not reach the wire, hyper overrides it.
pub(crate) fn wrong_length(_: Ctx, _: Request) -> BoxFuture<Fallible<Response>> {
    Box::pin(async {
        Ok(Response::new(200)
            .with_header("content-length", b"5")
            .with_header("transfer-encoding", b"chunked")
            .with_header("connection", b"close")
            .with_body(Body::text("hello world")))
    })
}

/// The same two chunks `stream` sends, but with the exact `content-length`
/// a handler would declare for them: the one framing header a streamed
/// body is allowed to keep, and the size the request log must report.
pub(crate) fn declared_stream(_: Ctx, _: Request) -> BoxFuture<Fallible<Response>> {
    Box::pin(async {
        let chunks = VecDeque::from(vec![b"first-".to_vec(), b"second".to_vec()]);
        Ok(Response::new(200)
            .with_header("content-length", b"12")
            .with_body(Body::Stream(Box::pin(TwoChunks(chunks)))))
    })
}

/// A stream that yields five bytes but declares twelve: the promise the
/// wire cannot keep, which must cost the connection, never a desynchronized
/// keep-alive stream.
pub(crate) fn short_stream(_: Ctx, _: Request) -> BoxFuture<Fallible<Response>> {
    Box::pin(async {
        let chunks = VecDeque::from(vec![b"five!".to_vec()]);
        Ok(Response::new(200)
            .with_header("content-length", b"12")
            .with_body(Body::Stream(Box::pin(TwoChunks(chunks)))))
    })
}

/// A stream whose declared length is no number at all: the edge drops the
/// header, logs it and lets hyper frame the body its own way.
pub(crate) fn bad_declared_stream(_: Ctx, _: Request) -> BoxFuture<Fallible<Response>> {
    Box::pin(async {
        let chunks = VecDeque::from(vec![b"first-".to_vec(), b"second".to_vec()]);
        Ok(Response::new(200)
            .with_header("content-length", b"twelve")
            .with_body(Body::Stream(Box::pin(TwoChunks(chunks)))))
    })
}

/// A stream with two `content-length` headers that agree: still a value the
/// edge cannot pass through, because one wire header may carry only one.
pub(crate) fn double_declared_stream(_: Ctx, _: Request) -> BoxFuture<Fallible<Response>> {
    Box::pin(async {
        let chunks = VecDeque::from(vec![b"first-".to_vec(), b"second".to_vec()]);
        Ok(Response::new(200)
            .with_header("content-length", b"12")
            .with_header("content-length", b"12")
            .with_body(Body::Stream(Box::pin(TwoChunks(chunks)))))
    })
}

/// A stream that overshoots its declared length: the wire may not carry
/// more than the promise, and the connection must survive the lie intact.
pub(crate) fn long_stream(_: Ctx, _: Request) -> BoxFuture<Fallible<Response>> {
    Box::pin(async {
        let chunks = VecDeque::from(vec![b"first-".to_vec(), b"second".to_vec()]);
        Ok(Response::new(200)
            .with_header("content-length", b"5")
            .with_body(Body::Stream(Box::pin(TwoChunks(chunks)))))
    })
}

/// A stream that yields one chunk and then fails: the failure cannot change
/// the head any more, so it must at least reach the log.
// `Sync`: a request body borrow must cross threads while a guard awaits.
struct Breaks {
    polls: AtomicUsize,
}

impl Chunks for Breaks {
    fn poll_chunk(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Fallible<Vec<u8>>>> {
        let count = self.polls.fetch_add(1, Ordering::SeqCst);
        match count {
            0 => Poll::Ready(Some(Ok(b"half".to_vec()))),
            1 => Poll::Ready(Some(Err(Box::new(CloseReason) as Box<dyn Failure>))),
            _ => Poll::Ready(None),
        }
    }
}

/// The kernel has no public unit failure to raise from a test stream, so the
/// harness declares one; its code names the situation in the log.
struct CloseReason;

impl std::fmt::Debug for CloseReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("test stream failure")
    }
}

impl Failure for CloseReason {
    fn code(&self) -> timbre_kernel::FailureCode {
        timbre_kernel::FailureCode::new("http.body_failed")
    }

    fn status(&self) -> u16 {
        500
    }

    fn public_message(&self) -> &'static str {
        "the test stream broke"
    }
}

pub(crate) fn broken_stream(_: Ctx, _: Request) -> BoxFuture<Fallible<Response>> {
    Box::pin(async {
        Ok(Response::new(200).with_body(Body::Stream(Box::pin(Breaks {
            polls: AtomicUsize::new(0),
        }))))
    })
}
