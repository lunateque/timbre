//! Native tag writers for preparing test files; not part of the production API.

use std::path::Path;

pub use lofty::TextEncoding;
pub use lofty::ape::{ApeItem, ApeTag};
pub use lofty::config::WriteOptions;
pub use lofty::id3::v1::Id3v1Tag;
pub use lofty::id3::v2::{
    AttachedPictureFrame, BinaryFrame, Frame, FrameId, Id3v2Tag, KeyValueFrame,
    SyncTextContentType, SynchronizedTextFrame, TextInformationFrame, TimestampFormat,
    TimestampFrame, UniqueFileIdentifierFrame, UnsynchronizedTextFrame,
};
pub use lofty::iff::wav::RiffInfoList;
pub use lofty::mp4::{Atom, AtomData, AtomIdent, DataType, Ilst};
pub use lofty::ogg::OggPictureStorage;
pub use lofty::ogg::tag::VorbisComments;
pub use lofty::picture::{MimeType, Picture, PictureInformation, PictureType};
pub use lofty::tag::items::Timestamp;
pub use lofty::tag::{Accessor, ItemValue as ApeValue, Tag, TagExt, TagType};
use timbre_kernel::Fallible;

/// Write a native fixture tag to an existing audio file.
///
/// # Errors
/// Returns a failure if the tag cannot be written to this file.
pub fn write(path: &Path, tag: &impl TagExt, options: WriteOptions) -> Fallible {
    tag.save_to_path(path, options)
        .map_err(crate::failures::invalid)
}
