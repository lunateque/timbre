#[expect(
    dead_code,
    reason = "each test binary uses a subset of the shared fixture"
)]
mod support;

use std::fs::{self, File};
use std::time::{Duration, SystemTime};
use support::{FIRST, Fixture};
use timbre_edge_tags::fixtures::{
    Accessor, Id3v2Tag, OggPictureStorage, Picture, PictureType, VorbisComments, WriteOptions,
    write,
};
use timbre_kernel::artwork::{Artwork, ImageBody};

const EMBEDDED: &[u8] = include_bytes!("fixtures/embedded.png");
const SIDECAR: &[u8] = include_bytes!("fixtures/cover.jpg");

/// The picture bytes of an image, whichever way the port handed them over.
#[expect(
    clippy::unwrap_used,
    reason = "a cache file that cannot be read must fail the test"
)]
fn body(image: &timbre_kernel::artwork::Image) -> Vec<u8> {
    match &image.body {
        ImageBody::Bytes(bytes) => bytes.clone(),
        ImageBody::CacheFile(file) => {
            let mut bytes = Vec::new();
            std::io::Read::read_to_end(&mut &*file, &mut bytes).unwrap();
            bytes
        }
    }
}

#[test]
fn front_cover_on_any_track_wins_and_old_ids_expire() {
    let library = Fixture::new(&["music"]);
    library.extra("art_cache_mb = 1\n");
    library.flac(
        "release/a.flac",
        &[
            ("TITLE", "First"),
            ("ALBUM", "Release"),
            ("TRACKNUMBER", "1"),
        ],
    );
    library.flac(
        "release/b.flac",
        &[
            ("TITLE", "Second"),
            ("ALBUM", "Release"),
            ("TRACKNUMBER", "2"),
        ],
    );
    let picture = |kind, mut bytes: &[u8]| {
        let mut picture = Picture::from_reader(&mut bytes).unwrap();
        picture.set_pic_type(kind);
        picture
    };
    for (file, title, number, pic) in [
        (
            "release/a.flac",
            "First",
            "1",
            picture(PictureType::CoverBack, SIDECAR),
        ),
        (
            "release/b.flac",
            "Second",
            "2",
            picture(PictureType::CoverFront, EMBEDDED),
        ),
    ] {
        let mut tag = VorbisComments::new();
        for (key, value) in [
            ("TITLE", title),
            ("ALBUM", "Release"),
            ("TRACKNUMBER", number),
        ] {
            tag.push(key.to_owned(), value.to_owned());
        }
        tag.insert_picture(pic, None).unwrap();
        write(&library.path(file), &tag, WriteOptions::default()).unwrap();
    }
    library.write("release/cover.jpg", SIDECAR);
    library.scan(false);
    let ctx = library.ctx();
    let port = ctx.ports().get::<dyn Artwork>().unwrap();
    let id = library.track("First").cover_art.unwrap();
    let original = port.image(&ctx, &id, None).unwrap();
    assert_eq!(original.mime, "image/png");
    assert_eq!(body(&original), EMBEDDED);
    // Eviction runs when a thumbnail is stored, against the size the cache
    // directory had when it was first measured; plant the filler first.
    let dir = library.data("art");
    fs::create_dir_all(&dir).unwrap();
    let old = dir.join(format!("{}-9.jpg", "0".repeat(32)));
    let recent = dir.join(format!("{}-9.jpg", "1".repeat(32)));
    for (file, millis) in [(&old, 1000), (&recent, 2000)] {
        let handle = File::create(file).unwrap();
        handle.set_len(700_000).unwrap();
        handle
            .set_modified(SystemTime::UNIX_EPOCH + Duration::from_millis(millis))
            .unwrap();
    }
    let initial = port.image(&ctx, &id, Some(16)).unwrap();
    assert!(
        matches!(initial.body, ImageBody::Bytes(_)),
        "the thumbnail that was just generated is served from memory"
    );
    let again = port.image(&ctx, &id, Some(16)).unwrap();
    // The second call is served from the cache, through the handle it
    // checked, and must carry the same picture and etag.
    assert!(matches!(again.body, ImageBody::CacheFile(_)));
    assert_eq!(body(&again), body(&initial));
    assert_eq!(again.etag, initial.etag);
    assert!(!old.exists(), "the least recently used file is evicted");
    assert!(recent.exists());
    library.remove("release/a.flac");
    library.remove("release/b.flac");
    library.remove("release/cover.jpg");
    library.flac(
        "release/c.flac",
        &[("TITLE", "Without Cover"), ("ALBUM", "Release")],
    );
    library.scan(false);
    assert_eq!(library.track("Without Cover").cover_art, None);
    let error = port.image(&ctx, &id, None).unwrap_err();
    assert_eq!(error.code().as_str(), "library.not_found");
}

#[test]
fn embedded_original_detects_changes_before_the_next_scan() {
    let library = Fixture::new(&["music"]);
    let file = "album/track.flac";
    library.flac(file, &[("TITLE", "Track")]);
    let mut tag = VorbisComments::new();
    tag.push("TITLE".into(), "Track".into());
    tag.insert_picture(Picture::from_reader(&mut &*EMBEDDED).unwrap(), None)
        .unwrap();
    write(&library.path(file), &tag, WriteOptions::default()).unwrap();
    library.scan(false);
    let ctx = library.ctx();
    let port = ctx.ports().get::<dyn Artwork>().unwrap();
    let id = library.track("Track").cover_art.unwrap();
    assert_eq!(body(&port.image(&ctx, &id, None).unwrap()), EMBEDDED);
    library.flac(file, &[("TITLE", "Track")]);
    assert_eq!(
        port.image(&ctx, &id, None).unwrap_err().code().as_str(),
        "library.not_found"
    );
    assert!(
        library
            .log()
            .contains("\"event\":\"library.art_source_stale\"")
    );
}

#[test]
fn opus_picture_spanning_ogg_pages_is_read_by_tag_locator() {
    let library = Fixture::new(&["music"]);
    let file = "album/track.opus";
    library.opus(file, &[("TITLE", "Opus")]);
    let mut bytes = EMBEDDED.to_vec();
    bytes.resize(100_000, 0x19);
    let mut tag = VorbisComments::new();
    tag.push("TITLE".into(), "Opus".into());
    tag.insert_picture(Picture::from_reader(&mut bytes.as_slice()).unwrap(), None)
        .unwrap();
    write(&library.path(file), &tag, WriteOptions::default()).unwrap();
    let file = fs::read(library.path(file)).unwrap();
    assert!(file.windows(4).filter(|bytes| *bytes == b"OggS").count() >= 4);
    assert_original(&library, "Opus", &bytes);
}

#[test]
fn unsynchronized_id3_picture_is_read_by_tag_locator() {
    let library = Fixture::new(&["music"]);
    let path = library.write(
        "album/track.mp3",
        include_bytes!("../../../edge/tags/tests/fixtures/tone.mp3"),
    );
    let mut tag = Id3v2Tag::default();
    tag.set_title("Unsync".into());
    let mut source = SIDECAR;
    tag.insert_picture(Picture::from_reader(&mut source).unwrap());
    let mut options = WriteOptions::default();
    write(&path, &tag, options.use_id3v23(true)).unwrap();
    let file = fs::read(&path).unwrap();
    assert!(file.starts_with(b"ID3\x03"));
    let end = 10
        + file[6..10]
            .iter()
            .fold(0, |n, byte| (n << 7) | usize::from(*byte));
    let mut encoded = file[..10].to_vec();
    encoded[5] |= 0x80;
    for (index, byte) in file[10..end].iter().copied().enumerate() {
        encoded.push(byte);
        if byte == 0xff
            && file
                .get(11 + index)
                .is_some_and(|&next| next >= 0xe0 || next == 0)
        {
            encoded.push(0);
        }
    }
    let size = encoded.len() - 10;
    for (index, shift) in [21, 14, 7, 0].into_iter().enumerate() {
        encoded[6 + index] = u8::try_from((size >> shift) & 127).unwrap();
    }
    encoded.extend_from_slice(&file[end..]);
    assert!(
        !encoded
            .windows(SIDECAR.len())
            .any(|window| window == SIDECAR)
    );
    fs::write(path, encoded).unwrap();
    assert_original(&library, "Unsync", SIDECAR);
}

#[expect(
    clippy::unwrap_used,
    reason = "shared assertions for both test fixtures"
)]
fn assert_original(library: &Fixture, title: &str, bytes: &[u8]) {
    library.scan(false);
    assert_eq!(
        library.count("library_embedded_art WHERE offset IS NULL"),
        1
    );
    let ctx = library.ctx();
    let port = ctx.ports().get::<dyn Artwork>().unwrap();
    let id = library.track(title).cover_art.unwrap();
    let original = port.image(&ctx, &id, None).unwrap();
    assert_eq!(
        original.etag,
        format!("{:032x}", timbre_edge_image::content_hash(bytes))
    );
    assert_eq!(body(&original), bytes);
}

#[test]
fn synced_sidecar_wins_over_plain_without_discarding_other_languages() {
    let library = Fixture::new(&["music"]);
    library.flac("release/a.flac", &[("TITLE", "Sidecar")]);
    library.write("release/a.txt", b"wrong precedence");
    library.write(
        "release/a.lrc",
        b"[lang:en]\n[offset:50]\n[00:01.000]line <00:01.900>word",
    );
    library.write("release/a.ja.lrc", b"[00:02.000]Japanese");
    library.write("release/unrelated.lrc", b"[00:02.000]not for this track");
    library.flac(
        "release/b.flac",
        &[("TITLE", "Embedded"), ("LYRICS", "embedded first")],
    );
    library.write("release/b.lrc", b"[00:01.000]not embedded");
    library.scan(false);
    let ctx = library.ctx();
    let port = Fixture::catalog(&ctx);
    let lyrics = port
        .lyrics(&ctx, &library.id("Sidecar"), FIRST)
        .unwrap()
        .items;
    assert_eq!(
        lyrics
            .iter()
            .map(|song| (song.language.as_deref(), song.synced))
            .collect::<Vec<_>>(),
        [(Some("en"), true), (Some("ja"), true)]
    );
    assert_eq!(lyrics[0].lines[0].start, Some(Duration::from_millis(1050)));
    assert_eq!(&*lyrics[0].lines[0].text, "line word");
    assert_eq!(
        lyrics[0].lines[0].words[0].start,
        Duration::from_millis(1950)
    );
    assert_eq!(&*lyrics[1].lines[0].text, "Japanese");
    let lyrics = port
        .lyrics(&ctx, &library.id("Embedded"), FIRST)
        .unwrap()
        .items;
    assert_eq!(lyrics.len(), 1);
    assert!(!lyrics[0].synced);
    assert_eq!(&*lyrics[0].lines[0].text, "embedded first");
}
