//! Shared harness: a real `serve` on a current-thread tokio runtime in its
//! own thread, a kernel with test routes, and a JSON log read from memory.

use std::collections::VecDeque;
use std::io::{ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll};
use std::time::{Duration, SystemTime};
use timbre_edge_http::{HttpConfig, serve};
use timbre_kernel::http::{Body, Chunks, Method, Request, Response};
use timbre_kernel::{
    BoxFuture, Ctx, Fallible, Format, LogFilter, Logger, Registry, Runtime, ThreadRuntime,
};

/// A running `serve`, its bound address and the log it has written.
pub(crate) struct TestServer {
    pub(crate) address: SocketAddr,
    log: Arc<LogSink>,
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
            runtime,
            thread: Some(thread),
        }
    }

    /// Send `request` on a fresh connection, collecting the answer until the
    /// server closes it. A reset or timeout yields whatever arrived, so a
    /// refused head can show up as 431 or as a closed connection.
    #[expect(
        clippy::expect_used,
        clippy::panic,
        reason = "test helper: an unconnectable server or a broken read is a hard failure"
    )]
    pub(crate) fn roundtrip(&self, request: &str) -> String {
        let mut stream = TcpStream::connect(self.address).expect("the server accepts");
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .expect("a read timeout");
        if stream.write_all(request.as_bytes()).is_err() {
            return String::new(); // refused the head before the write landed
        }
        let mut bytes = Vec::new();
        let mut chunk = [0u8; 4096];
        loop {
            match stream.read(&mut chunk) {
                Ok(0) => break,
                Ok(read) => bytes.extend_from_slice(chunk.get(..read).unwrap_or(&[])),
                Err(error)
                    if matches!(
                        error.kind(),
                        ErrorKind::UnexpectedEof
                            | ErrorKind::ConnectionReset
                            | ErrorKind::BrokenPipe
                            | ErrorKind::WouldBlock
                    ) =>
                {
                    break;
                }
                Err(error) => panic!("reading the answer failed: {error}"),
            }
        }
        String::from_utf8_lossy(&bytes).into_owned()
    }

    pub(crate) fn status_line(&self, request: &str) -> String {
        self.roundtrip(request)
            .lines()
            .next()
            .unwrap_or_default()
            .to_owned()
    }

    /// The response body after the blank line, chunk-decoded if needed.
    #[expect(
        clippy::expect_used,
        reason = "test helper: a headless answer is the failure to report"
    )]
    pub(crate) fn body(&self, request: &str) -> String {
        let response = self.roundtrip(request);
        let (head, body) = response
            .split_once("\r\n\r\n")
            .expect("headers, a blank line, then a body");
        if head
            .to_ascii_lowercase()
            .contains("transfer-encoding: chunked")
        {
            dechunk(body)
        } else {
            body.to_owned()
        }
    }

    pub(crate) fn lines(&self) -> Vec<String> {
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

/// Concatenate the payloads of a `chunked` transfer body.
fn dechunk(body: &str) -> String {
    let mut out = String::new();
    let mut rest = body;
    while let Some((size, tail)) = rest.split_once("\r\n") {
        let Ok(len) = usize::from_str_radix(size.trim_end(), 16) else {
            break;
        };
        let Some(chunk) = tail.get(..len) else { break };
        out.push_str(chunk);
        rest = tail.get(len + 2..).unwrap_or_default();
    }
    out
}

/// `METHOD target HTTP/1.1` with `connection: close` and the given extras.
pub(crate) fn request_with(method: &str, target: &str, headers: &[String], body: &str) -> String {
    let mut head =
        format!("{method} {target} HTTP/1.1\r\nhost: timbre.test\r\nconnection: close\r\n");
    for header in headers {
        head.push_str(header);
        head.push_str("\r\n");
    }
    format!("{head}\r\n{body}")
}
