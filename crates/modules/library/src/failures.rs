//! The library module's failures.

use timbre_kernel::{Failure, FailureCode};

/// A scanned catalog item is not present.
pub const NOT_FOUND: FailureCode = FailureCode::new("library.not_found");
/// A configured root cannot be scanned safely.
pub const ROOT_INVALID: FailureCode = FailureCode::new("library.root_invalid");
/// A scan is already running.
pub const SCAN_RUNNING: FailureCode = FailureCode::new("library.scan_running");
/// Artwork exceeds a decode or resize bound.
pub const IMAGE_TOO_LARGE: FailureCode = FailureCode::new("library.image_too_large");
/// Artwork could not be decoded or re-encoded.
pub const IMAGE_UNREADABLE: FailureCode = FailureCode::new("library.image_unreadable");
/// The host did not supply a library runtime resource.
pub const NOT_CONFIGURED: FailureCode = FailureCode::new("library.not_configured");
/// The caller may not initiate a scan.
pub const FORBIDDEN: FailureCode = FailureCode::new("library.forbidden");
/// A file or working thread could not be opened.
pub const IO_FAILED: FailureCode = FailureCode::new("library.io_failed");
/// A command or paging argument was invalid.
pub const ARGUMENT_INVALID: FailureCode = FailureCode::new("library.argument_invalid");

pub(crate) enum LibraryFailure {
    NotFound,
    RootInvalid(String),
    ScanRunning,
    ImageTooLarge,
    ImageUnreadable,
    NotConfigured,
    Forbidden,
    Io(std::io::Error),
    Argument,
}

impl std::fmt::Debug for LibraryFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RootInvalid(reason) => f.debug_tuple("RootInvalid").field(reason).finish(),
            Self::Io(error) => f.debug_tuple("Io").field(error).finish(),
            _ => f.write_str(self.code().as_str()),
        }
    }
}

impl Failure for LibraryFailure {
    fn code(&self) -> FailureCode {
        match self {
            Self::NotFound => NOT_FOUND,
            Self::RootInvalid(_) => ROOT_INVALID,
            Self::ScanRunning => SCAN_RUNNING,
            Self::ImageTooLarge => IMAGE_TOO_LARGE,
            Self::ImageUnreadable => IMAGE_UNREADABLE,
            Self::NotConfigured => NOT_CONFIGURED,
            Self::Forbidden => FORBIDDEN,
            Self::Io(_) => IO_FAILED,
            Self::Argument => ARGUMENT_INVALID,
        }
    }

    fn status(&self) -> u16 {
        match self {
            Self::NotFound => 404,
            Self::ScanRunning => 409,
            Self::ImageTooLarge => 413,
            Self::Forbidden => 403,
            Self::Argument => 400,
            Self::NotConfigured | Self::RootInvalid(_) => 503,
            Self::Io(_) | Self::ImageUnreadable => 500,
        }
    }

    fn public_message(&self) -> &str {
        match self {
            Self::NotFound => "Library item not found.",
            Self::RootInvalid(_) => "A library root could not be scanned.",
            Self::ScanRunning => "A library scan is already running.",
            Self::ImageTooLarge => "The artwork is too large.",
            Self::ImageUnreadable => "The artwork could not be read.",
            Self::NotConfigured => "The library is not configured.",
            Self::Forbidden => "You do not have permission to scan the library.",
            Self::Io(_) => "Library I/O failed.",
            Self::Argument => "Invalid library argument.",
        }
    }
}

impl LibraryFailure {
    pub(crate) fn boxed(self) -> Box<dyn Failure> {
        Box::new(self)
    }
}

pub(crate) fn io(error: std::io::Error) -> Box<dyn Failure> {
    LibraryFailure::Io(error).boxed()
}
