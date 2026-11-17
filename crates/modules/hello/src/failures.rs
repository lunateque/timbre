//! The hello module's failures.

use timbre_kernel::{Failure, FailureCode};

/// Hello cannot answer.
pub const UNAVAILABLE: FailureCode = FailureCode::new("hello.unavailable");

/// The guard passed a request without a principal: the kernel's guard flow
/// is broken, so hello refuses to greet anyone rather than guess a name.
pub const PRINCIPAL_MISSING: FailureCode = FailureCode::new("hello.principal_missing");

/// Failures raised by the hello module.
#[derive(Debug)]
pub enum HelloFailure {
    /// Hello is not able to greet right now.
    Unavailable,
    /// A guarded route ran without the principal its guard establishes.
    PrincipalMissing,
}

impl Failure for HelloFailure {
    fn code(&self) -> FailureCode {
        match self {
            Self::Unavailable => UNAVAILABLE,
            Self::PrincipalMissing => PRINCIPAL_MISSING,
        }
    }

    fn status(&self) -> u16 {
        match self {
            Self::Unavailable => 503,
            Self::PrincipalMissing => 500,
        }
    }

    fn public_message(&self) -> &'static str {
        match self {
            Self::Unavailable => "Hello is unavailable.",
            Self::PrincipalMissing => "Hello could not tell who is asking.",
        }
    }
}
