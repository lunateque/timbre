//! The crypto edge: random secrets, SHA-256 digests, constant-time
//! comparison and the one text encoding secrets are written in.
//!
//! Nothing here knows what a secret means. [`random_bytes`] draws from the
//! operating system, [`sha256`] hashes, [`Digest::matches`] compares without
//! leaking where two digests differ, and [`base32`] turns bytes into the
//! lowercase text a secret travels as (and strictly back).

#![warn(missing_docs)]

pub mod base32;
mod digest;
pub mod failures;
mod random;

pub use digest::{Digest, sha256};
pub use random::random_bytes;
