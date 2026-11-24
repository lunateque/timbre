//! Random bytes from the operating system.

use crate::failures::CryptoFailure;
use timbre_kernel::Fallible;

/// `N` bytes from the operating system's generator.
///
/// # Errors
///
/// `crypto.random_failed` when the system generator is unavailable; a secret
/// is never built from a fallback, so the caller must fail instead.
pub fn random_bytes<const N: usize>() -> Fallible<[u8; N]> {
    let mut bytes = [0u8; N];
    getrandom::fill(&mut bytes).map_err(|_| CryptoFailure::RandomFailed.boxed())?;
    Ok(bytes)
}
