//! SHA-256 digests and the only comparison that may be made on one.

use sha2::Digest as _;
use subtle::ConstantTimeEq as _;

/// The length of a SHA-256 digest in bytes.
pub(crate) const LENGTH: usize = 32;

/// A SHA-256 digest. There is no `PartialEq`: a digest of a secret is
/// compared with [`Digest::matches`], which takes the same time whatever the
/// first differing byte is, and `==` would quietly reintroduce the oracle.
#[derive(Clone, Copy)]
pub struct Digest([u8; LENGTH]);

impl Digest {
    /// Wrap digest bytes read back from storage.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; LENGTH]) -> Self {
        Self(bytes)
    }

    /// The digest of a stored blob, or `None` when the blob is not 32 bytes.
    #[must_use]
    pub fn from_slice(bytes: &[u8]) -> Option<Self> {
        <[u8; LENGTH]>::try_from(bytes).ok().map(Self)
    }

    /// The raw bytes, for storing.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; LENGTH] {
        &self.0
    }

    /// Whether the two digests are equal, in time independent of where they
    /// first differ.
    #[must_use]
    pub fn matches(&self, other: &Digest) -> bool {
        self.0.ct_eq(&other.0).into()
    }
}

impl std::fmt::Debug for Digest {
    /// A digest stands in for the secret it was made of, so it is masked like
    /// one: no accidental `{:?}` puts a stored digest in a log or a report.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(timbre_kernel::REDACTED)
    }
}

/// The SHA-256 of `data`.
#[must_use]
pub fn sha256(data: &[u8]) -> Digest {
    let mut bytes = [0u8; LENGTH];
    let computed = sha2::Sha256::digest(data);
    bytes.copy_from_slice(computed.as_slice());
    Digest(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_digest_is_the_published_sha_256_vector() {
        // The NIST vector for "abc"; a wrong hash function or a truncated
        // copy fails here instead of in whatever stores the digest.
        assert_eq!(
            crate::base32::encode(sha256(b"abc").as_bytes()),
            crate::base32::encode(&[
                0xba, 0x78, 0x16, 0xbf, 0x8f, 0x01, 0xcf, 0xea, 0x41, 0x41, 0x40, 0xde, 0x5d, 0xae,
                0x22, 0x23, 0xb0, 0x03, 0x61, 0xa3, 0x96, 0x17, 0x7a, 0x9c, 0xb4, 0x10, 0xff, 0x61,
                0xf2, 0x00, 0x15, 0xad,
            ])
        );
    }
}
