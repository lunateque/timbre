use super::{Sample, check_picture, cover};
use super::{TEXT_ENCODING, frame_id, people, timestamp};
use timbre_edge_tags::fixtures::{
    BinaryFrame, Frame, Id3v2Tag, SyncTextContentType, SynchronizedTextFrame, TextInformationFrame,
    TimestampFormat, UniqueFileIdentifierFrame, UnsynchronizedTextFrame, WriteOptions, write,
};
use timbre_edge_tags::{Advisory, Fields, Performer, ReplayGain, TextValues, read};
use timbre_edge_tags::{LyricBody, LyricTime};

#[expect(
    clippy::too_many_lines,
    reason = "one tag matrix per format reads as a table; splitting it hides the pairs"
)]
#[test]
fn id3_matrix_round_trips_native_tags() {
    let mut tag = Id3v2Tag::default();
    put(&mut tag, "TIT2", &["title-0"]);
    put(&mut tag, "TALB", &["album-1"]);
    put(&mut tag, "TPE1", &["A feat. B"]);
    put(&mut tag, "TPE2", &["AC/DC"]);
    put(&mut tag, ":Artists", &["artists-4", "artists-104"]);
    put(
        &mut tag,
        ":ALBUMARTISTS",
        &["album_artists-5", "album_artists-105"],
    );
    put(&mut tag, "TSST", &["disc_subtitle-6"]);
    put(&mut tag, "TCMP", &["1"]);
    tag.insert(Frame::UniqueFileIdentifier(UniqueFileIdentifierFrame::new(
        "http://musicbrainz.org",
        b"musicbrainz_recording_id-8".as_slice(),
    )));
    put(
        &mut tag,
        ":MusicBrainz Release Track Id",
        &["musicbrainz_track_id-9"],
    );
    put(
        &mut tag,
        ":MusicBrainz Album Id",
        &["musicbrainz_album_id-10"],
    );
    put(
        &mut tag,
        ":MusicBrainz Release Group Id",
        &["musicbrainz_release_group_id-11"],
    );
    put(
        &mut tag,
        ":MusicBrainz Artist Id",
        &["musicbrainz_artist_ids-12", "musicbrainz_artist_ids-112"],
    );
    put(
        &mut tag,
        ":MusicBrainz Album Artist Id",
        &[
            "musicbrainz_album_artist_ids-13",
            "musicbrainz_album_artist_ids-113",
        ],
    );
    put(
        &mut tag,
        ":MusicBrainz Original Album Id",
        &["musicbrainz_original_album_id-14"],
    );
    put(
        &mut tag,
        ":MusicBrainz Original Artist Id",
        &[
            "musicbrainz_original_artist_ids-15",
            "musicbrainz_original_artist_ids-115",
        ],
    );
    put(
        &mut tag,
        ":MusicBrainz Album Type",
        &["release_types-16", "release_types-116"],
    );
    put(
        &mut tag,
        ":MusicBrainz Album Status",
        &["release_status-17"],
    );
    put(
        &mut tag,
        ":MusicBrainz Album Release Country",
        &["release_country-18"],
    );
    timestamp(&mut tag, "TDRC", "2024-08-23");
    timestamp(&mut tag, "TDOR", "1982-01-15");
    put(&mut tag, "TCOM", &["composer-22", "composer-122"]);
    put(&mut tag, "TPE3", &["conductor-23"]);
    put(&mut tag, "TEXT", &["lyricist-24"]);
    put(&mut tag, ":Writer", &["writer-25"]);
    put(&mut tag, "TPE4", &["remixer-26"]);
    put(&mut tag, ":WORK", &["work-31"]);
    put(&mut tag, "MVNM", &["movement-32"]);
    put(&mut tag, ":SHOWMOVEMENT", &["0"]);
    put(&mut tag, "TSOT", &["title_sort-34"]);
    put(&mut tag, "TSOP", &["artist_sort-35"]);
    put(&mut tag, "TSOA", &["album_sort-36"]);
    put(&mut tag, "TSO2", &["album_artist_sort-37"]);
    put(&mut tag, "TSOC", &["composer_sort-38"]);
    put(&mut tag, "TCON", &["genre-39"]);
    put(&mut tag, "TPUB", &["label-40"]);
    put(&mut tag, "TMOO", &["mood-41"]);
    put(&mut tag, "TBPM", &["119"]);
    put(&mut tag, "TSRC", &["isrc-43", "isrc-143"]);
    put(&mut tag, ":MUSICBRAINZ_ALBUMCOMMENT", &["album_version-44"]);
    put(&mut tag, ":ITUNESADVISORY", &["2"]);
    put(&mut tag, ":REPLAYGAIN_TRACK_GAIN", &["-7.25 dB"]);
    put(&mut tag, ":REPLAYGAIN_TRACK_PEAK", &["0.75"]);
    put(&mut tag, ":REPLAYGAIN_ALBUM_GAIN", &["+1.5 dB"]);
    put(&mut tag, ":REPLAYGAIN_ALBUM_PEAK", &["0.875"]);
    tag.insert(Frame::UniqueFileIdentifier(UniqueFileIdentifierFrame::new(
        "other-owner",
        b"wrong-id".as_slice(),
    )));
    people(
        &mut tag,
        "TIPL",
        &[
            ("arranger", "arranger-27"),
            ("engineer", "engineer-28"),
            ("mix", "mixer-29"),
            ("DJ-mix", "dj_mixer-30"),
        ],
    );
    people(&mut tag, "TMCL", &[("clarinet", "Amy")]);
    put(&mut tag, "TRCK", &["4/12"]);
    put(&mut tag, "TPOS", &["2/3"]);
    put(&mut tag, "MVIN", &["5/6"]);
    tag.insert_picture(cover());
    tag.insert(Frame::UnsynchronizedText(UnsynchronizedTextFrame::new(
        TEXT_ENCODING,
        *b"spa",
        "caption",
        "line",
    )));
    let timed = SynchronizedTextFrame::new(
        TEXT_ENCODING,
        *b"deu",
        TimestampFormat::MS,
        SyncTextContentType::Lyrics,
        Some("timed caption".into()),
        vec![(1_205, "Zeile".into())],
    );
    tag.insert(Frame::Binary(BinaryFrame::new(
        frame_id("SYLT"),
        timed.as_bytes(WriteOptions::default()).unwrap(),
    )));
    for name in ["tone.mp3", "tone.wav", "tone.aiff"] {
        let sample = Sample::new(name);
        write(&sample.path, &tag, WriteOptions::default()).unwrap();
        let tags = read(&sample.path).unwrap();
        assert_eq!(
            tags.fields,
            Fields {
                title: Some("title-0".into()),
                album: Some("album-1".into()),
                artist: Some(TextValues::Raw("A feat. B".into())),
                album_artist: Some(TextValues::Raw("AC/DC".into())),
                artists: vec!["artists-4".into(), "artists-104".into()],
                album_artists: vec!["album_artists-5".into(), "album_artists-105".into()],
                disc_subtitle: Some("disc_subtitle-6".into()),
                compilation: Some(true),
                musicbrainz_recording_id: Some("musicbrainz_recording_id-8".into()),
                musicbrainz_track_id: Some("musicbrainz_track_id-9".into()),
                musicbrainz_album_id: Some("musicbrainz_album_id-10".into()),
                musicbrainz_release_group_id: Some("musicbrainz_release_group_id-11".into()),
                musicbrainz_artist_ids: vec![
                    "musicbrainz_artist_ids-12".into(),
                    "musicbrainz_artist_ids-112".into()
                ],
                musicbrainz_album_artist_ids: vec![
                    "musicbrainz_album_artist_ids-13".into(),
                    "musicbrainz_album_artist_ids-113".into()
                ],
                musicbrainz_original_album_id: Some("musicbrainz_original_album_id-14".into()),
                musicbrainz_original_artist_ids: vec![
                    "musicbrainz_original_artist_ids-15".into(),
                    "musicbrainz_original_artist_ids-115".into()
                ],
                release_types: vec!["release_types-16".into(), "release_types-116".into()],
                release_status: Some("release_status-17".into()),
                release_country: Some("release_country-18".into()),
                date: Some("2024-08-23".into()),
                original_date: Some("1982-01-15".into()),
                composer: Some(TextValues::List(vec![
                    "composer-22".into(),
                    "composer-122".into()
                ])),
                conductor: Some(TextValues::Raw("conductor-23".into())),
                lyricist: Some(TextValues::Raw("lyricist-24".into())),
                writer: Some(TextValues::Raw("writer-25".into())),
                remixer: Some(TextValues::Raw("remixer-26".into())),
                arranger: Some(TextValues::Raw("arranger-27".into())),
                engineer: Some(TextValues::Raw("engineer-28".into())),
                mixer: Some(TextValues::Raw("mixer-29".into())),
                dj_mixer: Some(TextValues::Raw("dj_mixer-30".into())),
                work: Some("work-31".into()),
                movement: Some("movement-32".into()),
                show_movement: Some(false),
                title_sort: Some("title_sort-34".into()),
                artist_sort: Some(TextValues::Raw("artist_sort-35".into())),
                album_sort: Some("album_sort-36".into()),
                album_artist_sort: Some(TextValues::Raw("album_artist_sort-37".into())),
                composer_sort: Some(TextValues::Raw("composer_sort-38".into())),
                genre: Some(TextValues::Raw("genre-39".into())),
                label: Some(TextValues::Raw("label-40".into())),
                mood: Some(TextValues::Raw("mood-41".into())),
                bpm: Some(119),
                isrc: vec!["isrc-43".into(), "isrc-143".into()],
                album_version: Some("album_version-44".into()),
                advisory: Some(Advisory::Clean),
                track_number: Some(4),
                total_tracks: Some(12),
                disc_number: Some(2),
                total_discs: Some(3),
                movement_number: Some(5),
                movement_total: Some(6),
                performers: vec![Performer {
                    name: "Amy".into(),
                    instrument: Some("clarinet".into())
                }],
                replay_gain: ReplayGain {
                    track_gain: Some(-7.25),
                    track_peak: Some(0.75),
                    album_gain: Some(1.5),
                    album_peak: Some(0.875),
                    r128_track_gain: None,
                    r128_album_gain: None,
                },
                ..Fields::default()
            },
            "{name}"
        );
        check_picture(&tags, Some(3));
        let plain = tags.lyrics.iter().find(|lyric| !lyric.synced).unwrap();
        assert_eq!(plain.language.as_deref(), Some("spa"));
        assert_eq!(plain.description.as_deref(), Some("caption"));
        assert_eq!(plain.body, LyricBody::Text("line".into()));
        let synced = tags.lyrics.iter().find(|lyric| lyric.synced).unwrap();
        assert_eq!(synced.language.as_deref(), Some("deu"));
        assert_eq!(synced.description.as_deref(), Some("timed caption"));
        assert_eq!(
            synced.body,
            LyricBody::Timed {
                unit: LyricTime::Milliseconds,
                entries: vec![(1_205, "Zeile".into())]
            }
        );
    }
}

fn put(tag: &mut Id3v2Tag, key: &'static str, values: &[&str]) {
    let text = values.join("\0");
    if let Some(description) = key.strip_prefix(':') {
        tag.insert_user_text(description.into(), text);
    } else {
        tag.insert(Frame::Text(TextInformationFrame::new(
            frame_id(key),
            TEXT_ENCODING,
            text,
        )));
    }
}
