//! Startup behavior of the binary, observed through its real stdout.
//! Piped stdout is never a terminal, so every line here is JSON.
//!
//! No module is named here: which modules are wired is the generated
//! `modules.rs`'s business, and every combination of them is built and tested
//! by `cargo xtask removability`.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};

// The clippy.toml test allowance covers `#[test]` functions only; these helpers
// want the same loud failure, so the expectation is stated at each site.
fn run(log_value: &str) -> (Vec<String>, Option<i32>) {
    run_with(log_value, &[])
}

#[expect(
    clippy::expect_used,
    reason = "test helper: a missing binary must show why"
)]
fn run_with(log_value: &str, args: &[&str]) -> (Vec<String>, Option<i32>) {
    let output = Command::new(env!("CARGO_BIN_EXE_timbre"))
        .env("TIMBRE_LOG", log_value)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("the binary runs");
    let stdout = String::from_utf8(output.stdout).expect("stdout is utf-8");
    (
        stdout.lines().map(str::to_owned).collect(),
        output.status.code(),
    )
}

#[test]
fn help_lists_the_built_in_commands() {
    // `warn` hides every log line, so what stdout keeps is the list itself.
    let (lines, code) = run_with("warn", &["help"]);
    assert_eq!(code, Some(0), "`help` exits 0");
    let joined = lines.join("\n");
    let listed = |needle: &str| {
        lines
            .iter()
            .any(|line| !line.starts_with('{') && line.contains(needle))
    };
    // Only built-ins are asserted: module commands are `modules.rs`'s business.
    assert!(listed("serve"), "serve is listed: {joined}");
    assert!(listed("db check"), "db check is listed: {joined}");
    assert!(listed("db backup"), "db backup is listed: {joined}");
    assert!(listed("db prune"), "db prune is listed: {joined}");
    assert!(listed("help"), "help is listed: {joined}");
}

#[test]
fn an_unparsable_filter_exits_with_a_json_explanation() {
    let (lines, code) = run("loud");
    assert_eq!(code, Some(1));
    let line = lines
        .iter()
        .find(|line| line.contains(r#""event":"timbre.log_filter_invalid""#))
        .expect("a timbre.log_filter_invalid line");
    assert!(line.contains(r#""value":"loud""#));
    assert!(line.contains(r#""code":"kernel.log_filter_invalid""#));
}

#[test]
fn warn_hides_the_info_lines_of_a_command() {
    // `warn` must hide `timbre.started` and `http.*` info lines; here the
    // fastest full startup is a command that touches the database.
    let dir = temp_data_dir("warn");
    let output = Command::new(env!("CARGO_BIN_EXE_timbre"))
        .env("TIMBRE_LOG", "warn")
        .env("TIMBRE_SERVER__DATA_DIR", dir.path().as_os_str())
        .args(["db", "check"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("the binary runs");
    assert_eq!(
        output.status.code(),
        Some(0),
        "db check passes on a fresh database"
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout is utf-8");
    assert!(
        stdout
            .lines()
            .all(|line| !line.contains(r#""level":"info""#)),
        "no info lines at `warn`: {stdout}"
    );
}

#[test]
fn serve_logs_an_unknown_path_with_404() {
    let mut server = ServeTest::start("serve-unknown");
    let address = server.bound_address().to_owned();
    let address = address.as_str();
    // A path no module can own: every build, with or without modules, must
    // answer 404 for it and put exactly one request line in the log.
    let response = http_get(address, "/timbre-test-unknown");
    assert!(
        response.starts_with("HTTP/1.1 404 Not Found"),
        "an unknown path answers 404: {response}"
    );
    let lines = server.log_lines();
    assert!(
        lines
            .iter()
            .any(|line| line.contains(r#""event":"timbre.started""#)),
        "the server logs its start: {lines:?}"
    );
    assert!(
        lines
            .iter()
            .any(|line| line.contains(r#""event":"http.listening""#)
                && line.contains(&format!(r#""addr":"{address}""#))),
        "the bound port is logged: {lines:?}"
    );
    let requests: Vec<&String> = lines
        .iter()
        .filter(|line| line.contains(r#""event":"http.request""#))
        .collect();
    assert_eq!(requests.len(), 1, "the one request is logged: {lines:?}");
    assert!(
        requests[0].contains(r#""path":"/timbre-test-unknown""#)
            && requests[0].contains(r#""status":404"#),
        "the request line carries its path and status: {}",
        requests[0]
    );
    server.stop();
}

/// One temp data directory per test, removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn path(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[expect(
    clippy::expect_used,
    reason = "test helper: a scratch directory the run cannot do without"
)]
fn temp_data_dir(tag: &str) -> TempDir {
    static COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let unique = format!(
        "timbre-startup-{tag}-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    );
    let path = std::env::temp_dir().join(unique);
    std::fs::create_dir_all(&path).expect("the scratch directory exists");
    TempDir(path)
}

/// A `timbre serve` child writing JSON to a pipe; stdout stays open so its
/// log lines can be read back.
struct ServeTest {
    child: Child,
    reader: Option<BufReader<std::process::ChildStdout>>,
    address: String,
    startup: Vec<String>,
    _dir: TempDir,
}

impl ServeTest {
    #[expect(
        clippy::expect_used,
        reason = "test helper: a server that will not start is a failed test"
    )]
    fn start(tag: &str) -> ServeTest {
        let dir = temp_data_dir(tag);
        let mut child = Command::new(env!("CARGO_BIN_EXE_timbre"))
            .env("TIMBRE_LOG", "info")
            .env("TIMBRE_SERVER__DATA_DIR", dir.path().as_os_str())
            .env("TIMBRE_HTTP__LISTEN", r#"["127.0.0.1:0"]"#)
            .args(["serve"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("the server starts");
        let mut reader = BufReader::new(child.stdout.take().expect("stdout is piped"));
        // The bound address is only known from the `http.listening` line; the
        // earlier startup lines are kept for the assertions.
        let mut startup = Vec::new();
        let address = loop {
            let mut line = String::new();
            let flowed = reader.read_line(&mut line).expect("the log flows");
            assert_ne!(
                flowed, 0,
                "the server closed its log before reporting a bound address"
            );
            if line.contains(r#""event":"http.listening""#) {
                startup.push(line.clone());
                break field(&line, "addr");
            }
            startup.push(line);
        };
        ServeTest {
            child,
            reader: Some(reader),
            address,
            startup,
            _dir: dir,
        }
    }

    fn bound_address(&self) -> &str {
        &self.address
    }

    /// Lines logged so far, after giving the server a moment to flush the
    /// request it must have answered.
    #[expect(
        clippy::expect_used,
        reason = "test helper: the pipe exists until `stop`"
    )]
    fn log_lines(&mut self) -> Vec<String> {
        let reader = self.reader.as_mut().expect("the pipe is open");
        let mut lines = std::mem::take(&mut self.startup);
        loop {
            let mut line = String::new();
            match reader.read_line(&mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    if line.is_empty() {
                        break;
                    }
                    let done = line.contains(r#""event":"http.request""#);
                    lines.push(line);
                    if done {
                        break;
                    }
                }
            }
        }
        lines
    }

    fn stop(mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[expect(
    clippy::expect_used,
    reason = "test helper: a missing field means the line under test is wrong"
)]
fn field(line: &str, key: &str) -> String {
    let needle = format!("\"{key}\":\"");
    let start = line.find(&needle).expect("the field is logged") + needle.len();
    let rest = &line[start..];
    rest.find('"')
        .map_or_else(|| rest.to_owned(), |end| rest[..end].to_owned())
}

#[test]
fn the_request_log_never_carries_a_query_secret() {
    let mut server = ServeTest::start("serve-secrets");
    let address = server.bound_address().to_owned();
    let response = http_get(&address, "/rest/ping?u=anna&p=secret&apiKey=k");
    assert!(
        response.starts_with("HTTP/1.1"),
        "any status is fine: {response}"
    );
    let lines = server.log_lines();
    let logged = lines
        .iter()
        .find(|line| line.contains(r#""event":"http.request""#))
        .expect("the request is logged");
    assert!(
        logged.contains(r#""path":"/rest/ping?u=anna&p=***&apiKey=***""#),
        "secret values are redacted, names and order stay: {logged}"
    );
    assert!(
        !logged.contains("secret") && !logged.contains("&apiKey=k"),
        "no secret text: {logged}"
    );
    // No header value ever reaches the log, so `Authorization` cannot leak.
    for line in &lines {
        assert!(
            !line.contains("Bearer"),
            "header values stay out of the log: {line}"
        );
    }
    server.stop();
}

#[expect(
    clippy::expect_used,
    reason = "test helper: a request that cannot reach the server fails the test"
)]
fn http_get(address: &str, path: &str) -> String {
    let socket = address
        .to_socket_addrs()
        .expect("the address resolves")
        .next()
        .expect("one address");
    let mut stream = TcpStream::connect(socket).expect("the server accepts");
    stream
        .write_all(
            format!("GET {path} HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n\r\n")
                .as_bytes(),
        )
        .expect("the request goes out");
    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .expect("the response arrives");
    response
}

#[test]
fn a_malformed_server_section_stops_startup() {
    // `data_dir` must be a string; a number is not one, and the error must
    // stop the process with `config.invalid`, never fall back to defaults.
    let dir = temp_data_dir("bad-section");
    let output = Command::new(env!("CARGO_BIN_EXE_timbre"))
        .env("TIMBRE_LOG", "info")
        .env("TIMBRE_SERVER__DATA_DIR", "2024")
        .env("TIMBRE_CONFIG", dir.path().join("absent.toml").as_os_str())
        .args(["db", "check"])
        .current_dir(dir.path())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("the binary runs");
    assert_eq!(output.status.code(), Some(1), "a bad section fails startup");
    let stdout = String::from_utf8(output.stdout).expect("stdout is utf-8");
    let line = stdout
        .lines()
        .find(|line| line.contains(r#""event":"timbre.config_failed""#))
        .unwrap_or_else(|| panic!("a timbre.config_failed line in: {stdout}"));
    assert!(
        line.contains(r#""code":"config.invalid""#),
        "the code is config.invalid: {line}"
    );
    assert!(
        !line.contains("2024"),
        "the offending value is redacted: {line}"
    );
}

#[test]
fn a_config_flag_without_a_value_is_a_usage_error() {
    let (lines, code) = run_with("info", &["--config"]);
    assert_eq!(code, Some(2), "`--config` alone exits 2");
    assert!(
        lines
            .iter()
            .any(|line| line.contains(r#""event":"timbre.usage""#)
                && line.contains("needs a path")),
        "the usage error explains itself: {lines:?}"
    );
}

#[test]
fn a_non_unicode_argument_is_a_usage_error() {
    // One raw argument the platform cannot spell as UTF-8/UTF-16 text:
    // a 0xFF byte on Unix, an unpaired surrogate on Windows.
    #[cfg(unix)]
    let raw = {
        use std::os::unix::ffi::OsStringExt;
        std::ffi::OsString::from_vec(b"serve\xFF".to_vec())
    };
    #[cfg(windows)]
    let raw = {
        use std::os::windows::ffi::OsStringExt;
        std::ffi::OsString::from_wide(&[0xD800])
    };
    let output = Command::new(env!("CARGO_BIN_EXE_timbre"))
        .env("TIMBRE_LOG", "info")
        .arg(raw)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("the binary runs");
    assert_eq!(
        output.status.code(),
        Some(2),
        "an undecodable argument exits 2"
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout is utf-8");
    assert!(
        stdout.contains(r#""event":"timbre.usage""#) && stdout.contains("not valid unicode"),
        "the usage error names the argument problem: {stdout}"
    );
}
