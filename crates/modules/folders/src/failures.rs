//! The folders module's failures.

use timbre_kernel::{Failure, FailureCode};

pub(crate) const NOT_AVAILABLE: FailureCode = FailureCode::new("folders.not_available");
pub(crate) const NOT_FOUND: FailureCode = FailureCode::new("folders.not_found");
pub(crate) const INVALID_CURSOR: FailureCode = FailureCode::new("folders.invalid_cursor");
pub(crate) const PAGE_TOO_LARGE: FailureCode = FailureCode::new("folders.page_too_large");

#[derive(Debug)]
pub(crate) enum BrowseFailure {
    NotAvailable,
    NotFound,
    InvalidCursor,
    PageTooLarge,
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
            Self::InvalidCursor => INVALID_CURSOR,
            Self::PageTooLarge => PAGE_TOO_LARGE,
        }
    }

    fn status(&self) -> u16 {
        match self {
            Self::NotAvailable => 503,
            Self::NotFound => 404,
            Self::InvalidCursor | Self::PageTooLarge => 400,
        }
    }

    fn public_message(&self) -> &str {
        match self {
            Self::NotAvailable => "Folder browsing is not available on this server.",
            Self::NotFound => "The folder was not found.",
            Self::InvalidCursor => "The folder page cursor is invalid.",
            Self::PageTooLarge => "The requested page is larger than this server serves.",
        }
    }
}
