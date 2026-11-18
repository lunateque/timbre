//! Public wrappers over this crate's `pub(crate)` internals, for `fuzz/`.
//!
//! The fuzz harnesses call the shipped parsers through these instead of
//! carrying copies, so a parser change cannot silently leave the fuzzing
//! behind. Every wrapper is a one-line delegation or a shape change (a
//! `HeaderMap` built from raw pairs, a source flag instead of the
//! `ParamsSource` enum); none duplicates logic. The module exists only under
//! the `fuzzing` cargo feature, which nothing in the shipped build enables,
//! so the stable surface is unchanged.

use std::net::IpAddr;
use timbre_kernel::Fallible;

/// Copy of the crate-internal [`crate::adapter::path_and_query`].
///
/// # Errors
///
/// `400` exactly as the real parser refuses a request-target.
pub fn path_and_query(target: &str) -> Result<(String, Vec<(String, String)>), u16> {
    crate::adapter::path_and_query(target)
}

/// Copy of the crate-internal [`crate::adapter::form_pairs`].
#[must_use]
pub fn form_pairs(body: &[u8]) -> Vec<(String, String)> {
    crate::adapter::form_pairs(body)
}

/// Copy of the crate-internal [`crate::adapter::is_form_post`].
#[must_use]
pub fn is_form_post(method: &str, content_type: Option<&[u8]>) -> bool {
    crate::adapter::is_form_post(method, content_type)
}

/// Copy of the crate-internal [`crate::adapter::param_usable`].
#[must_use]
pub fn param_usable(param: &str) -> bool {
    crate::adapter::param_usable(param)
}

/// The request headers as the kernel shape sees them, from raw name and
/// value byte pairs: entries the `http` crate rejects are dropped (hyper
/// never hands those over), the rest go through `adapter::header_pairs`.
pub fn header_pairs_from(raw: &[(Vec<u8>, Vec<u8>)]) -> Vec<(String, Vec<u8>)> {
    let mut map = http::HeaderMap::new();
    for (name, value) in raw {
        let (Ok(name), Ok(value)) = (
            http::HeaderName::from_bytes(name),
            http::HeaderValue::from_bytes(value),
        ) else {
            continue;
        };
        map.append(name, value);
    }
    crate::adapter::header_pairs(&map)
}

/// Copy of the crate-internal [`crate::adapter::header_bytes`].
#[must_use]
pub fn header_bytes(headers: &[(String, Vec<u8>)], name: &str) -> Option<Vec<u8>> {
    crate::adapter::header_bytes(headers, name)
}

/// Copy of the crate-internal [`crate::adapter::header_values`].
pub fn header_values<'a>(
    headers: &'a [(String, Vec<u8>)],
    name: &str,
) -> impl Iterator<Item = &'a [u8]> {
    crate::adapter::header_values(headers, name)
}

/// Copy of the crate-internal [`crate::params::refusal`], with `url_source`
/// standing in for the crate-internal `ParamsSource`.
#[must_use]
pub fn refusal(
    pairs: &[(String, String)],
    limit: usize,
    url_source: bool,
) -> Option<(u16, &'static str)> {
    let source = if url_source {
        crate::params::ParamsSource::Url
    } else {
        crate::params::ParamsSource::Form
    };
    crate::params::refusal(pairs, limit, source)
}

/// Copy of the crate-internal [`crate::params::redacted_query`].
#[must_use]
pub fn redacted_query(pairs: &[(String, String)]) -> String {
    crate::params::redacted_query(pairs)
}

/// Copy of the crate-internal [`crate::params::logged_target`].
#[must_use]
pub fn logged_target(path: &str, query: &[(String, String)]) -> String {
    crate::params::logged_target(path, query)
}

/// Copy of the crate-internal [`crate::params::redacted_target`].
#[must_use]
pub fn redacted_target(target: &str) -> String {
    crate::params::redacted_target(target)
}

/// Copy of the crate-internal [`crate::params::is_secret_key`].
#[must_use]
pub fn is_secret_key(key: &str) -> bool {
    crate::params::is_secret_key(key)
}

/// The longest key a request may carry; the crate-internal cap the
/// parameter refusals measure against.
pub const MAX_PARAM_KEY_BYTES: usize = crate::params::MAX_PARAM_KEY_BYTES;

/// Resolve the client address the way the server does for one request:
/// parse every `trusted` entry with the crate-internal
/// `proxy::Trusted::parse` (the first malformed entry aborts with
/// `http.proxy_invalid`, like `serve` does before accepting connections),
/// join the header values with `proxy::forwarded_for`, then run
/// `proxy::client_address`.
///
/// # Errors
///
/// `http.proxy_invalid` for a malformed trusted entry.
pub fn client_address_from(
    peer: IpAddr,
    forwarded_values: &[(String, Vec<u8>)],
    trusted: &[String],
) -> Fallible<IpAddr> {
    let mut proxies = Vec::new();
    for entry in trusted {
        proxies.push(crate::proxy::Trusted::parse(entry)?);
    }
    let forwarded = crate::proxy::forwarded_for(forwarded_values);
    Ok(crate::proxy::client_address(
        peer,
        forwarded.as_deref(),
        &proxies,
    ))
}

/// Parse one `trusted_proxies` entry with the crate-internal
/// `proxy::Trusted::parse`, keeping only the verdict.
///
/// # Errors
///
/// `http.proxy_invalid` for a malformed entry.
pub fn trusted_entry_accepted(entry: &str) -> Fallible {
    crate::proxy::Trusted::parse(entry).map(|_| ())
}

/// Build an [`crate::HttpConfig`] from an `[http]` document and
/// `TIMBRE_HTTP__<KEY>` overrides handed in as pairs: the same deserialization
/// `HttpConfig::from_config` runs through `Config::section`, with the process
/// environment replaced by `overrides` so a fuzzer can steer every key, and
/// the same per-key fallback to [`crate::HttpConfig::default`].
///
/// # Errors
///
/// `config.invalid` exactly as `HttpConfig::from_config` reports it.
pub fn http_section(
    document: &str,
    overrides: Vec<(String, String)>,
) -> Fallible<crate::HttpConfig> {
    let section: crate::config::HttpSection =
        timbre_edge_config::fuzzing::section(document, "http", overrides)?;
    Ok(crate::HttpConfig::from_section(section))
}
