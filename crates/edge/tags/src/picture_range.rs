use std::io::{self, Read, Seek, SeekFrom};
use std::ops::Range;

use crate::{Container, FileTags, payload};

// Limit the search to metadata, even when the file has a large padding or free box.
const SEARCH_LIMIT: u64 = 48 * 1024 * 1024;
// A longer picture is located by its tag index instead: the failure table would
// cost four bytes per picture byte for a span that saves one tag parse.
const MAX_NEEDLE: usize = 4 * 1024 * 1024;

pub(crate) fn find(
    reader: &mut (impl Read + Seek),
    tags: &FileTags,
    picture: &[u8],
) -> io::Result<Option<Range<u64>>> {
    if picture.is_empty() {
        return Ok(None);
    }
    match tags.properties.container {
        Container::Mpeg | Container::Flac => {
            let end = tags.payload.0.bounds().start;
            if end >= 10
                && let [b'I', b'D', b'3', _, _, flags, ..] = payload::at::<10>(reader, 0)?
                && flags & 0x80 != 0
            {
                return Ok(None);
            }
            search(reader, 0..end, picture)
        }
        Container::Mp4 => {
            let end = reader.seek(SeekFrom::End(0))?;
            let Some(ranges) = tags.payload.0.byte_ranges() else {
                return Ok(None);
            };
            let mut start = 0;
            for next in ranges
                .iter()
                .map(|range| (range.start, range.end))
                .chain([(end, end)])
            {
                if let Some(found) = search(reader, start..next.0, picture)? {
                    return Ok(Some(found));
                }
                start = next.1;
            }
            Ok(None)
        }
        _ => Ok(None),
    }
}

fn search(
    reader: &mut (impl Read + Seek),
    extent: Range<u64>,
    picture: &[u8],
) -> io::Result<Option<Range<u64>>> {
    let length = extent.end.saturating_sub(extent.start);
    let size = u64::try_from(picture.len()).map_err(payload::malformed)?;
    if length < size || length > SEARCH_LIMIT || picture.len() > MAX_NEEDLE {
        return Ok(None);
    }
    let mut metadata = vec![0; usize::try_from(length).map_err(payload::malformed)?];
    reader.seek(SeekFrom::Start(extent.start))?;
    reader.read_exact(&mut metadata)?;
    let Some(index) = position(&metadata, picture) else {
        return Ok(None);
    };
    let offset = extent.start + u64::try_from(index).map_err(payload::malformed)?;
    Ok(Some(offset..offset + size))
}

/// Knuth-Morris-Pratt: metadata full of repeated bytes makes the naive window
/// scan quadratic, and a 1 MiB picture in a 48 MiB tag then takes hours.
fn position(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    let table = failure_table(needle)?;
    let mut matched = 0usize;
    for (index, &byte) in haystack.iter().enumerate() {
        while matched > 0 && Some(&byte) != needle.get(matched) {
            matched = usize::try_from(*table.get(matched.checked_sub(1)?)?).ok()?;
        }
        if Some(&byte) == needle.get(matched) {
            matched += 1;
        }
        if matched == needle.len() {
            return index.checked_add(1)?.checked_sub(matched);
        }
    }
    None
}

fn failure_table(needle: &[u8]) -> Option<Vec<u32>> {
    let mut table = vec![0u32; needle.len()];
    let mut prefix = 0usize;
    for index in 1..needle.len() {
        let byte = *needle.get(index)?;
        while prefix > 0 && Some(&byte) != needle.get(prefix) {
            prefix = usize::try_from(*table.get(prefix.checked_sub(1)?)?).ok()?;
        }
        if Some(&byte) == needle.get(prefix) {
            prefix += 1;
        }
        *table.get_mut(index)? = u32::try_from(prefix).ok()?;
    }
    Some(table)
}
