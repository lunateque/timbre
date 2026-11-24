//! Files on disk: cover art and bounded reads; lyrics in the `lyrics` child.

mod lyrics;

pub(crate) use lyrics::{
    SidecarLyric, lyric_filename, lyric_fits, lyrics, read_sidecar_lyric, save_embedded_lyrics,
    save_sidecar_lyric,
};

use crate::failures::{LibraryFailure, io};
use crate::store::{contained, exec};
use std::fs::File;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use timbre_edge_image::Limits;
use timbre_edge_sqlite::rusqlite::{Connection, params};
use timbre_edge_tags::{FileTags, Picture};
use timbre_kernel::Fallible;

pub(crate) const IMAGE_LIMITS: Limits = Limits {
    max_dimension: 8192,
    max_bytes: 32 * 1024 * 1024,
};
/// Enough leading bytes to tell JPEG, PNG and WEBP apart (WEBP needs 12).
const SIGNATURE_BYTES: usize = 16;

#[derive(Clone)]
pub(crate) struct ArtData {
    pub(crate) hash: String,
    mime: &'static str,
}

impl ArtData {
    fn new(bytes: &[u8]) -> Option<Self> {
        let mime = timbre_edge_image::mime(bytes)?;
        let hash = format!("{:032x}", timbre_edge_image::content_hash(bytes));
        Some(Self { hash, mime })
    }

    pub(crate) fn write(&self, sql: &Connection) -> Fallible {
        exec(
            sql,
            "INSERT OR IGNORE INTO library_art(hash,mime) VALUES(?1,?2)",
            params![self.hash, self.mime],
        )?;
        Ok(())
    }
}

pub(crate) struct EmbeddedArt {
    pub(crate) art: ArtData,
    pub(crate) front: bool,
    index: i64,
    span: Option<std::ops::Range<u64>>,
}

impl EmbeddedArt {
    pub(crate) fn write(&self, sql: &Connection, track: i64) -> Fallible {
        let offset = self
            .span
            .as_ref()
            .and_then(|range| i64::try_from(range.start).ok());
        let length = offset
            .and(self.span.as_ref())
            .and_then(|range| i64::try_from(range.end - range.start).ok());
        exec(
            sql,
            "INSERT OR REPLACE INTO library_embedded_art(track_id,offset,length,picture) VALUES(?1,?2,?3,?4)",
            params![track, offset, length, self.index],
        )?;
        Ok(())
    }
}

pub(crate) fn embedded_art(path: &Path, tags: &FileTags) -> Option<EmbeddedArt> {
    let valid = |picture: &Picture| {
        picture.bytes.len() <= IMAGE_LIMITS.max_bytes
            && timbre_edge_image::mime(&picture.bytes).is_some()
    };
    let front = tags
        .pictures
        .iter()
        .position(|picture| picture.kind == Some(3) && valid(picture));
    let chosen = front.or_else(|| tags.pictures.iter().position(valid))?;
    let picture = tags.pictures.get(chosen)?;
    Some(EmbeddedArt {
        art: ArtData::new(&picture.bytes)?,
        front: front.is_some(),
        span: timbre_edge_tags::picture_range(path, tags, &picture.bytes)
            .ok()
            .flatten(),
        index: i64::try_from(chosen).ok()?,
    })
}

pub(crate) fn sidecar_art(path: &Path, root: &Path) -> Fallible<Option<(ArtData, PathBuf)>> {
    let path = contained(path, root)?;
    if !has_image_signature(&path)? {
        return Ok(None);
    }
    let bytes = read_bounded(&path, IMAGE_LIMITS.max_bytes)?
        .ok_or_else(|| LibraryFailure::ImageTooLarge.boxed())?;
    Ok(ArtData::new(&bytes).map(|art| (art, path)))
}

/// Whether the first bytes name a format this server decodes. A large file
/// that only looks like a cover by its name is refused before it is read.
fn has_image_signature(path: &Path) -> Fallible<bool> {
    let mut file = File::open(path).map_err(io)?;
    let mut head = [0u8; SIGNATURE_BYTES];
    let mut filled = 0usize;
    while let Some(slot) = head.get_mut(filled..).filter(|slot| !slot.is_empty()) {
        let read = file.read(slot).map_err(io)?;
        if read == 0 {
            break;
        }
        filled = filled.saturating_add(read);
    }
    Ok(head
        .get(..filled)
        .and_then(timbre_edge_image::mime)
        .is_some())
}

/// Image failures belong to the edge crate; at this boundary they become
/// the library's own codes, so no client ever sees an `image.*` code.
pub(crate) fn image_failure(
    failure: &dyn timbre_kernel::Failure,
) -> Box<dyn timbre_kernel::Failure> {
    match failure.code() {
        timbre_edge_image::failures::LIMIT => LibraryFailure::ImageTooLarge.boxed(),
        // A format this build cannot decode is not a server fault: the art
        // is simply not servable, which clients read as absent.
        timbre_edge_image::failures::UNSUPPORTED => LibraryFailure::NotFound.boxed(),
        _ => LibraryFailure::ImageUnreadable.boxed(),
    }
}

pub(crate) fn read_bounded(path: &Path, cap: usize) -> Fallible<Option<Vec<u8>>> {
    let file = File::open(path).map_err(io)?;
    let max = u64::try_from(cap).unwrap_or(u64::MAX);
    if file.metadata().map_err(io)?.len() > max {
        return Ok(None);
    }
    let mut bytes = Vec::new();
    file.take(max.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(io)?;
    Ok((bytes.len() <= cap).then_some(bytes))
}

#[cfg(test)]
mod tests {
    use timbre_edge_image::failures::ImageFailure;

    #[test]
    fn an_undecodable_format_is_absent_art_and_not_a_server_fault() {
        let cases = [
            (ImageFailure::Unsupported, "library.not_found", 404),
            (ImageFailure::Limit, "library.image_too_large", 413),
            (
                ImageFailure::Decode("broken".to_owned()),
                "library.image_unreadable",
                500,
            ),
            (
                ImageFailure::Thumbnail("broken".to_owned()),
                "library.image_unreadable",
                500,
            ),
        ];
        for (failure, code, status) in cases {
            let mapped = super::image_failure(&failure);
            assert_eq!(mapped.code().as_str(), code);
            assert_eq!(mapped.status(), status);
        }
    }
}
