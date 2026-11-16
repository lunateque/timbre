//! What the HTTP edge listens on and how much it accepts.

use std::time::Duration;

/// The configuration `serve` runs with, built by the binary from `[http]`.
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
}

impl Default for HttpConfig {
    fn default() -> Self {
        HttpConfig {
            listen: vec!["127.0.0.1:4533".to_owned()],
            trusted_proxies: Vec::new(),
            max_body_bytes: 1024 * 1024,
            body_read_timeout: Duration::from_secs(30),
        }
    }
}
