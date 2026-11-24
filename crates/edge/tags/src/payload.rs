use std::io::{self, Read, Seek, SeekFrom};
use std::ops::Range;

use xxhash_rust::xxh3::Xxh3;

use crate::Container;
use crate::ogg;

const WINDOW: usize = 64 * 1024;
// A leading tag larger than this is a decompression or allocation target,
// never a cover; its frames are skipped and the audio is read without them.
pub(crate) const MAX_LEADING_TAGS: u64 = 16 * 1024 * 1024;
// Enough for every real interleaved file; millions of tiny boxes are an attack.
const MAX_SEGMENTS: usize = 4096;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Layout {
    Span(Range<u64>),
    Segments(Vec<Range<u64>>),
    Ogg(ogg::Stream),
}

impl Layout {
    pub(crate) fn bounds(&self) -> Range<u64> {
        match self {
            Self::Span(range) => range.clone(),
            Self::Segments(ranges) => {
                ranges.first().map_or(0, |r| r.start)..ranges.last().map_or(0, |r| r.end)
            }
            Self::Ogg(stream) => stream.bounds(),
        }
    }

    pub(crate) fn byte_ranges(&self) -> Option<&[Range<u64>]> {
        match self {
            Self::Span(range) => Some(std::slice::from_ref(range)),
            Self::Segments(ranges) => Some(ranges),
            Self::Ogg(_) => None,
        }
    }

    pub(crate) fn hash(&self, reader: &mut (impl Read + Seek)) -> io::Result<(u64, u128)> {
        let mut hash = Xxh3::new();
        let size = if let Self::Ogg(stream) = self {
            stream.hash(reader, &mut hash, WINDOW)?
        } else {
            let ranges = self
                .byte_ranges()
                .ok_or_else(|| malformed("missing audio ranges"))?;
            let size = ranges
                .iter()
                .map(|range| range.end - range.start)
                .sum::<u64>();
            let window = u64::try_from(WINDOW).unwrap_or_default();
            let mut buffer = vec![0; WINDOW];
            if size <= 2 * window {
                hash_window(reader, ranges, 0, size, &mut hash, &mut buffer)?;
            } else {
                hash_window(reader, ranges, 0, window, &mut hash, &mut buffer)?;
                hash_window(
                    reader,
                    ranges,
                    size - window,
                    window,
                    &mut hash,
                    &mut buffer,
                )?;
            }
            size
        };
        Ok((size, hash.digest128()))
    }
}

fn hash_window(
    reader: &mut (impl Read + Seek),
    ranges: &[Range<u64>],
    mut skip: u64,
    mut length: u64,
    hash: &mut Xxh3,
    buffer: &mut [u8],
) -> io::Result<()> {
    for range in ranges {
        let size = range.end - range.start;
        if skip >= size {
            skip -= size;
            continue;
        }
        reader.seek(SeekFrom::Start(range.start + skip))?;
        let mut available = (size - skip).min(length);
        while available > 0 {
            let count = usize::try_from(available)
                .unwrap_or(usize::MAX)
                .min(buffer.len());
            let bytes = buffer
                .get_mut(..count)
                .ok_or_else(|| malformed("invalid window"))?;
            reader.read_exact(bytes)?;
            hash.update(bytes);
            let count = u64::try_from(count).map_err(malformed)?;
            available -= count;
            length -= count;
        }
        if length == 0 {
            break;
        }
        skip = 0;
    }
    Ok(())
}

pub(crate) fn locate(reader: &mut (impl Read + Seek), container: Container) -> io::Result<Layout> {
    let end = reader.seek(SeekFrom::End(0))?;
    let start = leading_tags(reader, end)?;
    let range = match container {
        Container::Mpeg | Container::WavPack => start..trailing_tags(reader, start, end)?,
        Container::Flac => {
            let audio_end = trailing_tags(reader, start, end)?;
            flac(reader, start, audio_end)?
        }
        Container::Ape => {
            let audio_end = trailing_tags(reader, start, end)?;
            ape(reader, start, audio_end)?
        }
        Container::Mp4 => return mp4(reader, start, end),
        Container::Ogg => return ogg::Stream::locate(reader, start, end).map(Layout::Ogg),
        Container::Wav | Container::Aiff => {
            return iff(reader, start, end, container == Container::Wav);
        }
    };
    if range.is_empty() || range.end > end {
        return Err(malformed("invalid audio extent"));
    }
    Ok(Layout::Span(range))
}

pub(crate) fn malformed(detail: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, detail)
}

pub(crate) fn at<const N: usize>(
    reader: &mut (impl Read + Seek),
    start: u64,
) -> io::Result<[u8; N]> {
    let mut bytes = [0; N];
    reader.seek(SeekFrom::Start(start))?;
    reader.read_exact(&mut bytes)?;
    Ok(bytes)
}

fn leading_tags(reader: &mut (impl Read + Seek), end: u64) -> io::Result<u64> {
    let mut start = 0;
    while end - start >= 10 {
        let header = at::<10>(reader, start)?;
        if let [b'I', b'D', b'3', version, _, flags, a, b, c, d] = header {
            if [a, b, c, d].iter().any(|&part| part >= 128) {
                return Err(malformed("ID3 size is not syncsafe"));
            }
            let size = [a, b, c, d]
                .into_iter()
                .fold(0u64, |size, part| (size << 7) | u64::from(part));
            start += 10
                + size
                + if version == 4 && flags & 0x10 != 0 {
                    10
                } else {
                    0
                };
        } else if header.starts_with(b"APETAGEX") && end - start >= 32 {
            let size = u64::from(u32::from_le_bytes(at(reader, start + 12)?));
            if size < 32 {
                return Err(malformed("APE tag size is too small"));
            }
            start += 32 + size;
        } else {
            break;
        }
        if start > end {
            return Err(malformed("leading tag exceeds the file"));
        }
        // An oversized tag is not malformed: the payload still starts after
        // it, and `guard` tells the parser to skip the tag's frames.
    }
    Ok(start)
}

fn trailing_tags(reader: &mut (impl Read + Seek), start: u64, mut end: u64) -> io::Result<u64> {
    loop {
        if end - start >= 128 && at::<3>(reader, end - 128)? == *b"TAG" {
            end -= 128;
            if end - start >= 227 && at::<4>(reader, end - 227)? == *b"TAG+" {
                end -= 227;
            }
            continue;
        }
        if end - start >= 15 && at::<9>(reader, end - 9)? == *b"LYRICS200" {
            let size = at::<6>(reader, end - 15)?;
            let size = std::str::from_utf8(&size)
                .ok()
                .and_then(|s| s.parse::<u64>().ok())
                .filter(|size| *size >= 11 && *size <= end - start - 15)
                .ok_or_else(|| malformed("invalid Lyrics3 size"))?;
            end -= size + 15;
            continue;
        }
        if end - start >= 32 && at::<8>(reader, end - 32)? == *b"APETAGEX" {
            let size = u64::from(u32::from_le_bytes(at(reader, end - 20)?));
            let flags = u32::from_le_bytes(at(reader, end - 12)?);
            let extent = size + if flags & (1 << 31) != 0 { 32 } else { 0 };
            if size < 32 || extent > end - start {
                return Err(malformed("invalid trailing APE tag"));
            }
            end -= extent;
            continue;
        }
        return Ok(end);
    }
}

fn flac(reader: &mut (impl Read + Seek), start: u64, end: u64) -> io::Result<Range<u64>> {
    if end - start < 8 || at::<4>(reader, start)? != *b"fLaC" {
        return Err(malformed("missing FLAC marker"));
    }
    let mut offset = start + 4;
    while end - offset >= 4 {
        let [flags, a, b, c] = at(reader, offset)?;
        let size = u64::from(u32::from_be_bytes([0, a, b, c]));
        if size > end - offset - 4 {
            break;
        }
        offset += size + 4;
        if flags & 0x80 != 0 {
            return Ok(offset..end);
        }
    }
    Err(malformed("missing final FLAC metadata block"))
}

fn ape(reader: &mut (impl Read + Seek), start: u64, end: u64) -> io::Result<Range<u64>> {
    let version = u16::from_le_bytes(at(reader, start + 4)?);
    let le = |reader: &mut _, offset| {
        at(reader, start + offset)
            .map(u32::from_le_bytes)
            .map(u64::from)
    };
    if version >= 3980 {
        let offset = start + le(reader, 8)? + le(reader, 12)? + le(reader, 16)? + le(reader, 20)?;
        let size = (le(reader, 28)? << 32) | le(reader, 24)?;
        let limit = offset
            .checked_add(size)
            .filter(|limit| *limit <= end)
            .ok_or_else(|| malformed("APE frame extent exceeds the file"))?;
        return Ok(offset..limit);
    }
    let flags = u16::from_le_bytes(at(reader, start + 8)?);
    let wav_header = le(reader, 16)?;
    let wav_tail = le(reader, 20)?;
    let frames = le(reader, 24)?;
    let mut header = 32;
    if flags & 4 != 0 {
        header += 4;
    }
    // The old descriptor stores the seek-table count only when it differs from the frame count.
    let entries = if flags & 16 != 0 {
        let stored = le(reader, header)?;
        header += 4;
        stored
    } else {
        frames
    };
    if flags & 32 == 0 {
        header += wav_header;
    }
    let limit = end
        .checked_sub(wav_tail)
        .ok_or_else(|| malformed("invalid APE WAV trailer"))?;
    Ok((start + header + 4 * entries)..limit)
}

fn mp4(reader: &mut (impl Read + Seek), start: u64, end: u64) -> io::Result<Layout> {
    let mut ranges = Vec::new();
    let mut offset = start;
    while end - offset >= 8 {
        let [s0, s1, s2, s3, t0, t1, t2, t3] = at(reader, offset)?;
        let (size, header) = match u32::from_be_bytes([s0, s1, s2, s3]) {
            0 => (end - offset, 8),
            1 if end - offset >= 16 => (u64::from_be_bytes(at(reader, offset + 8)?), 16),
            size => (u64::from(size), 8),
        };
        if size < header || size > end - offset {
            return Err(malformed("invalid MP4 box size"));
        }
        if [t0, t1, t2, t3] == *b"mdat" && size > header {
            if ranges.len() >= MAX_SEGMENTS {
                return Err(malformed("too many MP4 audio boxes"));
            }
            ranges.push(offset + header..offset + size);
        }
        offset += size;
    }
    segments(ranges)
}

fn iff(reader: &mut (impl Read + Seek), start: u64, end: u64, wav: bool) -> io::Result<Layout> {
    let endian = if wav {
        u32::from_le_bytes
    } else {
        u32::from_be_bytes
    };
    let mut offset = start + 12;
    let mut found = Vec::new();
    let form_end = start + 8 + u64::from(endian(at(reader, start + 4)?));
    if form_end > end || offset > form_end {
        return Err(malformed("invalid RIFF/AIFF form size"));
    }
    while form_end - offset >= 8 {
        let [t0, t1, t2, t3, s0, s1, s2, s3] = at(reader, offset)?;
        let size = u64::from(endian([s0, s1, s2, s3]));
        let id = [t0, t1, t2, t3];
        let body = offset + 8;
        if size > form_end - body {
            return Err(malformed("invalid RIFF/AIFF chunk size"));
        }
        if wav && id == *b"data" && size != 0 {
            found.push(body..body + size);
        } else if !wav && id == *b"SSND" && size >= 8 {
            let skip = 8 + u64::from(u32::from_be_bytes(at(reader, body)?));
            if skip >= size {
                return Err(malformed("invalid AIFF sound chunk offset"));
            }
            found.push(body + skip..body + size);
        }
        if found.len() > MAX_SEGMENTS {
            return Err(malformed("too many RIFF/AIFF audio chunks"));
        }
        offset = body + size + size % 2;
        if offset > form_end {
            break;
        }
    }
    segments(found)
}

fn segments(mut ranges: Vec<Range<u64>>) -> io::Result<Layout> {
    match ranges.len() {
        0 => Err(malformed("missing audio data chunk")),
        1 => ranges
            .pop()
            .map(Layout::Span)
            .ok_or_else(|| malformed("missing audio data chunk")),
        _ => Ok(Layout::Segments(ranges)),
    }
}
