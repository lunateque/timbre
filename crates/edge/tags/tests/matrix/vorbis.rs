use super::{Sample, check_picture, cover};
use timbre_edge_tags::LyricBody;
use timbre_edge_tags::fixtures::{OggPictureStorage, VorbisComments, WriteOptions, write};
use timbre_edge_tags::{Advisory, Fields, Performer, ReplayGain, TextValues, read};

#[expect(
    clippy::too_many_lines,
    reason = "one tag matrix per format reads as a table; splitting it hides the pairs"
)]
#[test]
fn vorbis_matrix_round_trips_native_tags() {
    let mut tag = VorbisComments::new();
    put(&mut tag, "TITLE", &["title-0"]);
    put(&mut tag, "ALBUM", &["album-1"]);
    put(&mut tag, "ARTIST", &["A feat. B"]);
    put(&mut tag, "ALBUMARTIST", &["AC/DC"]);
    put(&mut tag, "ARTISTS", &["artists-4", "artists-104"]);
    put(
        &mut tag,
        "ALBUMARTISTS",
        &["album_artists-5", "album_artists-105"],
    );
    put(&mut tag, "DISCSUBTITLE", &["disc_subtitle-6"]);
    put(&mut tag, "COMPILATION", &["1"]);
    put(
        &mut tag,
        "MUSICBRAINZ_TRACKID",
        &["musicbrainz_recording_id-8"],
    );
    put(
        &mut tag,
        "MUSICBRAINZ_RELEASETRACKID",
        &["musicbrainz_track_id-9"],
    );
    put(
        &mut tag,
        "MUSICBRAINZ_ALBUMID",
        &["musicbrainz_album_id-10"],
    );
    put(
        &mut tag,
        "MUSICBRAINZ_RELEASEGROUPID",
        &["musicbrainz_release_group_id-11"],
    );
    put(
        &mut tag,
        "MUSICBRAINZ_ARTISTID",
        &["musicbrainz_artist_ids-12", "musicbrainz_artist_ids-112"],
    );
    put(
        &mut tag,
        "MUSICBRAINZ_ALBUMARTISTID",
        &[
            "musicbrainz_album_artist_ids-13",
            "musicbrainz_album_artist_ids-113",
        ],
    );
    put(
        &mut tag,
        "MUSICBRAINZ_ORIGINALALBUMID",
        &["musicbrainz_original_album_id-14"],
    );
    put(
        &mut tag,
        "MUSICBRAINZ_ORIGINALARTISTID",
        &[
            "musicbrainz_original_artist_ids-15",
            "musicbrainz_original_artist_ids-115",
        ],
    );
    put(
        &mut tag,
        "RELEASETYPE",
        &["release_types-16", "release_types-116"],
    );
    put(&mut tag, "RELEASESTATUS", &["release_status-17"]);
    put(&mut tag, "RELEASECOUNTRY", &["release_country-18"]);
    put(&mut tag, "DATE", &["2024-08-23"]);
    put(&mut tag, "ORIGINALDATE", &["1982-01-15"]);
    put(&mut tag, "ORIGINALYEAR", &["1982"]);
    put(&mut tag, "COMPOSER", &["composer-22", "composer-122"]);
    put(&mut tag, "CONDUCTOR", &["conductor-23"]);
    put(&mut tag, "LYRICIST", &["lyricist-24"]);
    put(&mut tag, "WRITER", &["writer-25"]);
    put(&mut tag, "REMIXER", &["remixer-26"]);
    put(&mut tag, "ARRANGER", &["arranger-27"]);
    put(&mut tag, "ENGINEER", &["engineer-28"]);
    put(&mut tag, "MIXER", &["mixer-29"]);
    put(&mut tag, "DJMIXER", &["dj_mixer-30"]);
    put(&mut tag, "WORK", &["work-31"]);
    put(&mut tag, "MOVEMENTNAME", &["movement-32"]);
    put(&mut tag, "SHOWMOVEMENT", &["0"]);
    put(&mut tag, "TITLESORT", &["title_sort-34"]);
    put(&mut tag, "ARTISTSORT", &["artist_sort-35"]);
    put(&mut tag, "ALBUMSORT", &["album_sort-36"]);
    put(&mut tag, "ALBUMARTISTSORT", &["album_artist_sort-37"]);
    put(&mut tag, "COMPOSERSORT", &["composer_sort-38"]);
    put(&mut tag, "GENRE", &["genre-39"]);
    put(&mut tag, "LABEL", &["label-40"]);
    put(&mut tag, "MOOD", &["mood-41"]);
    put(&mut tag, "BPM", &["119"]);
    put(&mut tag, "ISRC", &["isrc-43", "isrc-143"]);
    put(&mut tag, "MUSICBRAINZ_ALBUMCOMMENT", &["album_version-44"]);
    put(&mut tag, "ITUNESADVISORY", &["2"]);
    put(&mut tag, "REPLAYGAIN_TRACK_GAIN", &["-7.25 dB"]);
    put(&mut tag, "REPLAYGAIN_TRACK_PEAK", &["0.75"]);
    put(&mut tag, "REPLAYGAIN_ALBUM_GAIN", &["+1.5 dB"]);
    put(&mut tag, "REPLAYGAIN_ALBUM_PEAK", &["0.875"]);
    put(&mut tag, "TRACKNUMBER", &["4"]);
    put(&mut tag, "TRACKTOTAL", &["12"]);
    put(&mut tag, "DISCNUMBER", &["2"]);
    put(&mut tag, "TOTALDISCS", &["3"]);
    put(&mut tag, "MOVEMENT", &["5"]);
    put(&mut tag, "MOVEMENTTOTAL", &["6"]);
    put(&mut tag, "PERFORMER", &["Amy (clarinet)"]);
    put(&mut tag, "R128_TRACK_GAIN", &["-1234"]);
    put(&mut tag, "R128_ALBUM_GAIN", &["678"]);
    tag.insert_picture(cover(), None).unwrap();
    put(&mut tag, "LYRICS", &["plain line"]);
    put(&mut tag, "LYRICS:ja", &["[00:12.34] timing"]);
    put(&mut tag, "COMMENT", &["Comment text"]);
    put(&mut tag, "GROUPING", &["Grouping text"]);
    for name in ["tone.flac", "tone.ogg", "tone.opus"] {
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
                original_year: Some(1982),
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
                comment: Some("Comment text".into()),
                grouping: Some(TextValues::Raw("Grouping text".into())),
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
                    r128_track_gain: (name == "tone.opus").then_some(-1234),
                    r128_album_gain: (name == "tone.opus").then_some(678),
                },
            },
            "{name}"
        );
        check_picture(&tags, Some(3));
        assert_eq!(tags.lyrics.len(), 2);
        assert!(!tags.lyrics[0].synced);
        assert_eq!(tags.lyrics[0].language, None);
        assert!(tags.lyrics[1].synced);
        assert_eq!(tags.lyrics[1].language.as_deref(), Some("ja"));
        assert_eq!(
            tags.lyrics[1].body,
            LyricBody::Text("[00:12.34] timing".into())
        );
    }
}

fn put(tag: &mut VorbisComments, key: &str, values: &[&str]) {
    for value in values {
        tag.push(key.into(), (*value).into());
    }
}
