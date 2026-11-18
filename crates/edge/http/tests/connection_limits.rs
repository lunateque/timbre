//! The per-client concurrent connection cap: one address gets at most
//! `max_connections_per_client` live connections, a trusted proxy address is
//! exempt, and a closed connection frees its peer's slot.

mod support;

use std::io::Write;
use std::net::{SocketAddr, TcpStream};
use support::TestServer;

/// Hold `count` connections from this client open, request sent and answer
/// unread, so each stays live (`/slow` answers only after 2 s, far longer
/// than any assertion below takes).
#[expect(
    clippy::expect_used,
    reason = "test helper: a request that cannot go out is the failure to report"
)]
fn hold_open(server: &TestServer, count: usize) -> Vec<TcpStream> {
    let mut held = Vec::new();
    for _ in 0..count {
        let mut stream = server.connect();
        stream
            .write_all(b"GET /slow HTTP/1.1\r\nhost: timbre.test\r\nconnection: close\r\n\r\n")
            .expect("the request goes out");
        held.push(stream);
    }
    held
}

fn request() -> String {
    support::request_with("GET", "/echo", &[], "")
}

fn connections_full_lines(server: &TestServer) -> Vec<String> {
    server
        .lines()
        .into_iter()
        .filter(|line| line.contains(r#""event":"http.connections_full""#))
        .collect()
}

/// A probe retried until it comes back refused (or served): accepting and
/// counting the held connections runs on the server's tasks, so a probe that
/// races them is retried rather than judged wrong.
#[expect(
    clippy::panic,
    reason = "test helper: a probe served forever is the failure to report"
)]
fn probe_until_refused(server: &TestServer) -> String {
    for _ in 0..250 {
        let answer = server.roundtrip(&request());
        if !answer.contains("hello") {
            return answer;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    panic!(
        "the over-cap probe was served forever: {:?}",
        server.lines()
    );
}

/// A probe retried until it is served: the slot a dropped held connection
/// frees reaches the cap's table asynchronously.
fn probe_until_served(server: &TestServer) -> Option<String> {
    for _ in 0..250 {
        let answer = server.roundtrip(&request());
        if answer.contains("hello") {
            return Some(answer);
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    None
}

#[test]
fn a_peer_over_the_cap_is_closed_at_once_and_gets_back_in_when_one_leaves() {
    let server = TestServer::start(|config| config.max_connections_per_client = 2);
    let mut held = hold_open(&server, 2);
    let answer = probe_until_refused(&server);
    assert!(
        !answer.contains("hello"),
        "the over-cap connection gets no service: {answer:?}"
    );
    let refusals = connections_full_lines(&server);
    let line = refusals
        .first()
        .unwrap_or_else(|| panic!("the refusal is logged: {:?}", server.lines()));
    assert!(
        line.contains(r#""peer":"127.0.0.1""#) && line.contains(r#""limit":2"#),
        "the line names the peer and the cap: {line}"
    );
    // One held connection dies and frees its slot; the same peer is admitted
    // again while the other one is still live.
    drop(held.pop().expect("one held connection"));
    let admitted = probe_until_served(&server);
    assert!(
        admitted.is_some(),
        "after a release the peer is served again: {:?}",
        server.lines()
    );
    server.stop();
}

#[test]
fn another_peer_is_never_capped_by_this_one() {
    // The cap counts per address: a peer at its limit must not starve a
    // client that connects from a different loopback address.
    let server = TestServer::start(|config| config.max_connections_per_client = 1);
    let held = hold_open(&server, 1);
    // A second source address on the same loopback route.
    let local: SocketAddr = "127.0.0.2:0".parse().expect("a loopback source");
    let socket = socket2::Socket::new(
        socket2::Domain::IPV4,
        socket2::Type::STREAM,
        Some(socket2::Protocol::TCP),
    )
    .expect("a client socket");
    socket.bind(&local.into()).expect("bound to 127.0.0.2");
    socket
        .connect(&server.address.into())
        .expect("the server accepts the second peer");
    let mut other: TcpStream = socket.into();
    other
        .set_read_timeout(Some(std::time::Duration::from_secs(10)))
        .expect("a read timeout");
    other
        .write_all(request().as_bytes())
        .expect("the request goes out");
    let answer = support::wire::read_until_end(&mut other);
    assert!(
        answer.contains("hello"),
        "the second peer is served while the first is at its cap: {answer}"
    );
    // The first peer, meanwhile, is refused.
    let answer = probe_until_refused(&server);
    assert!(
        !answer.contains("hello"),
        "the first peer sits at its cap: {answer:?}"
    );
    let refusals = connections_full_lines(&server);
    assert!(
        refusals
            .iter()
            .all(|line| line.contains(r#""peer":"127.0.0.1""#)),
        "only the capped address is refused: {refusals:?}"
    );
    drop(held);
    server.stop();
}

#[test]
fn a_trusted_proxy_address_is_exempt_from_the_cap() {
    // Behind a proxy every real client shares the proxy's one address;
    // counting there would lock the whole deployment out at the cap.
    let server = TestServer::start(|config| {
        config.max_connections_per_client = 2;
        config.trusted_proxies = vec!["127.0.0.0/8".to_owned()];
    });
    let held = hold_open(&server, 5);
    let probe = server.roundtrip(&request());
    assert!(
        probe.contains("hello"),
        "six connections from the proxy address are all served: {probe}"
    );
    assert!(
        connections_full_lines(&server).is_empty(),
        "the exempt peer is never refused: {:?}",
        server.lines()
    );
    drop(held);
    server.stop();
}
