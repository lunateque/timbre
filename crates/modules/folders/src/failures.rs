//! The folders module's failures.

use timbre_kernel::{Failure, FailureCode};

pub(crate) const NOT_AVAILABLE: FailureCode = FailureCode::new("folders.not_available");
pub(crate) const NOT_FOUND: FailureCode = FailureCode::new("folders.not_found");

#[derive(Debug)]
pub(crate) enum BrowseFailure {
    NotAvailable,
    NotFound,
}

impl BrowseFailure {
    pub(crate) fn boxed(self) -> Box<dyn Failure> {
        Box::new(self)
    }
}

impl Failure for BrowseFailure {
    fn code(&self) -> FailureCode {
        match self {
            Self::NotAvailable => NOT_AVAILABLE,
            Self::NotFound => NOT_FOUND,
        }
    }

    fn status(&self) -> u16 {
        match self {
            Self::NotAvailable => 503,
            Self::NotFound => 404,
        }
    }

    fn public_message(&self) -> &str {
        match self {
            Self::NotAvailable => "Folder browsing is not available on this server.",
            Self::NotFound => "The folder was not found.",
        }
    }
}
