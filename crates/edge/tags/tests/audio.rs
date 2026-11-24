#![cfg(feature = "fixtures")]
// The fixtures and the hand-written Ogg pages live in helpers outside
// `#[test]`, where clippy's test exemptions do not apply.
#![expect(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    reason = "test harness: a broken fixture must fail the test loudly"
)]

mod common;
mod matrix;
mod ogg_fingerprint;

use std::io::{Read, Seek, SeekFrom, Write};
use std::time::Duration;

use common::Sample;
use timbre_edge_tags::fixtures::{Id3v1Tag, Tag, TagType, WriteOptions, write};
use timbre_edge_tags::{Codec, Container, DurationSource, fingerprint, read};

#[test]
fn identifies_supported_containers_without_confusing_the_codec() {
    for (name, codec, container, rate) in [
        ("tone.mp3", Codec::Mp3, Container::Mpeg, 44_100),
        ("tone.flac", Codec::Flac, Container::Flac, 8_000),
        ("tone.ogg", Codec::Vorbis, Container::Ogg, 44_100),
        ("tone.opus", Codec::Opus, Container::Ogg, 48_000),
        ("tone.m4a", Codec::Alac, Container::Mp4, 8_000),
        ("tone-aac.m4a", Codec::Aac, Container::Mp4, 44_100),
        ("tone.wav", Codec::Pcm, Container::Wav, 8_000),
        ("tone.aiff", Codec::Pcm, Container::Aiff, 8_000),
        ("tone.wv", Codec::WavPack, Container::WavPack, 8_000),
        ("tone.ape", Codec::MonkeyAudio, Container::Ape, 8_000),
    ] {
        let sample = Sample::new(name);
        let tags = read(&sample.path).unwrap();
        assert_eq!(tags.properties.codec, codec, "{name}");
        assert_eq!(tags.properties.container, container, "{name}");
        assert_eq!(tags.properties.sample_rate, Some(rate), "{name}");
        assert_eq!(tags.properties.channels, Some(1), "{name}");
        assert!(tags.properties.bitrate_kbps.unwrap() > 0, "{name}");
        let expected = Duration::from_secs(1);
        assert!(
            tags.properties.duration.abs_diff(expected) < Duration::from_millis(55),
            "{name}: {:?}",
            tags.properties.duration
        );
        assert!(tags.payload.bounds().end <= std::fs::metadata(&sample.path).unwrap().len());
    }
}

#[test]
fn uses_encoder_count_and_walks_frames_only_without_it() {
    let no_xing = Sample::new("tone.mp3");
    let xing = Sample::new("tone-xing.mp3");
    let probed = read(&no_xing.path).unwrap();
    let counted = read(&xing.path).unwrap();
    assert_eq!(probed.properties.duration_source, DurationSource::Frames);
    assert_eq!(counted.properties.duration_source, DurationSource::Encoder);
    assert_eq!(counted.properties.duration, Duration::from_secs(1));
    assert_eq!(
        probed.properties.duration,
        Duration::from_nanos(1_044_897_959)
    );
}

#[test]
fn ignores_leading_id3v2_and_trailing_ape_and_id3v1() {
    let sample = Sample::new("tone.mp3");
    let before = read(&sample.path).unwrap();
    let initial = fingerprint(&sample.path, &before).unwrap();
    sample.set_title(TagType::Id3v2, "edited");
    sample.set_title(TagType::Ape, "legacy ape tag");
    write(
        &sample.path,
        &Id3v1Tag::from(Tag::new(TagType::Id3v1)),
        WriteOptions::default(),
    )
    .unwrap();
    let after = read(&sample.path).unwrap();
    let physical = after.payload.bounds();
    let file = std::fs::read(&sample.path).unwrap();
    assert_eq!(&file[..3], b"ID3");
    assert_eq!(&file[file.len() - 128..file.len() - 125], b"TAG");
    assert_eq!(&file[file.len() - 160..file.len() - 152], b"APETAGEX");
    assert!(physical.start > 10);
    assert!(physical.end < u64::try_from(file.len() - 160).unwrap());
    assert_eq!(physical.end - physical.start, initial.payload_size);
    assert_eq!(fingerprint(&sample.path, &after).unwrap(), initial);
    change_byte(&sample.path, physical.start + 20);
    assert_ne!(fingerprint(&sample.path, &after).unwrap(), initial);
}

#[test]
fn fingerprint_ignores_mp4_metadata_moved_ahead_of_the_samples() {
    let sample = Sample::new("tone-aac.m4a");
    let before = read(&sample.path).unwrap();
    let initial = fingerprint(&sample.path, &before).unwrap();
    sample.set_title(TagType::Mp4Ilst, &"cover title ".repeat(8000));
    let after = read(&sample.path).unwrap();
    assert!(after.payload.bounds().start > before.payload.bounds().start);
    assert_eq!(fingerprint(&sample.path, &after).unwrap(), initial);
    change_byte(&sample.path, after.payload.bounds().start + 20);
    assert_ne!(fingerprint(&sample.path, &after).unwrap(), initial);
}

fn change_byte(path: &std::path::Path, offset: u64) {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .read(true)
        .open(path)
        .unwrap();
    file.seek(SeekFrom::Start(offset)).unwrap();
    let mut byte = [0];
    file.read_exact(&mut byte).unwrap();
    file.seek(SeekFrom::Start(offset)).unwrap();
    file.write_all(&[byte[0] ^ 1]).unwrap();
}
