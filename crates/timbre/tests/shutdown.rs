//! Shutdown is the same whether serving ended cleanly or with a failure.

use std::io::Read;
use std::net::TcpListener;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// Long enough for a bind failure and the drain that follows it, short
/// enough that a hung child fails the test instead of the suite.
const PATIENCE: Duration = Duration::from_secs(30);

#[test]
fn a_serve_that_cannot_bind_still_drains_before_it_exits() {
    // A listener held open for the whole run: the server's own bind of the
    // same address fails deterministically on every platform, after the
    // kernel started and before anything is served.
    let taken = TcpListener::bind("127.0.0.1:0").expect("a port to hold");
    let address = taken.local_addr().expect("the held address");
    let dir = std::env::temp_dir().join(format!("timbre-shutdown-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut child = Command::new(env!("CARGO_BIN_EXE_timbre"))
        .env("TIMBRE_LOG", "info")
        .env("TIMBRE_SERVER__DATA_DIR", dir.as_os_str())
        .env("TIMBRE_HTTP__LISTEN", format!(r#"["{address}"]"#))
        .args(["serve"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("the binary runs");
    let code = wait_for(&mut child);
    let mut stdout = String::new();
    child
        .stdout
        .take()
        .expect("stdout is piped")
        .read_to_string(&mut stdout)
        .expect("stdout is utf-8");
    drop(taken);
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(code, Some(1), "a bind failure exits 1: {stdout}");
    assert!(
        stdout.contains(r#""code":"http.bind_failed""#),
        "the serve failed on the bind, not on something else: {stdout}"
    );
    // `timbre.stopped` is logged beside the `ServerDrained` publish, so its
    // absence means the journal never got its final pass either.
    assert!(
        stdout.contains(r#""event":"timbre.stopped""#),
        "the failed serve still drained: {stdout}"
    );
}

/// The child's exit code, or `None` after it was killed for taking longer
/// than `PATIENCE`.
#[expect(
    clippy::expect_used,
    reason = "test helper: a child that cannot be waited on is a failed test"
)]
fn wait_for(child: &mut Child) -> Option<i32> {
    let deadline = Instant::now() + PATIENCE;
    loop {
        match child.try_wait().expect("the child is waitable") {
            Some(status) => return status.code(),
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            None => std::thread::sleep(Duration::from_millis(25)),
        }
    }
}
