mod ape;
mod id3;
mod mp4;
mod vorbis;

use std::borrow::Cow;

use crate::common::Sample;
use timbre_edge_tags::fixtures::{
    Frame, FrameId, Id3v1Tag, Id3v2Tag, KeyValueFrame, Picture, PictureType, RiffInfoList,
    TextEncoding, TextInformationFrame, TimestampFrame, WriteOptions, write,
};
use timbre_edge_tags::{Fields, FileTags, LyricBody, Performer, TextValues, read};

const ART: &[u8] = include_bytes!("../fixtures/cover.png");
const TEXT_ENCODING: TextEncoding = TextEncoding::UTF8;

fn cover() -> Picture {
    let mut bytes = ART;
    let mut picture = Picture::from_reader(&mut bytes).unwrap();
    picture.set_pic_type(PictureType::CoverFront);
    picture
}

fn check_picture(tags: &FileTags, kind: Option<u8>) {
    assert_eq!(tags.pictures.len(), 1);
    assert_eq!(tags.pictures[0].kind, kind);
    assert_eq!(tags.pictures[0].mime.as_deref(), Some("image/png"));
    assert_eq!(tags.pictures[0].bytes, ART);
}

fn check_plain_lyric(tags: &FileTags) {
    assert_eq!(tags.lyrics.len(), 1);
    assert_eq!(tags.lyrics[0].body, LyricBody::Text("plain line".into()));
    assert_eq!(tags.lyrics[0].language, None);
    assert!(!tags.lyrics[0].synced);
}

fn frame_id(name: &'static str) -> FrameId<'static> {
    FrameId::Valid(Cow::Borrowed(name))
}

fn people(tag: &mut Id3v2Tag, frame: &'static str, entries: &[(&'static str, &'static str)]) {
    let pairs = entries
        .iter()
        .map(|(role, name)| (Cow::Borrowed(*role), Cow::Borrowed(*name)))
        .collect::<Vec<_>>();
    tag.insert(Frame::KeyValue(KeyValueFrame::new(
        frame_id(frame),
        TEXT_ENCODING,
        pairs,
    )));
}

fn timestamp(tag: &mut Id3v2Tag, frame: &'static str, date: &str) {
    tag.insert(Frame::Timestamp(TimestampFrame::new(
        frame_id(frame),
        TEXT_ENCODING,
        date.parse().unwrap(),
    )));
}

#[test]
fn old_id3_keeps_legacy_text_raw_and_resolves_legacy_dates_and_credits() {
    let mut tag = Id3v2Tag::default();
    tag.insert(Frame::Text(TextInformationFrame::new(
        frame_id("TPE1"),
        TEXT_ENCODING,
        "A feat. B",
    )));
    tag.insert(Frame::Text(TextInformationFrame::new(
        frame_id("TPE2"),
        TEXT_ENCODING,
        "AC/DC",
    )));
    tag.insert(Frame::Text(TextInformationFrame::new(
        frame_id("TIT1"),
        TEXT_ENCODING,
        "extended group",
    )));
    for (key, text) in [
        ("TITLESORT", "sort title"),
        ("ARTISTSORT", "sort artist"),
        ("ALBUMSORT", "sort album"),
        ("ALBUMARTISTSORT", "sort album artist"),
        ("COMPOSERSORT", "sort composer"),
    ] {
        tag.insert_user_text(key.into(), text.into());
    }
    timestamp(&mut tag, "TDRC", "2011-02-03");
    timestamp(&mut tag, "TDOR", "1999");
    people(&mut tag, "TIPL", &[("engineer", "Jane"), ("cello", "Sue")]);
    let sample = Sample::new("tone.mp3");
    write(&sample.path, &tag, WriteOptions::default().use_id3v23(true)).unwrap();
    let bytes = std::fs::read(&sample.path).unwrap();
    for frame in [b"TYER", b"TDAT", b"TORY", b"IPLS"] {
        assert!(bytes.windows(4).any(|window| window == frame), "{frame:?}");
    }
    let f = read(&sample.path).unwrap().fields;
    assert_eq!(f.artist, Some(TextValues::Raw("A feat. B".into())));
    assert_eq!(f.album_artist, Some(TextValues::Raw("AC/DC".into())));
    assert_eq!(f.date.as_deref(), Some("2011-02-03"));
    assert_eq!(f.original_year, Some(1999));
    assert_eq!(f.original_date, None);
    assert_eq!(f.engineer, Some(TextValues::Raw("Jane".into())));
    assert_eq!(
        f.performers,
        vec![Performer {
            name: "Sue".into(),
            instrument: Some("cello".into())
        }]
    );
    assert_eq!(f.work.as_deref(), Some("extended group"));
    assert_eq!(f.title_sort.as_deref(), Some("sort title"));
    assert_eq!(f.artist_sort, Some(TextValues::Raw("sort artist".into())));
    assert_eq!(f.album_sort.as_deref(), Some("sort album"));
    assert_eq!(
        f.album_artist_sort,
        Some(TextValues::Raw("sort album artist".into()))
    );
    assert_eq!(
        f.composer_sort,
        Some(TextValues::Raw("sort composer".into()))
    );
}

#[test]
fn riff_info_is_a_basic_field_fallback_to_embedded_id3() {
    let sample = Sample::new("tone.wav");
    let mut tag = RiffInfoList::new();
    for (key, value) in [
        ("INAM", "RIFF title"),
        ("IPRD", "RIFF album"),
        ("IART", "RIFF artist"),
        ("ITRK", "4"),
        ("ICNT", "US"),
        ("ICRD", "1995-02-24"),
        ("IMUS", "composer"),
        ("IWRI", "writer"),
        ("IENG", "engineer"),
        ("IGNR", "genre"),
    ] {
        tag.insert(key.into(), value.into());
    }
    write(&sample.path, &tag, WriteOptions::default()).unwrap();
    let expected = Fields {
        title: Some("RIFF title".into()),
        album: Some("RIFF album".into()),
        artist: Some(TextValues::Raw("RIFF artist".into())),
        track_number: Some(4),
        release_country: Some("US".into()),
        date: Some("1995-02-24".into()),
        composer: Some(TextValues::Raw("composer".into())),
        writer: Some(TextValues::Raw("writer".into())),
        engineer: Some(TextValues::Raw("engineer".into())),
        genre: Some(TextValues::Raw("genre".into())),
        ..Fields::default()
    };
    assert_eq!(read(&sample.path).unwrap().fields, expected);
    sample.set_title(
        timbre_edge_tags::fixtures::TagType::Id3v2,
        "Preferred title",
    );
    assert_eq!(
        read(&sample.path).unwrap().fields,
        Fields {
            title: Some("Preferred title".into()),
            ..expected
        }
    );
}

#[test]
fn id3v1_fills_missing_mpeg_basics() {
    let sample = Sample::new("tone.mp3");
    let legacy = Id3v1Tag {
        title: Some("legacy title".into()),
        artist: Some("AC/DC".into()),
        album: Some("legacy album".into()),
        year: Some(1992),
        track_number: Some(5),
        genre: Some(17),
        ..Id3v1Tag::default()
    };
    write(&sample.path, &legacy, WriteOptions::default()).unwrap();
    sample.set_title(
        timbre_edge_tags::fixtures::TagType::Id3v2,
        "preferred title",
    );
    assert_eq!(
        read(&sample.path).unwrap().fields,
        Fields {
            title: Some("preferred title".into()),
            artist: Some(TextValues::Raw("AC/DC".into())),
            album: Some("legacy album".into()),
            date: Some("1992".into()),
            track_number: Some(5),
            genre: Some(TextValues::Raw("Rock".into())),
            ..Fields::default()
        }
    );
}
