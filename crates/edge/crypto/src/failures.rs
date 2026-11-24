//! The crypto edge's failures.

use timbre_kernel::{Failure, FailureCode};

/// The operating system refused to hand out random bytes.
pub const RANDOM_FAILED: FailureCode = FailureCode::new("crypto.random_failed");
/// Encoded text is not what the strict decoder accepts.
pub const ENCODING_INVALID: FailureCode = FailureCode::new("crypto.encoding_invalid");

/// What this edge can fail with.
#[derive(Debug)]
pub enum CryptoFailure {
    /// The system random generator was unavailable.
    RandomFailed,
    /// The text is not canonical base32 of the expected shape.
    EncodingInvalid,
}

impl CryptoFailure {
    /// The boxed form every caller returns.
    #[must_use]
    pub fn boxed(self) -> Box<dyn Failure> {
        Box::new(self)
    }
}

impl Failure for CryptoFailure {
    fn code(&self) -> FailureCode {
        match self {
            Self::RandomFailed => RANDOM_FAILED,
            Self::EncodingInvalid => ENCODING_INVALID,
        }
    }

    fn status(&self) -> u16 {
        match self {
            Self::RandomFailed => 500,
            Self::EncodingInvalid => 400,
        }
    }

    fn public_message(&self) -> &str {
        match self {
            Self::RandomFailed => "The server could not generate a secret.",
            Self::EncodingInvalid => "The value is not in the expected encoding.",
        }
    }
}
