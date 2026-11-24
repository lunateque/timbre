//! Failure codes returned by the tags edge.

use timbre_kernel::{Failure, FailureCode};

/// The file could not be opened or read.
pub const IO: FailureCode = FailureCode::new("tags.io");
/// The file is outside the supported containers or codecs.
pub const UNSUPPORTED: FailureCode = FailureCode::new("tags.unsupported");
/// The file has invalid metadata or audio boundaries.
pub const INVALID: FailureCode = FailureCode::new("tags.invalid");

#[derive(Debug)]
pub(crate) enum TagFailure {
    Io(std::io::Error),
    Unsupported,
    Invalid(String),
}

impl Failure for TagFailure {
    fn code(&self) -> FailureCode {
        match self {
            Self::Io(_) => IO,
            Self::Unsupported => UNSUPPORTED,
            Self::Invalid(_) => INVALID,
        }
    }

    fn status(&self) -> u16 {
        match self {
            Self::Io(_) => 500,
            Self::Unsupported => 415,
            Self::Invalid(_) => 422,
        }
    }

    fn public_message(&self) -> &str {
        match self {
            Self::Io(error) => {
                let _ = error.kind();
                "The audio file could not be read."
            }
            Self::Unsupported => "The audio format is not supported.",
            Self::Invalid(detail) => {
                let _ = detail.len();
                "The audio file or its tags are invalid."
            }
        }
    }
}

pub(crate) fn io(error: std::io::Error) -> Box<dyn Failure> {
    if matches!(
        error.kind(),
        std::io::ErrorKind::UnexpectedEof | std::io::ErrorKind::InvalidData
    ) {
        invalid(error)
    } else {
        Box::new(TagFailure::Io(error))
    }
}

pub(crate) fn invalid(
    message: impl Into<Box<dyn std::error::Error + Send + Sync>>,
) -> Box<dyn Failure> {
    Box::new(TagFailure::Invalid(message.into().to_string()))
}
