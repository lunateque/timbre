//! Native audio tags, stream properties and metadata-independent fingerprints.
#![warn(missing_docs)]

mod embedded;
pub mod failures;
mod fields;
mod guard;
mod limits;
mod metadata;
mod model;
mod mp3;
mod native;
mod ogg;
mod payload;
mod picture_range;

#[cfg(feature = "fixtures")]
pub mod fixtures;

use std::fs::File;
use std::io::{BufReader, Read, Seek};
use std::ops::Range;
use std::path::Path;

use lofty::file::FileType;
use lofty::probe::Probe;
use timbre_kernel::Fallible;

use failures::{TagFailure, io};
pub use fields::Fields;
pub use model::{
    Advisory, AudioPayload, Codec, Container, DurationSource, FileTags, Fingerprint, Lyric,
    LyricBody, LyricTime, Performer, Picture, Properties, ReplayGain, TextValues,
};

/// Read audio properties, native tag fields and audio payload bounds.
///
/// # Errors
/// Returns a failure if the file cannot be read, is unsupported or is malformed.
pub fn read(path: &Path) -> Fallible<FileTags> {
    let mut reader = BufReader::new(File::open(path).map_err(io)?);
    read_from(&mut reader, FileType::from_path(path))
}

/// Find the encoded bytes of a contiguous picture obtained from these tags.
/// Page-spanning, encoded, and unsynchronized tags may require [`read_picture`].
///
/// # Errors
/// Returns a failure if metadata bytes cannot be read.
pub fn picture_range(path: &Path, tags: &FileTags, bytes: &[u8]) -> Fallible<Option<Range<u64>>> {
    let mut reader = BufReader::new(File::open(path).map_err(io)?);
    picture_range::find(&mut reader, tags, bytes).map_err(io)
}

/// Read one picture in the same order as [`read`], without measuring audio.
/// Only the requested picture keeps its bytes.
///
/// # Errors
/// Returns a failure if the file is unsupported, unreadable, or malformed.
pub fn read_picture(path: &Path, index: usize) -> Fallible<Option<Picture>> {
    let mut reader = BufReader::new(File::open(path).map_err(io)?);
    let kind = file_type(&mut reader, FileType::from_path(path))?;
    native::pictures(&mut reader, kind, index).map(|pictures| {
        pictures
            .into_iter()
            .nth(index)
            .filter(|p| !p.bytes.is_empty())
    })
}

fn read_from(reader: &mut (impl Read + Seek), hint: Option<FileType>) -> Fallible<FileTags> {
    let kind = file_type(reader, hint)?;
    native::read(reader, kind)
}

fn file_type(reader: &mut (impl Read + Seek), hint: Option<FileType>) -> Fallible<FileType> {
    Probe::new(&mut *reader)
        .guess_file_type()
        .map_err(io)?
        .file_type()
        .or(hint)
        .ok_or_else(|| Box::new(TagFailure::Unsupported) as Box<dyn timbre_kernel::Failure>)
}

/// Hash the logical audio payload of the same file from which `tags` were read.
///
/// # Errors
/// Returns a failure if the file cannot be reopened or its audio bounds cannot be read.
pub fn fingerprint(path: &Path, tags: &FileTags) -> Fallible<Fingerprint> {
    let mut reader = BufReader::new(File::open(path).map_err(io)?);
    fingerprint_from(&mut reader, tags)
}

fn fingerprint_from(reader: &mut (impl Read + Seek), tags: &FileTags) -> Fallible<Fingerprint> {
    let (payload_size, hash) = tags.payload.0.hash(reader).map_err(io)?;
    Ok(Fingerprint {
        payload_size,
        duration: tags.properties.duration,
        hash,
    })
}

/// In-memory entry points for the fuzz harness using the production parsers.
#[cfg(feature = "fuzzing")]
pub mod fuzzing {
    use std::io::Cursor;

    use lofty::file::FileType;
    use timbre_kernel::Fallible;

    use crate::{FileTags, Fingerprint};

    /// Read audio and metadata from bytes with the given file extension.
    ///
    /// # Errors
    /// Fails for unsupported or invalid audio data.
    pub fn read(extension: &str, bytes: &[u8]) -> Fallible<FileTags> {
        super::read_from(&mut Cursor::new(bytes), FileType::from_ext(extension))
    }

    /// Fingerprint the in-memory input using the extracted audio layout.
    ///
    /// # Errors
    /// Fails if the referenced audio data cannot be read.
    pub fn fingerprint(tags: &FileTags, bytes: &[u8]) -> Fallible<Fingerprint> {
        super::fingerprint_from(&mut Cursor::new(bytes), tags)
    }
}
