mod page;

use std::io::{self, Read, Seek, SeekFrom};
use std::ops::Range;

use xxhash_rust::xxh3::Xxh3;

use crate::payload::{at, malformed};
use crate::{Codec, Container, DurationSource, Properties};

use self::page::{HEADER, MAX_PAGE, Page, Reverse};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Stream {
    serial: u32,
    first_page: u64,
    audio_start: u64,
    channels: u8,
    rate: u32,
    nominal_bitrate: Option<u32>,
    end: u64,
    samples: u64,
}

impl Stream {
    pub(crate) fn locate(
        reader: &mut (impl Read + Seek),
        start: u64,
        end: u64,
    ) -> io::Result<Self> {
        let mut bytes = Vec::with_capacity(MAX_PAGE);
        let ident = Page::read(reader, start, end, &mut bytes)?;
        if ident.end - ident.body < 19 {
            return Err(malformed("short Ogg identification packet"));
        }
        let prefix = at::<19>(reader, ident.body)?;
        let (headers, pre_skip, channels, rate, nominal_bitrate) = match prefix {
            [
                b'O',
                b'p',
                b'u',
                b's',
                b'H',
                b'e',
                b'a',
                b'd',
                _,
                channels,
                a,
                b,
                ..,
            ] => (2, u16::from_le_bytes([a, b]), channels, 48_000, None),
            [
                1,
                b'v',
                b'o',
                b'r',
                b'b',
                b'i',
                b's',
                _,
                _,
                _,
                _,
                channels,
                a,
                b,
                c,
                d,
                ..,
            ] if ident.end - ident.body >= 24 => {
                let nominal = i32::from_le_bytes(at(reader, ident.body + 20)?);
                (
                    3,
                    0,
                    channels,
                    u32::from_le_bytes([a, b, c, d]),
                    u32::try_from(nominal)
                        .ok()
                        .map(|n| n / 1000)
                        .filter(|n| *n != 0),
                )
            }
            _ => return Err(malformed("missing Ogg audio identification packet")),
        };
        if channels == 0 || rate == 0 {
            return Err(malformed("invalid channel count or sample rate"));
        }
        let first = first_audio(reader, start, end, ident.serial, headers, &mut bytes)?;
        let mut search = Reverse::new(end, first.0);
        let last = loop {
            let page = search
                .previous(reader, ident.serial, &mut bytes)?
                .ok_or_else(|| malformed("missing Ogg end-of-stream page"))?;
            if page.end_of_stream() {
                break page;
            }
        };
        let samples = last
            .granule
            .checked_sub(u64::from(pre_skip))
            .filter(|n| *n < u64::MAX)
            .ok_or_else(|| malformed("invalid final Ogg granule"))?;
        if first.1 >= last.end || samples == 0 {
            return Err(malformed("empty Ogg stream"));
        }
        Ok(Self {
            serial: ident.serial,
            first_page: first.0,
            audio_start: first.1,
            end: last.end,
            samples,
            channels,
            rate,
            nominal_bitrate,
        })
    }

    pub(crate) fn bounds(&self) -> Range<u64> {
        self.audio_start..self.end
    }

    pub(crate) fn properties(&self, codec: Codec) -> Properties {
        let rate = u64::from(self.rate);
        let duration = std::time::Duration::new(
            self.samples / rate,
            u32::try_from((self.samples % rate) * 1_000_000_000 / rate).unwrap_or_default(),
        );
        // Opus has no nominal bitrate. The physical audio extent includes page framing.
        let bitrate = self.nominal_bitrate.or_else(|| {
            let kbps = u128::from(self.end - self.audio_start) * 8 * u128::from(rate)
                / (u128::from(self.samples) * 1000);
            u32::try_from(kbps).ok().filter(|n| *n != 0)
        });
        Properties {
            codec,
            container: Container::Ogg,
            duration,
            duration_source: DurationSource::Container,
            bitrate_kbps: bitrate,
            sample_rate: Some(self.rate),
            channels: Some(self.channels),
            bit_depth: None,
        }
    }

    pub(crate) fn hash(
        &self,
        reader: &mut (impl Read + Seek),
        hash: &mut Xxh3,
        window: usize,
    ) -> io::Result<u64> {
        let mut bytes = Vec::with_capacity(MAX_PAGE);
        let mut offset = self.first_page;
        let mut prefix_end = self.audio_start;
        let mut remaining = window;
        while remaining != 0 && offset < self.end {
            let page = Page::read(reader, offset, self.end, &mut bytes)?;
            offset = page.end;
            if page.serial == self.serial {
                let start = page.body.max(self.audio_start);
                reader.seek(SeekFrom::Start(start))?;
                let size = usize::try_from(page.end - start)
                    .map_err(malformed)?
                    .min(remaining);
                bytes.resize(size, 0);
                reader.read_exact(&mut bytes)?;
                hash.update(&bytes);
                remaining -= size;
                prefix_end = start + u64::try_from(size).map_err(malformed)?;
            }
        }
        if prefix_end == self.end || remaining != 0 {
            return Ok(self.samples);
        }

        let mut suffix = vec![0; window];
        let mut search = Reverse::new(self.end, self.first_page);
        let mut remaining = window;
        while remaining != 0 {
            let page = search
                .previous(reader, self.serial, &mut bytes)?
                .ok_or_else(|| malformed("missing Ogg audio page"))?;
            if page.body == page.end && page.start > prefix_end {
                continue;
            }
            let begin = page.body.max(prefix_end);
            if begin >= page.end {
                break;
            }
            let size = usize::try_from(page.end - begin)
                .map_err(malformed)?
                .min(remaining);
            let start = usize::try_from(page.end - page.start).map_err(malformed)? - size;
            let output = suffix
                .get_mut(remaining - size..remaining)
                .ok_or_else(|| malformed("invalid Ogg suffix"))?;
            output.copy_from_slice(
                bytes
                    .get(start..start + size)
                    .ok_or_else(|| malformed("invalid Ogg body"))?,
            );
            remaining -= size;
            if begin == prefix_end {
                break;
            }
        }
        hash.update(
            suffix
                .get(remaining..)
                .ok_or_else(|| malformed("invalid Ogg suffix extent"))?,
        );
        Ok(self.samples)
    }
}

fn first_audio(
    reader: &mut (impl Read + Seek),
    start: u64,
    end: u64,
    serial: u32,
    mut headers: usize,
    bytes: &mut Vec<u8>,
) -> io::Result<(u64, u64)> {
    let mut offset = start;
    while offset < end {
        let page = Page::read(reader, offset, end, bytes)?;
        offset = page.end;
        if page.serial != serial {
            continue;
        }
        let mut audio = page.body;
        let table = bytes
            .get(HEADER..HEADER + page.segments)
            .ok_or_else(|| malformed("invalid Ogg lacing"))?;
        for &size in table {
            if headers == 0 && size != 0 {
                return Ok((page.start, audio));
            }
            audio += u64::from(size);
            if size < 255 && headers != 0 {
                headers -= 1;
            }
        }
    }
    Err(malformed("no Ogg audio packet after headers"))
}
