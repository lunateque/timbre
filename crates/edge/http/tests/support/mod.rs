//! Shared harness: a real `serve` on a current-thread tokio runtime in its
//! own thread, a kernel with test routes, and a JSON log read from memory.
//!
//! Every test file includes this module whole; helpers one file does not use
//! must not fail the no-dead-code gate in the others.
#![expect(dead_code, reason = "shared harness; each test file uses a subset")]

use std::collections::VecDeque;
use std::io::Write;
use std::net::{SocketAddr, TcpStream};
use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll};
use std::time::{Duration, SystemTime};
use timbre_edge_http::{HttpConfig, serve};
use timbre_kernel::http::{Body, Chunks, Method, Request, Response};
use timbre_kernel::{
    BoxFuture, Ctx, Failure, Fallible, Format, Kernel, LogFilter, Logger, Registry, Runtime,
    ThreadRuntime,
};

/// A running `serve`, its bound address and the log it has written.
pub(crate) struct TestServer {
    pub(crate) address: SocketAddr,
    log: Arc<LogSink>,
    kernel: Arc<Kernel>,
    runtime: Arc<ThreadRuntime>,
    thread: Option<std::thread::JoinHandle<()>>,
}

/// The JSON log, collected in memory behind a shared lock.
#[derive(Default)]
struct LogSink(Mutex<Vec<u8>>);

/// A boxed `Write` handle onto the shared sink.
struct LogSinkRef(Arc<LogSink>);

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

/// A handler that answers only after 2 s: past every timeout a test sets.
pub(crate) fn slow(_: Ctx, _: Request) -> BoxFuture<Fallible<Response>> {
    Box::pin(async {
        tokio::time::sleep(Duration::from_millis(2000)).await;
        Ok(Response::new(200).with_body(Body::text("eventually")))
    })
}

/// A response that carries its own wrong framing headers: hyper must
/// override them, not send them.
pub(crate) fn wrong_length(_: Ctx, _: Request) -> BoxFuture<Fallible<Response>> {
    Box::pin(async {
        Ok(Response::new(200)
            .with_header("content-length", b"5")
            .with_header("transfer-encoding", b"chunked")
            .with_header("connection", b"close")
            .with_body(Body::text("hello world")))
    })
}

/// A stream that yields one chunk and then fails: the failure cannot change
/// the head any more, so it must at least reach the log.
struct Breaks {
    polls: std::cell::Cell<usize>,
}

impl Chunks for Breaks {
    fn poll_chunk(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Fallible<Vec<u8>>>> {
        let count = self.polls.get();
        self.polls.set(count + 1);
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
            polls: std::cell::Cell::new(0),
        }))))
    })
}

impl LogSink {
    fn append(&self, buf: &[u8]) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .extend_from_slice(buf);
    }

    fn lines(&self) -> Vec<String> {
        let bytes = self
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        String::from_utf8_lossy(&bytes)
            .lines()
            .map(str::to_owned)
            .collect()
    }
}

impl Write for LogSinkRef {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.append(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[expect(
    clippy::expect_used,
    reason = "test fixture: a refused registration is a bug in this file"
)]
fn routes() -> Registry {
    let mut registry = Registry::new();
    for (method, pattern, handler) in [
        (Method::Get, "/echo", echo as fn(_, _) -> _),
        (Method::Post, "/echo", echo),
        (Method::Post, "/query", echo_query),
        (Method::Get, "/stream", stream),
        (Method::Get, "/broken", broken_stream),
        (Method::Get, "/slow-stream", slow_stream),
        (Method::Get, "/slow", slow),
        (Method::Get, "/wrong-length", wrong_length),
        (Method::Get, "/rest/{method}", param),
        (Method::Post, "/rest/{method}", param),
    ] {
        registry.route(method, pattern, handler).expect("a route");
    }
    registry
}

/// The ephemeral port `serve` reports on its `http.listening` line.
#[expect(
    clippy::panic,
    reason = "test helper: a server that never binds must fail loudly"
)]
fn wait_for_address(log: &Arc<LogSink>) -> SocketAddr {
    for _ in 0..2000 {
        let bound = log
            .lines()
            .iter()
            .filter(|line| line.contains(r#""event":"http.listening""#))
            .find_map(|line| {
                line.split(r#""addr":""#)
                    .nth(1)?
                    .split('"')
                    .next()?
                    .parse::<SocketAddr>()
                    .ok()
            });
        if let Some(address) = bound {
            return address;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("serve never logged its bound address")
}

impl TestServer {
    /// Start `serve` on a current-thread tokio runtime in its own thread,
    /// defaults adjusted by `configure`, and wait for the bound address.
    #[expect(
        clippy::expect_used,
        reason = "test fixture: a runtime that cannot start must say why"
    )]
    pub(crate) fn start(configure: impl FnOnce(&mut HttpConfig)) -> TestServer {
        let mut config = HttpConfig {
            listen: vec!["127.0.0.1:0".to_owned()],
            ..HttpConfig::default()
        };
        configure(&mut config);
        let log = Arc::new(LogSink::default());
        let runtime = Arc::new(ThreadRuntime::new());
        let logger = Logger::new(
            LogFilter::default(),
            Format::Json,
            Box::new(LogSinkRef(Arc::clone(&log))),
            SystemTime::now,
        );
        let kernel = routes().finish(logger, Arc::clone(&runtime) as Arc<dyn Runtime>);
        let ctx = Ctx::root(&kernel);
        let thread = std::thread::spawn(move || {
            let tokio = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("a tokio runtime for the test server");
            drop(tokio.block_on(serve(ctx, config)));
        });
        let address = wait_for_address(&log);
        TestServer {
            address,
            log,
            kernel,
            runtime,
            thread: Some(thread),
        }
    }

    /// Send `request` on a fresh connection, collecting the answer until the
    /// server closes it. A reset or timeout yields whatever arrived, so a
    /// refused head can show up as 431 or as a closed connection.
    pub(crate) fn roundtrip(&self, request: &str) -> String {
        roundtrip_on(self.connect(), request)
    }

    /// A connection to the server with a read timeout set; `roundtrip` uses
    /// one per request, keep-alive tests reuse a single one.
    #[expect(
        clippy::expect_used,
        reason = "test helper: an unconnectable server is a hard failure"
    )]
    pub(crate) fn connect(&self) -> TcpStream {
        let stream = TcpStream::connect(self.address).expect("the server accepts");
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .expect("a read timeout");
        stream
    }

    pub(crate) fn status_line(&self, request: &str) -> String {
        self.roundtrip(request)
            .lines()
            .next()
            .unwrap_or_default()
            .to_owned()
    }

    /// The response body after the blank line, chunk-decoded if needed.
    pub(crate) fn body(&self, request: &str) -> String {
        let response = self.roundtrip(request);
        body_of(&response)
    }

    /// Every log line written so far; the kernel logger is asynchronous, so
    /// the queue is flushed before reading.
    pub(crate) fn lines(&self) -> Vec<String> {
        self.kernel.flush_log();
        self.log.lines()
    }

    #[expect(
        clippy::expect_used,
        reason = "test helper: a serve that dies on shutdown is the bug"
    )]
    pub(crate) fn stop(mut self) {
        self.runtime.trigger_shutdown();
        if let Some(thread) = self.thread.take() {
            thread.join().expect("serve returns cleanly");
        }
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        // A test that never reached `stop` still lets its server thread end.
        self.runtime.trigger_shutdown();
    }
}

/// Raw-wire helpers shared by every test file.
pub(crate) mod wire;
pub(crate) use wire::{body_of, request_with, roundtrip_on};
