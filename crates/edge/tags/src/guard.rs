//! Pre-scan of a file's `ID3v2` tags, before a parser allocates from them.

use std::io::{Read, Seek};

use crate::Container;
use crate::payload::{MAX_LEADING_TAGS, at};

const V3_COMPRESSED: u8 = 0x80;
const V3_ENCRYPTED: u8 = 0x40;
const V4_COMPRESSED: u8 = 0x08;
const V4_ENCRYPTED: u8 = 0x04;
/// Chunks walked while looking for the `ID3 ` chunk of a RIFF or IFF file.
const MAX_CHUNKS: usize = 1024;

/// Whether this file's `ID3v2` tag must not be decoded. `WAV` and `AIFF`
/// carry it in a chunk, every other container at offset zero.
pub(crate) fn rejects_tags(reader: &mut (impl Read + Seek), container: Container) -> bool {
    match container {
        Container::Wav => chunked(reader, *b"RIFF", u32::from_le_bytes),
        Container::Aiff => chunked(reader, *b"FORM", u32::from_be_bytes),
        _ => unsafe_id3v2(reader, 0),
    }
}

/// Whether the `ID3v2` tag at `start` holds a frame that must not be decoded,
/// or is too large to walk at all.
///
/// A compressed frame expands to a size the frame header does not bound, so a
/// 49 KiB file can ask for tens of megabytes; an encrypted frame is opaque.
/// An oversized tag is skipped whole: the audio is still read, without tags.
pub(crate) fn unsafe_id3v2(reader: &mut (impl Read + Seek), start: u64) -> bool {
    let Ok([b'I', b'D', b'3', version, _, flags, a, b, c, d]) = at::<10>(reader, start) else {
        return false;
    };
    if [a, b, c, d].iter().any(|&part| part >= 128) {
        return false;
    }
    let size = [a, b, c, d]
        .into_iter()
        .fold(0u64, |size, part| (size << 7) | u64::from(part));
    if size.saturating_add(10) > MAX_LEADING_TAGS {
        return true;
    }
    if version < 3 {
        // ID3v2.2 frames carry no flags at all, so nothing can be compressed.
        return false;
    }
    let end = start + 10 + size;
    let mut offset = start + 10 + extended_header(reader, version, flags, start);
    while offset + 10 <= end {
        let Ok([i0, i1, i2, i3, s0, s1, s2, s3, _, frame_flags]) = at::<10>(reader, offset) else {
            return false;
        };
        if [i0, i1, i2, i3] == [0, 0, 0, 0] {
            return false;
        }
        let (compressed, encrypted) = if version >= 4 {
            (V4_COMPRESSED, V4_ENCRYPTED)
        } else {
            (V3_COMPRESSED, V3_ENCRYPTED)
        };
        if frame_flags & (compressed | encrypted) != 0 {
            return true;
        }
        let frame = if version >= 4 && [s0, s1, s2, s3].iter().all(|&part| part < 128) {
            [s0, s1, s2, s3]
                .into_iter()
                .fold(0u64, |size, part| (size << 7) | u64::from(part))
        } else {
            u64::from(u32::from_be_bytes([s0, s1, s2, s3]))
        };
        let Some(next) = offset
            .checked_add(10)
            .and_then(|next| next.checked_add(frame))
        else {
            return false;
        };
        offset = next;
    }
    false
}

fn extended_header(reader: &mut (impl Read + Seek), version: u8, flags: u8, start: u64) -> u64 {
    if flags & 0x40 == 0 {
        return 0;
    }
    let Ok(bytes) = at::<4>(reader, start + 10) else {
        return 0;
    };
    if version >= 4 {
        // The v2.4 extended header size is syncsafe and counts itself.
        bytes
            .into_iter()
            .fold(0u64, |size, part| (size << 7) | u64::from(part))
    } else {
        u64::from(u32::from_be_bytes(bytes)) + 4
    }
}

/// Walk a RIFF or IFF container's chunks and guard the `ID3 ` chunk it
/// carries: the tag is at a chunk offset, not at the start of the file.
fn chunked(reader: &mut (impl Read + Seek), form: [u8; 4], size: fn([u8; 4]) -> u32) -> bool {
    let Ok(header) = at::<12>(reader, 0) else {
        return false;
    };
    if !header.starts_with(&form) {
        return false;
    }
    let mut offset = 12;
    for _ in 0..MAX_CHUNKS {
        let Ok(chunk) = at::<8>(reader, offset) else {
            return false;
        };
        let (id, length) = chunk.split_at(4);
        let Ok(length) = <[u8; 4]>::try_from(length) else {
            return false;
        };
        let length = u64::from(size(length));
        if id.eq_ignore_ascii_case(b"id3 ") && unsafe_id3v2(reader, offset + 8) {
            return true;
        }
        let Some(next) = offset
            .checked_add(8)
            .and_then(|next| next.checked_add(length))
            .map(|next| next + (next % 2))
        else {
            return false;
        };
        offset = next;
    }
    false
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    fn tag(version: u8, frame_flags: u8) -> Vec<u8> {
        let mut bytes = vec![b'I', b'D', b'3', version, 0, 0, 0, 0, 0, 20];
        bytes.extend_from_slice(b"TIT2");
        bytes.extend_from_slice(&[0, 0, 0, 4, 0, frame_flags]);
        bytes.extend_from_slice(&[0, b'a', b'b', b'c']);
        bytes
    }

    /// A `WAV` file whose single `ID3 ` chunk holds `tag`.
    fn wav(tag: &[u8]) -> Vec<u8> {
        let mut bytes = b"RIFF".to_vec();
        bytes.extend_from_slice(&u32::try_from(tag.len() + 12).unwrap().to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(b"id3 ");
        bytes.extend_from_slice(&u32::try_from(tag.len()).unwrap().to_le_bytes());
        bytes.extend_from_slice(tag);
        bytes
    }

    #[test]
    fn a_plain_frame_is_safe() {
        let bytes = tag(3, 0);
        assert!(!super::unsafe_id3v2(&mut Cursor::new(bytes), 0));
    }

    #[test]
    fn a_compressed_v3_frame_is_rejected() {
        let bytes = tag(3, 0x80);
        assert!(super::unsafe_id3v2(&mut Cursor::new(bytes), 0));
    }

    #[test]
    fn a_compressed_v4_frame_is_rejected() {
        let bytes = tag(4, 0x08);
        assert!(super::unsafe_id3v2(&mut Cursor::new(bytes), 0));
    }

    #[test]
    fn a_file_without_a_tag_is_safe() {
        let bytes = b"RIFF0000WAVE".to_vec();
        assert!(!super::unsafe_id3v2(&mut Cursor::new(bytes), 0));
    }

    #[test]
    fn a_compressed_frame_in_a_wav_chunk_is_rejected() {
        let safe = wav(&tag(3, 0));
        assert!(!super::rejects_tags(
            &mut Cursor::new(safe),
            crate::Container::Wav
        ));
        let hostile = wav(&tag(3, 0x80));
        assert!(super::rejects_tags(
            &mut Cursor::new(hostile),
            crate::Container::Wav
        ));
    }

    #[test]
    fn a_tag_over_the_leading_limit_is_rejected_without_being_read() {
        // 18 MiB of declared size, ten bytes of file: nothing is read.
        let header = vec![b'I', b'D', b'3', 3, 0, 0, 0x09, 0, 0, 0];
        assert!(super::unsafe_id3v2(&mut Cursor::new(header), 0));
    }
}
