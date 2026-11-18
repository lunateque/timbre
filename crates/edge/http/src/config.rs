//! What the HTTP edge listens on and how much it accepts, and the `[http]`
//! config section that builds it.

use std::time::Duration;
use timbre_kernel::Fallible;

/// The configuration `serve` runs with, built from `[http]` by
/// [`HttpConfig::from_config`].
#[derive(Clone, Debug)]
pub struct HttpConfig {
    /// The addresses to bind, e.g. `["127.0.0.1:4533"]`.
    pub listen: Vec<String>,
    /// Addresses or CIDR prefixes whose `X-Forwarded-For` is believed.
    pub trusted_proxies: Vec<String>,
    /// The largest request body accepted; larger answers 413.
    pub max_body_bytes: usize,
    /// How long a client may take to send its whole request body; a body
    /// arriving slower than this answers 408.
    pub body_read_timeout: Duration,
    /// How long a connection may stay silent while the server decides
    /// HTTP/1 or HTTP/2 from its first bytes, and how long the client may
    /// take to send its request headers; both bound answers close the
    /// connection and release its slot.
    pub header_read_timeout: Duration,
    /// How long a route handler may take to produce its response; slower is
    /// answered 504. A streaming response body is not bounded by it, so a
    /// long download keeps running past the deadline.
    pub handler_timeout: Duration,
    /// How long a route's guard check (credential extraction, provider
    /// authentication, permission test) may take; slower is answered 504 and
    /// logged as `http.guard_timeout`.
    pub guard_timeout: Duration,
    /// The largest number of query+form pairs one request may carry; more
    /// answers 414 (URL), 413 (form).
    pub max_param_pairs: usize,
}

impl Default for HttpConfig {
    fn default() -> Self {
        HttpConfig {
            listen: vec!["127.0.0.1:4533".to_owned()],
            trusted_proxies: Vec::new(),
            max_body_bytes: 1024 * 1024,
            body_read_timeout: Duration::from_secs(30),
            header_read_timeout: Duration::from_secs(10),
            handler_timeout: Duration::from_secs(60),
            guard_timeout: Duration::from_secs(10),
            max_param_pairs: 1024,
        }
    }
}

/// The `[http]` section, owned by this edge: absent keys keep the
/// [`HttpConfig::default`] values, timeouts are milliseconds.
#[derive(timbre_edge_config::serde::Deserialize, Default)]
#[serde(crate = "timbre_edge_config::serde", deny_unknown_fields)]
pub(crate) struct HttpSection {
    /// Listen addresses.
    pub listen: Option<Vec<String>>,
    /// Trusted proxy entries.
    pub trusted_proxies: Option<Vec<String>>,
    /// The body size cap.
    pub max_body_bytes: Option<usize>,
    /// The header-read (and protocol-detection) timeout, in milliseconds.
    pub header_read_timeout_ms: Option<u64>,
    /// The handler deadline, in milliseconds.
    pub handler_timeout_ms: Option<u64>,
    /// The guard-check deadline, in milliseconds.
    pub guard_timeout_ms: Option<u64>,
    /// The body-read timeout, in milliseconds.
    pub body_read_timeout_ms: Option<u64>,
    /// The cap of query+form pairs.
    pub max_param_pairs: Option<usize>,
}

impl HttpConfig {
    /// Build the server configuration from the `[http]` section of
    /// `config`: `TIMBRE_HTTP__<KEY>` environment overrides apply over the
    /// file over the defaults.
    ///
    /// # Errors
    ///
    /// `config.invalid` when the section holds a key it does not know or a
    /// value of the wrong type.
    pub fn from_config(config: &timbre_edge_config::Config) -> Fallible<HttpConfig> {
        let section: HttpSection = config.section("http")?;
        Ok(Self::from_section(section))
    }

    /// The merge the `[http]` section ends in: every absent key keeps its
    /// [`HttpConfig::default`] value.
    pub(crate) fn from_section(section: HttpSection) -> HttpConfig {
        let defaults = HttpConfig::default();
        HttpConfig {
            listen: section.listen.unwrap_or(defaults.listen),
            trusted_proxies: section.trusted_proxies.unwrap_or(defaults.trusted_proxies),
            max_body_bytes: section.max_body_bytes.unwrap_or(defaults.max_body_bytes),
            body_read_timeout: milliseconds(
                section.body_read_timeout_ms,
                defaults.body_read_timeout,
            ),
            header_read_timeout: milliseconds(
                section.header_read_timeout_ms,
                defaults.header_read_timeout,
            ),
            handler_timeout: milliseconds(section.handler_timeout_ms, defaults.handler_timeout),
            guard_timeout: milliseconds(section.guard_timeout_ms, defaults.guard_timeout),
            max_param_pairs: section.max_param_pairs.unwrap_or(defaults.max_param_pairs),
        }
    }
}

/// The configured timeout in milliseconds, `fallback` when the key is absent.
fn milliseconds(millis: Option<u64>, fallback: Duration) -> Duration {
    millis.map_or(fallback, Duration::from_millis)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// A config document written to a process-unique scratch file.
    struct File(std::path::PathBuf);

    impl File {
        fn with(content: &str) -> File {
            static COUNTER: AtomicU32 = AtomicU32::new(0);
            let unique = format!(
                "timbre-http-config-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::SeqCst)
            );
            let path = std::env::temp_dir().join(unique);
            std::fs::create_dir_all(&path).expect("the scratch directory exists");
            let file = path.join("timbre.toml");
            std::fs::write(&file, content).expect("the config file is written");
            File(file)
        }

        fn config(&self) -> timbre_edge_config::Config {
            timbre_edge_config::Config::load(Some(self.0.as_path())).expect("the file parses")
        }
    }

    impl Drop for File {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_timeout_from_the_file_reaches_the_config() {
        let file = File::with(
            "[http]\nhandler_timeout_ms = 250\nguard_timeout_ms = 40\nmax_param_pairs = 7\n",
        );
        let http = HttpConfig::from_config(&file.config()).expect("the section parses");
        assert_eq!(http.handler_timeout, Duration::from_millis(250));
        assert_eq!(http.guard_timeout, Duration::from_millis(40));
        assert_eq!(http.max_param_pairs, 7);
        assert_eq!(
            http.body_read_timeout,
            HttpConfig::default().body_read_timeout,
            "absent keys keep their defaults"
        );
        assert_eq!(http.listen, HttpConfig::default().listen);
    }

    #[test]
    fn the_guard_deadline_defaults_to_ten_seconds() {
        assert_eq!(
            HttpConfig::default().guard_timeout,
            Duration::from_secs(10),
            "a guard that hangs must answer 504, not hold the request open"
        );
    }

    #[test]
    fn an_unknown_key_in_the_section_is_invalid() {
        let file = File::with("[http]\nhandler_timeout = 250\n");
        let error = HttpConfig::from_config(&file.config())
            .expect_err("a misspelled key is not silently dropped");
        assert_eq!(error.code().as_str(), "config.invalid");
    }
}
