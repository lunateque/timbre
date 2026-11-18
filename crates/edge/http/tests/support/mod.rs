//! Shared harness: a real `serve` on a current-thread tokio runtime in its
//! own thread, a kernel with test routes, and a JSON log read from memory.
//!
//! Every test file includes this module whole; helpers one file does not use
//! must not fail the no-dead-code gate in the others. The fixtures live in
//! sibling files, re-exported here so `support::name` stays the one path:
//! plain handlers in [`handlers`], the guard fixtures in [`guard`], raw-wire
//! request builders in [`wire`].
#![expect(dead_code, reason = "shared harness; each test file uses a subset")]

pub(crate) mod guard;
pub(crate) mod handlers;
pub(crate) mod wire;

use http_body_util::BodyExt;
use std::fmt::Write as _;
use std::io::Write;
use std::net::{SocketAddr, TcpStream};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::SystemTime;
use timbre_edge_http::{HttpConfig, serve};
use timbre_kernel::http::Method;
use timbre_kernel::{
    CredentialKind, Ctx, Format, Guard, Kernel, LogFilter, Logger, Registry, Runtime, ThreadRuntime,
};

use handlers::{
    bad_declared_stream, broken_stream, declared_stream, double_declared_stream, echo, echo_query,
    endless_stream, long_stream, param, short_stream, slow, slow_stream, stream, wrong_length,
};

/// A running `serve`, its bound address and the log it has written.
pub(crate) struct TestServer {
    pub(crate) address: SocketAddr,
    log: Arc<LogSink>,
    kernel: Arc<Kernel>,
    counter: Arc<dyn guard::Counter>,
    runtime: Arc<ThreadRuntime>,
    thread: Option<std::thread::JoinHandle<()>>,
}

/// The JSON log, collected in memory behind a shared lock.
#[derive(Default)]
struct LogSink(Mutex<Vec<u8>>);

/// A boxed `Write` handle onto the shared sink.
struct LogSinkRef(Arc<LogSink>);

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
fn routes(counter: Arc<guard::SharedCounter>) -> Registry {
    let mut registry = Registry::new();
    for (method, pattern, handler) in [
        (Method::Get, "/echo", echo as fn(_, _) -> _),
        (Method::Post, "/echo", echo),
        (Method::Post, "/query", echo_query),
        (Method::Get, "/stream", stream),
        (Method::Get, "/declared-stream", declared_stream),
        (Method::Head, "/declared-stream", declared_stream),
        (Method::Get, "/short-stream", short_stream),
        (Method::Get, "/long-stream", long_stream),
        (Method::Get, "/bad-declared-stream", bad_declared_stream),
        (
            Method::Get,
            "/double-declared-stream",
            double_declared_stream,
        ),
        (Method::Get, "/broken", broken_stream),
        (Method::Get, "/slow-stream", slow_stream),
        (Method::Get, "/endless", endless_stream),
        (Method::Get, "/slow", slow),
        (Method::Get, "/wrong-length", wrong_length),
        (Method::Get, "/rest/{method}", param),
        (Method::Post, "/rest/{method}", param),
    ] {
        registry
            .route(method, pattern, Guard::Public, handler)
            .expect("a route");
    }
    // The guarded routes: `/guarded` asks the scheme the fake provider
    // owns; `/admin` asks a permission the fake identity does not hold.
    registry
        .route(
            Method::Get,
            "/guarded",
            guard::read_guard(),
            guard::guarded_echo,
        )
        .expect("the read route");
    registry
        .route(
            Method::Get,
            "/admin",
            guard::admin_guard(),
            guard::guarded_echo,
        )
        .expect("the admin route");
    // A guarded form POST: its credential travels in the body, so this
    // route proves the edge buffers the body before checking the guard.
    registry
        .route(
            Method::Post,
            "/guarded-form",
            guard::form_guard(),
            guard::guarded_echo,
        )
        .expect("the guarded form route");
    // A guarded GET whose handler reports the body size: proves the body
    // buffered after a passed check still reaches the handler.
    registry
        .route(
            Method::Get,
            "/guarded-body",
            guard::read_guard(),
            guard::guarded_body_len,
        )
        .expect("the guarded body route");
    registry
        .credential_provider(
            CredentialKind::Scheme(guard::TEST_SCHEME),
            guard::provider() as Arc<dyn timbre_kernel::CredentialProvider>,
        )
        .expect("the fake scheme owner");
    registry
        .provide::<dyn guard::Counter>(counter)
        .expect("the counter port");
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
        std::thread::sleep(std::time::Duration::from_millis(5));
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
        let counter = Arc::new(guard::SharedCounter::default());
        let runtime = Arc::new(ThreadRuntime::new());
        let logger = Logger::new(
            LogFilter::default(),
            Format::Json,
            Box::new(LogSinkRef(Arc::clone(&log))),
            SystemTime::now,
        );
        let kernel =
            routes(Arc::clone(&counter)).finish(logger, Arc::clone(&runtime) as Arc<dyn Runtime>);
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
            counter,
            runtime,
            thread: Some(thread),
        }
    }

    /// How many times the guarded handler has actually run.
    pub(crate) fn guarded_calls(&self) -> usize {
        self.counter.calls()
    }

    /// Send one HTTP/2 (h2c, preface-detected) `GET target` and return the
    /// raw answer `"<status>\n<name>: <value>\n...\n\n<body>"`. hyper's own
    /// client speaks it, so a length the edge put on the wire is read back
    /// the way a real HTTP/2 client reads it.
    #[expect(
        clippy::expect_used,
        reason = "test helper: an h2 roundtrip that fails is the failure to report"
    )]
    pub(crate) fn roundtrip_h2(&self, target: &str) -> String {
        let address = self.address;
        let text = target.to_owned();
        std::thread::spawn(move || {
            let tokio = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("a tokio runtime for the h2 client");
            tokio.block_on(async move {
                let stream = tokio::net::TcpStream::connect(address)
                    .await
                    .expect("the server accepts");
                let io = hyper_util::rt::TokioIo::new(stream);
                let (mut sender, connection) =
                    hyper::client::conn::http2::handshake(hyper_util::rt::TokioExecutor::new(), io)
                        .await
                        .expect("the h2 handshake completes");
                // The connection future drives the stream: it must run.
                let driver = tokio::spawn(async move {
                    let _ = connection.await;
                });
                let request = hyper::Request::get(&text)
                    .body(http_body_util::Empty::<hyper::body::Bytes>::new())
                    .expect("a GET request");
                let response = sender
                    .send_request(request)
                    .await
                    .expect("the request is answered");
                let status = response.status().as_u16();
                let mut head = format!("{status}\n");
                for (name, value) in response.headers() {
                    let text = String::from_utf8_lossy(value.as_bytes()).into_owned();
                    let _ = writeln!(head, "{name}: {text}");
                }
                let body = response
                    .into_body()
                    .collect()
                    .await
                    .expect("the body arrives")
                    .to_bytes();
                drop(driver);
                format!("{head}\n{}", String::from_utf8_lossy(&body))
            })
        })
        .join()
        .expect("the h2 client thread ends")
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
            .set_read_timeout(Some(std::time::Duration::from_secs(10)))
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

pub(crate) use wire::{body_of, request_with, roundtrip_on};
