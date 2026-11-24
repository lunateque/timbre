//! Protocol-independent access to library cover art.

use crate::{Ctx, Fallible, Port};

/// An image body.
#[derive(Debug)]
pub enum ImageBody {
    /// Encoded image bytes.
    Bytes(Vec<u8>),
    /// An open, read-only handle on a generated cache file, positioned at
    /// its start. The handle is the proof: nothing can swap the entry for a
    /// symlink between the provider's checks and the caller's read.
    CacheFile(std::fs::File),
}

/// One encoded image and its content identity.
#[derive(Debug)]
pub struct Image {
    /// Where to read the image body.
    pub body: ImageBody,
    /// The IANA media type of the encoded image.
    pub mime: String,
    /// The hash identifying the encoded representation.
    pub etag: String,
}

/// Fetch artwork by the type-tagged, content-versioned id from a catalog value.
/// Callers should use [`Ctx::blocking`] when calling this from async code.
pub trait Artwork: Port {
    /// Fetch an original (`None`) or a thumbnail fitted to the requested long edge.
    ///
    /// # Errors
    /// Returns a failure if the artwork is absent, oversized or unreadable.
    fn image(&self, ctx: &Ctx, art_id: &str, size: Option<u32>) -> Fallible<Image>;
}
