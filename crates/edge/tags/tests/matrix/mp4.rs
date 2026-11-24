use super::check_plain_lyric;
use super::{Sample, check_picture, cover};
use std::borrow::Cow;
use timbre_edge_tags::fixtures::{Atom, AtomData, AtomIdent, DataType, Ilst, WriteOptions, write};
use timbre_edge_tags::{Advisory, Fields, ReplayGain, TextValues, read};

#[expect(
    clippy::too_many_lines,
    reason = "one tag matrix per format reads as a table; splitting it hides the pairs"
)]
#[test]
fn mp4_matrix_round_trips_native_tags() {
    let mut tag = Ilst::new();
    put(&mut tag, "©nam", &["title-0"]);
    put(&mut tag, "©alb", &["album-1"]);
    put(&mut tag, "©ART", &["A feat. B"]);
    put(&mut tag, "aART", &["AC/DC"]);
    put(&mut tag, ":ARTISTS", &["artists-4", "artists-104"]);
    put(
        &mut tag,
        ":ALBUMARTISTS",
        &["album_artists-5", "album_artists-105"],
    );
    put(&mut tag, ":DISCSUBTITLE", &["disc_subtitle-6"]);
    put(
        &mut tag,
        ":MusicBrainz Track Id",
        &["musicbrainz_recording_id-8"],
    );
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
    put(&mut tag, "©day", &["2024-08-23"]);
    put(&mut tag, "©wrt", &["composer-22", "composer-122"]);
    put(&mut tag, ":CONDUCTOR", &["conductor-23"]);
    put(&mut tag, ":LYRICIST", &["lyricist-24"]);
    put(&mut tag, ":REMIXER", &["remixer-26"]);
    put(&mut tag, ":ENGINEER", &["engineer-28"]);
    put(&mut tag, ":MIXER", &["mixer-29"]);
    put(&mut tag, ":DJMIXER", &["dj_mixer-30"]);
    put(&mut tag, "©wrk", &["work-31"]);
    put(&mut tag, "©mvn", &["movement-32"]);
    put(&mut tag, "sonm", &["title_sort-34"]);
    put(&mut tag, "soar", &["artist_sort-35"]);
    put(&mut tag, "soal", &["album_sort-36"]);
    put(&mut tag, "soaa", &["album_artist_sort-37"]);
    put(&mut tag, "soco", &["composer_sort-38"]);
    put(&mut tag, "©gen", &["genre-39"]);
    put(&mut tag, ":LABEL", &["label-40"]);
    put(&mut tag, ":MOOD", &["mood-41"]);
    put(&mut tag, ":ISRC", &["isrc-43", "isrc-143"]);
    put(&mut tag, ":MUSICBRAINZ_ALBUMCOMMENT", &["album_version-44"]);
    put(&mut tag, ":REPLAYGAIN_TRACK_GAIN", &["-7.25 dB"]);
    put(&mut tag, ":REPLAYGAIN_TRACK_PEAK", &["0.75"]);
    put(&mut tag, ":REPLAYGAIN_ALBUM_GAIN", &["+1.5 dB"]);
    put(&mut tag, ":REPLAYGAIN_ALBUM_PEAK", &["0.875"]);
    for (key, number, total) in [(*b"trkn", 4, 12), (*b"disk", 2, 3)] {
        tag.insert(Atom::new(
            AtomIdent::Fourcc(key),
            AtomData::Unknown {
                code: DataType::Reserved,
                data: vec![0, 0, 0, number, 0, total, 0, 0],
            },
        ));
    }
    for (key, n) in [
        ([0xa9, b'm', b'v', b'i'], 5u32),
        ([0xa9, b'm', b'v', b'c'], 6),
        (*b"tmpo", 119),
        (*b"rtng", 2),
    ] {
        tag.insert(Atom::new(
            AtomIdent::Fourcc(key),
            AtomData::UnsignedInteger(n),
        ));
    }
    tag.insert(Atom::new(AtomIdent::Fourcc(*b"cpil"), AtomData::Bool(true)));
    tag.insert(Atom::new(
        AtomIdent::Fourcc(*b"shwm"),
        AtomData::Bool(false),
    ));
    tag.insert_picture(cover());
    put(&mut tag, "©lyr", &["plain line"]);
    for name in ["tone.m4a", "tone-aac.m4a"] {
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
                composer: Some(TextValues::List(vec![
                    "composer-22".into(),
                    "composer-122".into()
                ])),
                conductor: Some(TextValues::Raw("conductor-23".into())),
                lyricist: Some(TextValues::Raw("lyricist-24".into())),
                remixer: Some(TextValues::Raw("remixer-26".into())),
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
        check_picture(&tags, None);
        check_plain_lyric(&tags);
    }
}

fn put(tag: &mut Ilst, key: &'static str, values: &[&str]) {
    let ident = key.strip_prefix(':').map_or_else(
        || {
            let bytes: Vec<_> = key
                .chars()
                .map(|ch| u8::try_from(u32::from(ch)).unwrap())
                .collect();
            AtomIdent::Fourcc(bytes.try_into().unwrap())
        },
        |name| AtomIdent::Freeform {
            mean: Cow::Borrowed("com.apple.iTunes"),
            name: Cow::Borrowed(name),
        },
    );
    let data = values
        .iter()
        .map(|value| AtomData::UTF8((*value).into()))
        .collect();
    tag.insert(Atom::from_collection(ident, data).unwrap());
}
