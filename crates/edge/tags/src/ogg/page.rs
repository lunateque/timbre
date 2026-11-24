use std::io::{self, Read, Seek, SeekFrom};

use crate::payload::{at, malformed};

pub(super) const HEADER: usize = 27;
pub(super) const MAX_PAGE: usize = HEADER + 255 + 255 * 255;
const REVERSE_BLOCK: u64 = 64 * 1024;
// A real end-of-stream page sits in the last pages of the file; scanning further
// back only re-parses garbage that happens to contain the capture pattern.
const REVERSE_BUDGET: u64 = 1024 * 1024;
const MAX_CANDIDATES: u32 = 4096;

pub(super) struct Page {
    pub(super) start: u64,
    pub(super) body: u64,
    pub(super) end: u64,
    pub(super) serial: u32,
    pub(super) granule: u64,
    flags: u8,
    pub(super) segments: usize,
    crc: u32,
}

impl Page {
    pub(super) fn read(
        reader: &mut (impl Read + Seek),
        start: u64,
        end: u64,
        bytes: &mut Vec<u8>,
    ) -> io::Result<Self> {
        if end.saturating_sub(start) < 27 {
            return Err(malformed("truncated Ogg header"));
        }
        let header = at::<HEADER>(reader, start)?;
        let [b'O', b'g', b'g', b'S', 0, flags, tail @ ..] = header else {
            return Err(malformed("invalid Ogg page marker or version"));
        };
        let granule = u64::from_le_bytes(field::<8>(&tail, 0)?);
        let serial = u32::from_le_bytes(field::<4>(&tail, 8)?);
        let crc = u32::from_le_bytes(field::<4>(&tail, 16)?);
        let count = *tail
            .last()
            .ok_or_else(|| malformed("truncated Ogg header"))?;
        let segments = usize::from(count);
        let table_end = HEADER + segments;
        let body = start + 27 + u64::from(count);
        if body > end {
            return Err(malformed("truncated Ogg segment table"));
        }
        bytes.clear();
        bytes.extend_from_slice(&header);
        bytes.resize(table_end, 0);
        let table = bytes
            .get_mut(HEADER..)
            .ok_or_else(|| malformed("invalid Ogg segment table"))?;
        reader.read_exact(table)?;
        let body_len = table.iter().copied().map(u64::from).sum::<u64>();
        if body_len > end - body {
            return Err(malformed("truncated Ogg page body"));
        }
        Ok(Self {
            start,
            body,
            end: body + body_len,
            segments,
            flags,
            serial,
            granule,
            crc,
        })
    }

    fn read_body(&self, reader: &mut impl Read, bytes: &mut Vec<u8>) -> io::Result<()> {
        let offset = bytes.len();
        let length = usize::try_from(self.end - self.body).map_err(malformed)?;
        bytes.resize(offset + length, 0);
        reader.read_exact(
            bytes
                .get_mut(offset..)
                .ok_or_else(|| malformed("missing Ogg body buffer"))?,
        )
    }

    pub(super) fn end_of_stream(&self) -> bool {
        self.flags & 4 != 0
    }
}

fn field<const N: usize>(bytes: &[u8], offset: usize) -> io::Result<[u8; N]> {
    bytes
        .get(offset..offset + N)
        .and_then(|part| <[u8; N]>::try_from(part).ok())
        .ok_or_else(|| malformed("truncated Ogg header field"))
}

pub(super) struct Reverse {
    limit: u64,
    floor: u64,
    block: Vec<u8>,
    block_start: u64,
    cursor: usize,
    candidates: u32,
    /// Bytes this scan may still read. Blocks and candidate bodies both
    /// draw on it: thousands of plausible headers inside one window would
    /// otherwise read far more than the window itself.
    budget: u64,
}

impl Reverse {
    pub(super) fn new(limit: u64, floor: u64) -> Self {
        Self {
            limit,
            floor: floor.max(limit.saturating_sub(REVERSE_BUDGET)),
            block: Vec::new(),
            block_start: limit,
            cursor: 0,
            candidates: 0,
            budget: REVERSE_BUDGET,
        }
    }

    /// Charge `bytes` to the budget; false once it is spent.
    fn spend(&mut self, bytes: u64) -> bool {
        let Some(left) = self.budget.checked_sub(bytes) else {
            self.budget = 0;
            return false;
        };
        self.budget = left;
        true
    }

    pub(super) fn previous(
        &mut self,
        reader: &mut (impl Read + Seek),
        serial: u32,
        bytes: &mut Vec<u8>,
    ) -> io::Result<Option<Page>> {
        loop {
            let parsable = self
                .block
                .get(..self.cursor)
                .ok_or_else(|| malformed("invalid Ogg scan block"))?;
            if let Some(index) = parsable.windows(4).rposition(|part| part == b"OggS") {
                self.cursor = index + 3;
                self.candidates += 1;
                if self.candidates > MAX_CANDIDATES {
                    return Err(malformed("too many Ogg page candidates"));
                }
                let start = self.block_start + u64::try_from(index).map_err(malformed)?;
                let Ok(page) = Page::read(reader, start, self.limit, bytes) else {
                    continue;
                };
                if page.serial != serial {
                    continue;
                }
                if !self.spend(page.end.saturating_sub(page.body)) {
                    return Ok(None);
                }
                page.read_body(reader, bytes)?;
                if crc(bytes) == page.crc {
                    self.limit = start;
                    return Ok(Some(page));
                }
            } else {
                if self.block_start == self.floor {
                    return Ok(None);
                }
                let end = self.block_start.saturating_add(3).min(self.limit);
                self.block_start = self
                    .block_start
                    .saturating_sub(REVERSE_BLOCK)
                    .max(self.floor);
                let size =
                    usize::try_from(end.saturating_sub(self.block_start)).map_err(malformed)?;
                if !self.spend(u64::try_from(size).map_err(malformed)?) {
                    return Ok(None);
                }
                self.block.resize(size, 0);
                self.cursor = size;
                reader.seek(SeekFrom::Start(self.block_start))?;
                reader.read_exact(&mut self.block)?;
            }
        }
    }
}

const fn crc_table() -> [u32; 256] {
    let mut table = [0; 256];
    let mut value = 0;
    // The CRC consumes the high byte first, unlike the common reflected CRC-32.
    let mut entries = table.as_mut_slice();
    while let [entry, rest @ ..] = entries {
        *entry = value << 24;
        let mut bit = 0;
        while bit < 8 {
            *entry = (*entry << 1)
                ^ if *entry & 0x8000_0000 != 0 {
                    0x04c1_1db7
                } else {
                    0
                };
            bit += 1;
        }
        value += 1;
        entries = rest;
    }
    table
}

fn crc(bytes: &[u8]) -> u32 {
    const TABLE: [u32; 256] = crc_table();
    bytes
        .iter()
        .copied()
        .enumerate()
        .fold(0u32, |crc, (offset, byte)| {
            let byte = if (22..26).contains(&offset) { 0 } else { byte };
            let [high, ..] = crc.to_be_bytes();
            (crc << 8)
                ^ TABLE
                    .get(usize::from(high ^ byte))
                    .copied()
                    .unwrap_or_default()
        })
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Read, Seek, SeekFrom};

    /// A reader that remembers how much of the file was touched.
    struct Counting {
        inner: Cursor<Vec<u8>>,
        read: u64,
    }

    impl Read for Counting {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            let count = self.inner.read(buffer)?;
            self.read += u64::try_from(count).unwrap_or(u64::MAX);
            Ok(count)
        }
    }

    impl Seek for Counting {
        fn seek(&mut self, to: SeekFrom) -> std::io::Result<u64> {
            self.inner.seek(to)
        }
    }

    /// Two mebibytes whose last mebibyte is packed with page headers that
    /// parse, carry the wanted serial and declare a 65 KiB body each, but
    /// never match their checksum.
    fn hostile() -> Vec<u8> {
        let mut bytes = vec![0xAA; 2 * 1024 * 1024];
        let mut header = vec![b'O', b'g', b'g', b'S', 0, 0];
        header.extend_from_slice(&0u64.to_le_bytes());
        header.extend_from_slice(&0u32.to_le_bytes());
        header.extend_from_slice(&0u32.to_le_bytes());
        header.extend_from_slice(&0u32.to_le_bytes());
        header.push(255);
        header.extend_from_slice(&[255; 255]);
        let mut offset = 1024 * 1024;
        while offset + header.len() + 255 * 255 < bytes.len() {
            bytes[offset..offset + header.len()].copy_from_slice(&header);
            offset += 1024;
        }
        bytes
    }

    #[test]
    fn a_reverse_scan_stops_reading_at_its_budget() {
        let bytes = hostile();
        let len = u64::try_from(bytes.len()).unwrap();
        let mut reader = Counting {
            inner: Cursor::new(bytes),
            read: 0,
        };
        let mut buffer = Vec::new();
        let found = super::Reverse::new(len, 0)
            .previous(&mut reader, 0, &mut buffer)
            .unwrap();
        assert!(found.is_none());
        assert!(
            reader.read <= 4 * 1024 * 1024,
            "the scan read {} bytes",
            reader.read
        );
    }
}
