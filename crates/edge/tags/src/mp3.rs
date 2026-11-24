use std::io::{self, Read, Seek, SeekFrom};
use std::ops::Range;
use std::time::Duration;

use crate::DurationSource;
use crate::payload::{at, malformed};

struct Frame {
    rate: u32,
    samples: u32,
    length: usize,
    xing_offset: usize,
    crc: bool,
}

impl Frame {
    fn decode([a, b, c, d]: [u8; 4]) -> Option<Self> {
        let version = (b >> 3) & 3;
        if a != 0xff || b & 0xe6 != 0xe2 || version == 1 {
            return None;
        }
        let rate = *[44_100, 48_000, 32_000].get(usize::from((c >> 2) & 3))?
            / match version {
                3 => 1,
                2 => 2,
                _ => 4,
            };
        let mpeg1 = version == 3;
        let bitrate = *if mpeg1 {
            [
                0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320,
            ]
        } else {
            [0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160]
        }
        .get(usize::from(c >> 4))?;
        if bitrate == 0 {
            return None;
        }
        let xing_offset = 4 + match (mpeg1, d >> 6 == 3) {
            (true, false) => 32,
            (true, true) | (false, false) => 17,
            (false, true) => 9,
        };
        let factor = if mpeg1 { 144_000 } else { 72_000 };
        Some(Self {
            rate,
            samples: if mpeg1 { 1152 } else { 576 },
            length: usize::try_from(factor * bitrate / rate + u32::from((c >> 1) & 1)).ok()?,
            xing_offset,
            crc: b & 1 == 0,
        })
    }

    fn encoder_samples(&self, bytes: &[u8]) -> Option<u64> {
        let after_crc = self.xing_offset + usize::from(self.crc) * 2;
        for offset in [after_crc, self.xing_offset] {
            if let Some([id @ .., f0, f1, f2, f3, a, b, c, d]) = bytes.get(offset..offset + 12)
                && matches!(id, b"Xing" | b"Info")
            {
                let flags = u32::from_be_bytes([*f0, *f1, *f2, *f3]);
                let count = u32::from_be_bytes([*a, *b, *c, *d]);
                if flags & 1 == 0 || count == 0 {
                    continue;
                }
                let extension = offset
                    + 12
                    + usize::from(flags & 2 != 0) * 4
                    + usize::from(flags & 4 != 0) * 100
                    + usize::from(flags & 8 != 0) * 4;
                let trim = match bytes.get(extension..extension + 24) {
                    Some(
                        [b'L', b'A', b'M', b'E', .., a, b, c]
                        | [b'L', b'a', b'v', b'f' | b'c', .., a, b, c],
                    ) => {
                        let delay = u16::from(*a) << 4 | u16::from(*b >> 4);
                        let padding = u16::from(*b & 15) << 8 | u16::from(*c);
                        u64::from(delay + padding)
                    }
                    _ => 0,
                };
                return (u64::from(count) * u64::from(self.samples))
                    .checked_sub(trim)
                    .filter(|n| *n != 0);
            }
        }
        if let Some([b'V', b'B', b'R', b'I', 0, 1, .., a, b, c, d]) = bytes.get(36..54) {
            return Some(u64::from(u32::from_be_bytes([*a, *b, *c, *d])) * u64::from(self.samples))
                .filter(|n| *n != 0);
        }
        None
    }
}

pub(crate) fn first_audio(
    reader: &mut (impl Read + Seek),
    range: Range<u64>,
) -> io::Result<Range<u64>> {
    let length = range.end - range.start;
    if length >= 4
        && let Some(frame) = Frame::decode(at(reader, range.start)?)
        && u64::try_from(frame.length).is_ok_and(|len| len <= length)
    {
        return Ok(range);
    }
    let max_probe = usize::try_from(length.min(65_540)).map_err(malformed)?;
    let mut bytes = vec![0; max_probe];
    reader.seek(SeekFrom::Start(range.start))?;
    reader.read_exact(&mut bytes)?;
    for (index, header) in bytes.windows(4).enumerate().skip(1) {
        let Some(frame) = Frame::decode(header.try_into().map_err(malformed)?) else {
            continue;
        };
        let offset = range.start + u64::try_from(index).map_err(malformed)?;
        let next = offset + u64::try_from(frame.length).map_err(malformed)?;
        if range.end.saturating_sub(next) >= 4
            && Frame::decode(at(reader, next)?).is_some_and(|other| other.rate == frame.rate)
        {
            return Ok(offset..range.end);
        }
    }
    // Free-format MPEG can still have usable properties, but not a trustworthy frame walk.
    Ok(range)
}

pub(crate) fn duration(
    reader: &mut (impl Read + Seek),
    range: &Range<u64>,
    estimate: Duration,
) -> io::Result<(Duration, DurationSource)> {
    let Some(first) = Frame::decode(at(reader, range.start)?) else {
        return Ok((estimate, DurationSource::Approximate));
    };
    // Even a maximally padded Layer III frame is at most 1441 bytes.
    let mut buffer = [0u8; 1441];
    let first_size = usize::try_from(range.end - range.start)
        .unwrap_or(usize::MAX)
        .min(first.length);
    reader.seek(SeekFrom::Start(range.start))?;
    let bytes = buffer
        .get_mut(..first_size)
        .ok_or_else(|| malformed("invalid MPEG frame length"))?;
    reader.read_exact(bytes)?;
    if let Some(samples) = first.encoder_samples(bytes) {
        return Ok((sample_time(samples, first.rate), DurationSource::Encoder));
    }
    let count = frame_samples(reader, range, &first, &mut buffer)?;
    Ok(
        count.map_or((estimate, DurationSource::Approximate), |samples| {
            (sample_time(samples, first.rate), DurationSource::Frames)
        }),
    )
}

fn frame_samples(
    reader: &mut (impl Read + Seek),
    range: &Range<u64>,
    first: &Frame,
    buffer: &mut [u8],
) -> io::Result<Option<u64>> {
    reader.seek(SeekFrom::Start(range.start))?;
    let mut remaining = range.end - range.start;
    let mut samples = 0u64;
    while remaining >= 4 {
        let mut header = [0; 4];
        reader.read_exact(&mut header)?;
        if header == [0; 4] {
            remaining -= 4;
            break;
        }
        let Some(frame) = Frame::decode(header)
            .filter(|frame| frame.rate == first.rate && frame.samples == first.samples)
        else {
            return Ok(None);
        };
        let length = u64::try_from(frame.length).map_err(malformed)?;
        if length > remaining || frame.length < 4 {
            return Ok(None);
        }
        let Some(bytes) = buffer.get_mut(..frame.length - 4) else {
            return Ok(None);
        };
        reader.read_exact(bytes)?;
        let Some(total) = samples.checked_add(u64::from(frame.samples)) else {
            return Ok(None);
        };
        samples = total;
        remaining -= length;
    }
    while remaining != 0 {
        let len = usize::try_from(remaining)
            .unwrap_or(usize::MAX)
            .min(buffer.len());
        let bytes = buffer
            .get_mut(..len)
            .ok_or_else(|| malformed("invalid MPEG padding"))?;
        reader.read_exact(bytes)?;
        if bytes.iter().any(|byte| *byte != 0) {
            return Ok(None);
        }
        remaining -= u64::try_from(len).map_err(malformed)?;
    }
    Ok(Some(samples).filter(|n| *n != 0))
}

fn sample_time(samples: u64, rate: u32) -> Duration {
    let rate = u64::from(rate);
    let nanos = (samples % rate) * 1_000_000_000 / rate;
    Duration::new(samples / rate, u32::try_from(nanos).unwrap_or_default())
}
