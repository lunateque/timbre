//! Failures from the art pipeline.

use timbre_kernel::{Failure, FailureCode};

/// The image format is not supported.
pub const UNSUPPORTED: FailureCode = FailureCode::new("image.unsupported");
/// A configured byte or dimension limit would be exceeded.
pub const LIMIT: FailureCode = FailureCode::new("image.limit");
/// A supported image could not be decoded.
pub const DECODE: FailureCode = FailureCode::new("image.decode_failed");
/// Thumbnail generation failed after decoding.
pub const THUMBNAIL: FailureCode = FailureCode::new("image.thumbnail_failed");

/// An art operation failed.
#[derive(Debug)]
pub enum ImageFailure {
    /// Unknown format or one not enabled for art.
    Unsupported,
    /// The image is too large or the requested size is outside the limits.
    Limit,
    /// The decoder's diagnostic.
    Decode(String),
    /// The resizer's or encoder's diagnostic.
    Thumbnail(String),
}

impl Failure for ImageFailure {
    fn code(&self) -> FailureCode {
        match self {
            Self::Unsupported => UNSUPPORTED,
            Self::Limit => LIMIT,
            Self::Decode(_) => DECODE,
            Self::Thumbnail(_) => THUMBNAIL,
        }
    }

    fn status(&self) -> u16 {
        match self {
            Self::Unsupported => 415,
            Self::Limit => 413,
            Self::Decode(_) => 422,
            Self::Thumbnail(_) => 500,
        }
    }

    fn public_message(&self) -> &'static str {
        match self {
            Self::Unsupported => "Unsupported image format.",
            Self::Limit => "Image exceeds the limits.",
            Self::Decode(_) => "Image could not be decoded.",
            Self::Thumbnail(_) => "Thumbnail could not be generated.",
        }
    }
}
