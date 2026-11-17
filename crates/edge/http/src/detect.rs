//! Protocol detection with a deadline: which protocol a connection speaks
//! is known only when its first bytes arrive, and an unbounded wait there
//! lets a silent client hold a connection slot forever.

use std::time::Duration;
use tokio::net::TcpStream;

/// The HTTP/2 connection preface. A connection speaks HTTP/2 exactly while
/// its first bytes are a prefix of this, and HTTP/1 as soon as they differ;
/// the detection reads at most this many bytes.
const H2_PREFACE: &[u8] = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";

/// What a connection's first bytes say it speaks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Detected {
    Http1,
    Http2,
}

/// The first pause between peeks of an undecided connection.
const PARK_START: Duration = Duration::from_millis(5);
/// The longest pause: a preface completing during a park is seen this late
/// at worst, negligible against the header-read budget detection runs in.
const PARK_MAX: Duration = Duration::from_millis(100);

// Peeks made by `detect` on this thread; a test counts them to prove the
// loop parks between looks instead of spinning on unread bytes.
#[cfg(test)]
thread_local! {
    pub(crate) static PEEKS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Read (by peeking, so the bytes stay for hyper) until the first bytes
/// decide the protocol, `None` when the client goes silent for `budget`,
/// hangs up, or the socket fails: `None` means close the connection and
/// release its slot. The budget bounds the whole detection phase, not each
/// individual read, so many dribbled 1-byte writes still cannot extend it.
/// While what arrived is only a prefix of the preface the loop sleeps
/// between peeks, doubling up to [`PARK_MAX`]: unread bytes keep `peek`
/// (and `stream.readable()`) immediately ready, so an unparked wait would
/// spin at full CPU until the deadline.
pub(crate) async fn detect(stream: &mut TcpStream, budget: Duration) -> Option<Detected> {
    let deadline = tokio::time::Instant::now().checked_add(budget)?;
    let mut buf = [0u8; 24];
    let mut park = PARK_START;
    loop {
        let now = tokio::time::Instant::now();
        let room = deadline.saturating_duration_since(now);
        if room.is_zero() {
            return None;
        }
        let peeked = match tokio::time::timeout(room, stream.peek(&mut buf)).await {
            Err(_elapsed) => return None,
            // An IO failure, or EOF before a protocol decided itself:
            // nothing to serve.
            Ok(Err(_) | Ok(0)) => return None,
            Ok(Ok(count)) => count,
        };
        #[cfg(test)]
        PEEKS.with(|peeks| peeks.set(peeks.get() + 1));
        let (got, want) = (buf.get(..peeked)?, H2_PREFACE.get(..peeked)?);
        if got != want {
            return Some(Detected::Http1);
        }
        if peeked == H2_PREFACE.len() {
            return Some(Detected::Http2);
        }
        // Still a prefix, still short: the peer may send the rest, and the
        // one deadline keeps waiting to a hard stop, parked between peeks.
        tokio::time::sleep(park.min(room)).await;
        park = (park * 2).min(PARK_MAX);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Drive `detect` on a real socket pair from a blocking thread with its
    /// own current-thread runtime; `act` runs against the client half.
    fn on_socket_pair(
        budget: Duration,
        act: impl FnOnce(&mut std::net::TcpStream) + Send + 'static,
    ) -> (Option<Detected>, usize) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a loopback port");
        let address = listener.local_addr().expect("a bound address");
        let mut client = std::net::TcpStream::connect(address).expect("a connected client");
        let (accepted, _) = listener.accept().expect("the server side accepts");
        drop(listener);
        let thread = std::thread::spawn(move || act(&mut client));
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("a runtime for the test");
        PEEKS.with(|peeks| peeks.set(0));
        let outcome = runtime.block_on(async {
            accepted.set_nonblocking(true).expect("nonblocking");
            let mut stream = TcpStream::from_std(accepted).expect("a tokio socket");
            detect(&mut stream, budget).await
        });
        let peeks = PEEKS.with(std::cell::Cell::get);
        thread.join().expect("the client half runs");
        (outcome, peeks)
    }

    #[test]
    fn the_h2_preface_is_detected_and_left_on_the_socket() {
        let (outcome, _) = on_socket_pair(Duration::from_secs(5), |client| {
            client.write_all(H2_PREFACE).expect("the preface goes out");
        });
        assert_eq!(
            outcome,
            Some(Detected::Http2),
            "the full preface decides h2"
        );
    }

    #[test]
    fn anything_not_the_preface_is_http1() {
        for probe in [b"GET / HTTP/1.1\r\n".to_vec(), b"PRI * HTTP/1.1".to_vec()] {
            let shown = String::from_utf8_lossy(&probe).into_owned();
            let (outcome, _) = on_socket_pair(Duration::from_secs(5), move |client| {
                client.write_all(&probe).expect("the probe goes out");
            });
            assert_eq!(outcome, Some(Detected::Http1), "{shown:?} decides h1");
        }
    }

    #[test]
    fn a_partial_preface_waits_until_the_deadline() {
        // One byte is still compatible with the h2 preface: detection cannot
        // decide and must release the connection at the budget, not earlier
        // and not later.
        let started = std::time::Instant::now();
        let (outcome, peeks) = on_socket_pair(Duration::from_millis(100), |client| {
            client.write_all(b"P").expect("the byte goes out");
            std::thread::sleep(Duration::from_millis(300));
        });
        assert_eq!(outcome, None, "a dribbled preface ends at the deadline");
        assert!(started.elapsed() >= Duration::from_millis(90));
        assert!(started.elapsed() < Duration::from_secs(5));
        // The socket holds a byte the whole time, so `peek` never yields
        // `WouldBlock`: an unparked loop would look thousands of times in
        // 100 ms. A backoff that at most doubles from 5 ms to 100 ms needs
        // well under ten peeks to burn the same budget.
        assert!(peeks < 20, "the loop parked between peeks, made {peeks}");
    }

    #[test]
    fn a_silent_connection_times_out() {
        let started = std::time::Instant::now();
        let (outcome, _) = on_socket_pair(Duration::from_millis(100), |_client| {
            std::thread::sleep(Duration::from_millis(300));
        });
        assert_eq!(outcome, None, "silence releases the connection");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "and within the budget"
        );
    }

    #[test]
    fn a_hung_up_connection_ends_the_phase_at_once() {
        let started = std::time::Instant::now();
        let (outcome, _) = on_socket_pair(Duration::from_secs(5), |client| {
            let _ = client.shutdown(std::net::Shutdown::Both);
        });
        assert_eq!(outcome, None, "EOF with no bytes closes the connection");
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "and ends the phase without waiting for the budget"
        );
    }
}
