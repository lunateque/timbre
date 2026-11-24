//! The audio reader must never report file extents beyond its input.

use timbre_edge_tags::{Container, fuzzing};

/// Exercise the shipped reader on arbitrary bytes with every supported extension.
///
/// # Panics
/// Panics for invalid physical extents, or if successful fingerprints disagree
/// with the parsed payload and duration.
pub fn tags_read(bytes: &[u8]) {
    let len = u64::try_from(bytes.len()).unwrap();
    for extension in [
        "mp3", "flac", "ogg", "opus", "m4a", "mp4", "wav", "aiff", "wv", "ape",
    ] {
        let Ok(tags) = fuzzing::read(extension, bytes) else {
            continue;
        };
        let bounds = tags.payload.bounds();
        assert!(bounds.start < bounds.end && bounds.end <= len);
        // Pictures are owned bytes, not borrowed file offsets. A tag may store
        // them compressed or as base64, so the encoded size is not a range.
        if let Some(ranges) = tags.payload.byte_ranges() {
            let mut previous = bounds.start;
            for range in ranges {
                assert!(
                    previous <= range.start && range.start < range.end && range.end <= bounds.end
                );
                previous = range.end;
            }
        }
        if let Ok(identity) = fuzzing::fingerprint(&tags, bytes) {
            assert_eq!(identity.duration, tags.properties.duration);
            if tags.properties.container != Container::Ogg {
                let size: u64 = tags
                    .payload
                    .byte_ranges()
                    .unwrap()
                    .iter()
                    .map(|range| range.end - range.start)
                    .sum();
                assert_eq!(identity.payload_size, size);
            }
        }
    }
}
