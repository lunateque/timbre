use timbre_edge_tags::{fingerprint, read};
use xxhash_rust::xxh3::xxh3_128;

use crate::common::Sample;

#[test]
fn ogg_fingerprint_survives_comments_spanning_pages_and_sharing_the_audio_page() {
    for (name, header_count) in [("tone.ogg", 3), ("tone.opus", 2)] {
        let sample = Sample::new(name);
        let source = std::fs::read(&sample.path).unwrap();
        let (mut packets, serial, final_granule) = open(&source);
        let tags = read(&sample.path).unwrap();
        let before = fingerprint(&sample.path, &tags).unwrap();
        let audio = packets[header_count..].concat();
        assert_eq!(before.hash, xxh3_128(&audio));

        packets[1] = comment(header_count == 2, 80_000);
        let shared = repaginate(&packets, header_count, serial, final_granule);
        std::fs::write(&sample.path, shared).unwrap();
        let retagged = read(&sample.path).unwrap();
        // The 80 000 byte comment still spans pages; the value itself is
        // clamped to the 64 KiB text cap.
        assert_eq!(retagged.fields.title.as_ref().unwrap().len(), 65_536);
        assert!(retagged.payload.bounds().start > tags.payload.bounds().start);
        assert_eq!(
            fingerprint(&sample.path, &retagged).unwrap(),
            before,
            "{name}"
        );

        packets[header_count][10] ^= 1;
        let changed = repaginate(&packets, header_count, serial, final_granule);
        std::fs::write(&sample.path, changed).unwrap();
        let changed_tags = read(&sample.path).unwrap();
        let identity = fingerprint(&sample.path, &changed_tags).unwrap();
        assert_ne!(identity.hash, before.hash);
        assert_eq!(identity.duration, before.duration);
        assert_eq!(identity.payload_size, before.payload_size);
    }
}

#[test]
fn ogg_windows_do_not_repeat_an_overlapping_suffix_or_hash_the_middle() {
    const WINDOW: usize = 64 * 1024;
    let sample = Sample::new("tone.opus");
    let source = std::fs::read(&sample.path).unwrap();
    let (source, serial, granule) = open(&source);
    let packet = &source[2];
    for size in [WINDOW + 1, 3 * WINDOW + 10] {
        let mut packets = source[..2].to_vec();
        packets.extend((0..size.div_ceil(packet.len())).map(|_| packet.clone()));
        let total = packets[2..].concat();
        let suffix = WINDOW.max(total.len() - WINDOW);
        let oracle = [total[..WINDOW].to_vec(), total[suffix..].to_vec()].concat();
        std::fs::write(&sample.path, repaginate(&packets, 2, serial, granule)).unwrap();
        let tags = read(&sample.path).unwrap();
        let identity = fingerprint(&sample.path, &tags).unwrap();
        assert_eq!(identity.hash, xxh3_128(&oracle));
        assert_eq!(identity.payload_size, 48_000);
    }
}

fn open(mut bytes: &[u8]) -> (Vec<Vec<u8>>, u32, u64) {
    let serial = u32::from_le_bytes(bytes[14..18].try_into().unwrap());
    let mut granule = 0;
    let mut packets = Vec::new();
    let mut in_progress = Vec::new();
    while !bytes.is_empty() {
        assert_eq!(&bytes[..4], b"OggS");
        granule = u64::from_le_bytes(bytes[6..14].try_into().unwrap());
        let (header, mut body) = bytes.split_at(27 + usize::from(bytes[26]));
        for &lace in &header[27..] {
            let (chunk, tail) = body.split_at(usize::from(lace));
            in_progress.extend(chunk);
            body = tail;
            if lace < 255 {
                packets.push(std::mem::take(&mut in_progress));
            }
        }
        bytes = body;
    }
    assert_eq!(in_progress, Vec::new());
    (packets, serial, granule)
}

fn comment(opus: bool, size: usize) -> Vec<u8> {
    let mut packet = if opus {
        b"OpusTags".to_vec()
    } else {
        b"\x03vorbis".to_vec()
    };
    packet.extend_from_slice(&4u32.to_le_bytes());
    packet.extend_from_slice(b"test");
    packet.extend_from_slice(&1u32.to_le_bytes());
    packet.extend_from_slice(&u32::try_from(6 + size).unwrap().to_le_bytes());
    packet.extend_from_slice(b"TITLE=");
    packet.extend(std::iter::repeat_n(b'x', size));
    if !opus {
        packet.push(1);
    }
    packet
}

fn repaginate(packets: &[Vec<u8>], headers: usize, serial: u32, final_granule: u64) -> Vec<u8> {
    let mut output = Vec::new();
    let mut sequence = 0;
    for (group, flags, granule) in [
        (&packets[..1], 2, 0),
        (&packets[1..=headers], 0, 0),
        (&packets[headers + 1..], 4, final_granule),
    ] {
        let laces: Vec<u8> = group
            .iter()
            .flat_map(|packet| {
                std::iter::repeat_n(255, packet.len() / 255)
                    .chain([u8::try_from(packet.len() % 255).unwrap()])
            })
            .collect();
        let body = group.concat();
        let mut payload = body.as_slice();
        let mut continued = false;
        let count = laces.len().div_ceil(255);
        for (n, segments) in laces.chunks(255).enumerate() {
            let last = n + 1 == count;
            let next_offset = segments.iter().map(|&size| usize::from(size)).sum();
            let (bytes, rest) = payload.split_at(next_offset);
            let flags = if last { flags } else { 0 } | u8::from(continued);
            let granule = if last { granule } else { u64::MAX };
            append_page(
                &mut output,
                serial,
                &mut sequence,
                flags,
                granule,
                segments,
                bytes,
            );
            payload = rest;
            continued = segments.last() == Some(&255);
        }
        assert_eq!(payload, [] as [u8; 0]);
    }
    output
}

fn append_page(
    out: &mut Vec<u8>,
    serial: u32,
    sequence: &mut u32,
    flags: u8,
    granule: u64,
    laces: &[u8],
    body: &[u8],
) {
    let start = out.len();
    out.extend_from_slice(b"OggS");
    out.extend_from_slice(&[0, flags]);
    out.extend_from_slice(&granule.to_le_bytes());
    out.extend_from_slice(&serial.to_le_bytes());
    out.extend_from_slice(&sequence.to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    out.push(u8::try_from(laces.len()).unwrap());
    out.extend_from_slice(laces);
    out.extend_from_slice(body);
    let checksum = out[start..].iter().fold(0u32, |mut crc, &byte| {
        crc ^= u32::from(byte) << 24;
        for _ in 0..8 {
            crc = if crc & 0x8000_0000 == 0 {
                crc << 1
            } else {
                (crc << 1) ^ 0x04c1_1db7
            };
        }
        crc
    });
    out[start + 22..start + 26].copy_from_slice(&checksum.to_le_bytes());
    *sequence += 1;
}
