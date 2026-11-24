#![expect(
    clippy::unwrap_used,
    reason = "fixture setup failures must fail the test"
)]

//! Caps and guards that keep one hostile file from exhausting time or memory.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use timbre_edge_tags::fixtures::{
    AttachedPictureFrame, Frame, Id3v2Tag, MimeType, Picture, PictureType, TextEncoding,
    TextInformationFrame, WriteOptions, write,
};
use timbre_edge_tags::{read, read_picture};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Sample {
    path: PathBuf,
    dir: PathBuf,
}

impl Sample {
    fn new() -> Self {
        Self::of("tone.mp3")
    }

    fn of(name: &str) -> Self {
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("timbre-limits-{}-{id}", std::process::id()));
        std::fs::create_dir(&dir).unwrap();
        let path = dir.join(name);
        let fixture =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("tests/fixtures/{name}"));
        std::fs::copy(fixture, &path).unwrap();
        Self { path, dir }
    }
}

impl Drop for Sample {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.dir).unwrap();
    }
}

fn frame_id(name: &'static str) -> timbre_edge_tags::fixtures::FrameId<'static> {
    timbre_edge_tags::fixtures::FrameId::Valid(std::borrow::Cow::Borrowed(name))
}

fn picture(bytes: Vec<u8>, kind: PictureType) -> Picture {
    Picture::unchecked(bytes)
        .pic_type(kind)
        .mime_type(MimeType::Png)
        .build()
}

fn syncsafe(size: usize) -> [u8; 4] {
    let size = u32::try_from(size).unwrap();
    [
        u8::try_from((size >> 21) & 0x7f).unwrap(),
        u8::try_from((size >> 14) & 0x7f).unwrap(),
        u8::try_from((size >> 7) & 0x7f).unwrap(),
        u8::try_from(size & 0x7f).unwrap(),
    ]
}

fn id3v2_bytes(frames: &[u8]) -> Vec<u8> {
    let mut tag = vec![b'I', b'D', b'3', 4, 0, 0];
    tag.extend_from_slice(&syncsafe(frames.len()));
    tag.extend_from_slice(frames);
    tag
}

fn raw_frame(id: &str, flags: u16, body: &[u8]) -> Vec<u8> {
    let mut frame = id.as_bytes().to_vec();
    frame.extend_from_slice(&syncsafe(body.len()));
    frame.extend_from_slice(&flags.to_be_bytes());
    frame.extend_from_slice(body);
    frame
}

fn prefixed(sample: &Sample, prefix: &[u8]) {
    let audio = std::fs::read(&sample.path).unwrap();
    let mut bytes = prefix.to_vec();
    bytes.extend_from_slice(&audio);
    std::fs::write(&sample.path, bytes).unwrap();
}

#[test]
fn a_repetitive_picture_is_located_without_a_quadratic_scan() {
    let sample = Sample::new();
    // Eight mebibytes of filler the cover matches byte for byte until its last
    // byte: a window scan then compares the whole cover at every offset.
    let filler = vec![b'a'; 8 * 1024 * 1024];
    let mut cover = vec![b'a'; 1024 * 1024];
    cover.push(b'b');
    let mut tag = Id3v2Tag::default();
    tag.insert(Frame::Picture(AttachedPictureFrame::new(
        TextEncoding::UTF8,
        picture(filler, PictureType::Media),
    )));
    tag.insert(Frame::Picture(AttachedPictureFrame::new(
        TextEncoding::UTF8,
        picture(cover.clone(), PictureType::CoverFront),
    )));
    write(&sample.path, &tag, WriteOptions::default()).unwrap();

    let tags = read(&sample.path).unwrap();
    let started = Instant::now();
    let range = timbre_edge_tags::picture_range(&sample.path, &tags, &cover)
        .unwrap()
        .unwrap();
    assert_eq!(range.end - range.start, 1024 * 1024 + 1);
    assert!(
        started.elapsed().as_secs() < 15,
        "the search compared the cover at every offset"
    );
}

#[test]
fn an_oversized_text_value_is_cut() {
    let sample = Sample::new();
    let mut tag = Id3v2Tag::default();
    tag.insert(Frame::Text(TextInformationFrame::new(
        frame_id("TIT2"),
        TextEncoding::UTF8,
        "t".repeat(200 * 1024),
    )));
    write(&sample.path, &tag, WriteOptions::default()).unwrap();

    let tags = read(&sample.path).unwrap();
    assert_eq!(tags.fields.title.map(|title| title.len()), Some(64 * 1024));
}

#[test]
fn only_sixteen_pictures_are_kept_and_two_carry_bytes() {
    let sample = Sample::new();
    let mut tag = Id3v2Tag::default();
    for index in 0..20u8 {
        let kind = if index == 5 {
            PictureType::CoverFront
        } else {
            PictureType::Other
        };
        tag.insert(Frame::Picture(AttachedPictureFrame::new(
            TextEncoding::UTF8,
            picture(vec![index; 1024], kind),
        )));
    }
    write(&sample.path, &tag, WriteOptions::default()).unwrap();

    let tags = read(&sample.path).unwrap();
    assert_eq!(tags.pictures.len(), 16);
    let carried = tags
        .pictures
        .iter()
        .filter(|picture| !picture.bytes.is_empty())
        .count();
    assert!(carried <= 2, "{carried} pictures were copied");
    assert!(tags.pictures.iter().all(|picture| picture.size == 1024));
    let last = read_picture(&sample.path, 15).unwrap().unwrap();
    assert_eq!(last.bytes.len(), 1024);
}

#[test]
fn a_compressed_frame_rejects_the_whole_tag() {
    let sample = Sample::new();
    let mut frames = raw_frame("TIT2", 0, &[3, b'k', b'e', b'p', b't']);
    frames.extend_from_slice(&raw_frame("TXXX", 0x0009, &[0, 0, 0x7f, 0x7f, 1, 2, 3, 4]));
    prefixed(&sample, &id3v2_bytes(&frames));

    let tags = read(&sample.path).unwrap();
    assert!(tags.rejected_tags);
    assert_eq!(tags.fields.title, None);
}

#[test]
fn a_huge_leading_tag_is_skipped_but_the_audio_is_read() {
    let sample = Sample::new();
    let body = vec![0; 17 * 1024 * 1024];
    prefixed(&sample, &id3v2_bytes(&body));

    // Covers up to 32 MiB are legal, so a tag past the walk limit costs the
    // file its tags, not its audio.
    let tags = read(&sample.path).unwrap();
    assert!(tags.rejected_tags);
    assert_eq!(tags.fields.title, None);
    assert!(tags.properties.duration > std::time::Duration::ZERO);
}

#[test]
fn a_compressed_frame_in_a_wav_chunk_rejects_the_tag() {
    let sample = Sample::of("tone.wav");
    let mut frames = raw_frame("TIT2", 0, &[3, b'k', b'e', b'p', b't']);
    frames.extend_from_slice(&raw_frame("TXXX", 0x0009, &[0, 0, 0x7f, 0x7f, 1, 2, 3, 4]));
    let tag = id3v2_bytes(&frames);
    let mut bytes = std::fs::read(&sample.path).unwrap();
    bytes.extend_from_slice(b"id3 ");
    bytes.extend_from_slice(&u32::try_from(tag.len()).unwrap().to_le_bytes());
    bytes.extend_from_slice(&tag);
    let size = u32::try_from(bytes.len() - 8).unwrap().to_le_bytes();
    bytes[4..8].copy_from_slice(&size);
    std::fs::write(&sample.path, bytes).unwrap();

    // The guard has to look inside the chunk: a hostile frame is no safer
    // for being carried by a RIFF file than by a bare ID3 header.
    let tags = read(&sample.path).unwrap();
    assert!(tags.rejected_tags);
    assert_eq!(tags.fields.title, None);
    assert!(tags.properties.duration > std::time::Duration::ZERO);
}

#[test]
fn an_undecodable_front_cover_does_not_displace_the_usable_one() {
    let sample = Sample::new();
    let mut tag = Id3v2Tag::default();
    tag.insert(Frame::Picture(AttachedPictureFrame::new(
        TextEncoding::UTF8,
        Picture::unchecked(vec![1; 512])
            .pic_type(PictureType::CoverFront)
            .mime_type(MimeType::Gif)
            .build(),
    )));
    tag.insert(Frame::Picture(AttachedPictureFrame::new(
        TextEncoding::UTF8,
        picture(vec![2; 512], PictureType::CoverFront),
    )));
    write(&sample.path, &tag, WriteOptions::default()).unwrap();

    let tags = read(&sample.path).unwrap();
    let carried: Vec<&[u8]> = tags
        .pictures
        .iter()
        .filter(|picture| !picture.bytes.is_empty())
        .map(|picture| picture.bytes.as_slice())
        .collect();
    // A format no decoder here understands is described, never carried: it
    // would otherwise take the one front cover slot from a usable cover.
    assert_eq!(carried, vec![[2; 512].as_slice()]);
}
