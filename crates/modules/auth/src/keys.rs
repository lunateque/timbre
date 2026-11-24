//! The shape of a device secret: `<prefix>_<secret>`.
//!
//! The prefix is a lookup handle, not a secret: it names the row to compare
//! against, so one indexed read answers where a scan over every device would
//! otherwise be needed. The secret is 256 bits of system randomness, and the
//! server keeps only the SHA-256 of the whole string, so a stolen database
//! hands an attacker nothing to present.

use timbre_edge_crypto::{Digest, base32, random_bytes, sha256};
use timbre_kernel::{Fallible, Secret};

/// Bytes of the lookup prefix.
const PREFIX_BYTES: usize = 8;
/// Bytes of the secret itself.
const SECRET_BYTES: usize = 32;

/// A key as presented by a client, after a parse that accepts only the exact
/// spelling [`ApiKey::render`] writes.
pub struct ApiKey {
    prefix: [u8; PREFIX_BYTES],
    secret: [u8; SECRET_BYTES],
}

impl std::fmt::Debug for ApiKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ApiKey(***)")
    }
}

impl ApiKey {
    /// Parse a presented key, `None` for anything that is not exactly one
    /// canonical prefix, one `_` and one canonical secret of the right
    /// lengths. Strictness is what makes the prefix usable as a lookup key:
    /// no second spelling of the same key can exist.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let (prefix, secret) = text.split_once('_')?;
        let prefix = base32::decode(prefix).ok()?;
        let secret = base32::decode(secret).ok()?;
        Some(Self {
            prefix: <[u8; PREFIX_BYTES]>::try_from(prefix.as_slice()).ok()?,
            secret: <[u8; SECRET_BYTES]>::try_from(secret.as_slice()).ok()?,
        })
    }

    /// The text a client presents for this key.
    #[must_use]
    pub fn render(&self) -> String {
        format!(
            "{}_{}",
            base32::encode(&self.prefix),
            base32::encode(&self.secret)
        )
    }

    /// The lookup handle, as stored in the `prefix` column.
    pub(crate) fn prefix(&self) -> String {
        base32::encode(&self.prefix)
    }
}

/// A freshly generated key: the text to hand over once, its lookup prefix
/// and the digest that is all the server keeps.
pub(crate) struct Issued {
    pub(crate) prefix: String,
    pub(crate) digest: Digest,
    pub(crate) secret: Secret<String>,
}

/// Draw a new key from the system generator.
pub(crate) fn issue() -> Fallible<Issued> {
    let key = ApiKey {
        prefix: random_bytes()?,
        secret: random_bytes()?,
    };
    let text = key.render();
    Ok(Issued {
        prefix: key.prefix(),
        digest: sha256(text.as_bytes()),
        secret: Secret::new(text),
    })
}

/// A new opaque device id. Ids are drawn, not derived from the key, so
/// reissuing a secret leaves the id (and every reference to it) alone.
pub(crate) fn device_id() -> Fallible<String> {
    let bytes: [u8; PREFIX_BYTES] = random_bytes()?;
    Ok(base32::encode(&bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_never_prints_the_secret_bytes() {
        let key = ApiKey {
            prefix: [0; PREFIX_BYTES],
            secret: [157; SECRET_BYTES],
        };
        let shown = format!("{key:?}");
        assert!(shown.contains("***"), "Debug marks the secret as redacted");
        assert!(!shown.contains("157"), "Debug may not print raw bytes");
    }

    #[test]
    fn an_issued_key_parses_back_to_itself() {
        let issued = issue().expect("the system generator answers");
        let text = issued.secret.expose().clone();
        let parsed = ApiKey::parse(&text).expect("an issued key parses");
        assert_eq!(parsed.render(), text);
        assert_eq!(parsed.prefix(), issued.prefix);
        assert!(sha256(text.as_bytes()).matches(&issued.digest));
    }

    #[test]
    fn every_malformed_shape_is_refused() {
        let text = issue()
            .expect("the system generator answers")
            .secret
            .expose()
            .clone();
        let (prefix, secret) = text.split_once('_').expect("the key has one underscore");
        let mut uppercased = secret.to_owned();
        uppercased.replace_range(0..1, "A");
        let mut foreign = secret.to_owned();
        foreign.replace_range(0..1, "1");
        for broken in [
            String::new(),
            prefix.to_owned(),
            secret.to_owned(),
            format!("{prefix}_"),
            format!("_{secret}"),
            format!("{prefix}_{secret}_{secret}"),
            format!("{prefix}{secret}"),
            format!("{prefix}_{secret}a"),
            format!("{prefix}a_{secret}"),
            format!("{prefix}_{uppercased}"),
            format!("{prefix}_{foreign}"),
        ] {
            assert!(ApiKey::parse(&broken).is_none(), "accepted {broken:?}");
        }
    }
}
