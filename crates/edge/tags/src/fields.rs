use crate::metadata::{Key, ReadFields, Source};
use crate::{Advisory, Performer, ReplayGain, TextValues};

/// Picard/Timbre fields, with no inferred, split or display-only values.
#[derive(Clone, Debug, Default, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct Fields {
    /// Track title.
    pub title: Option<String>,
    /// Album title.
    pub album: Option<String>,
    /// Tagged description or general comment.
    pub comment: Option<String>,
    /// Tagged grouping, used to organize works.
    pub grouping: Option<TextValues>,
    /// Track artist display credit.
    pub artist: Option<TextValues>,
    /// Album artist display credit.
    pub album_artist: Option<TextValues>,
    /// Explicit track artists.
    pub artists: Vec<String>,
    /// Explicit album artists.
    pub album_artists: Vec<String>,
    /// Disc subtitle.
    pub disc_subtitle: Option<String>,
    /// Explicit compilation flag.
    pub compilation: Option<bool>,
    /// `MusicBrainz` recording ID.
    pub musicbrainz_recording_id: Option<String>,
    /// `MusicBrainz` release track ID.
    pub musicbrainz_track_id: Option<String>,
    /// `MusicBrainz` release ID.
    pub musicbrainz_album_id: Option<String>,
    /// `MusicBrainz` release-group ID.
    pub musicbrainz_release_group_id: Option<String>,
    /// `MusicBrainz` artist IDs.
    pub musicbrainz_artist_ids: Vec<String>,
    /// `MusicBrainz` album artist IDs.
    pub musicbrainz_album_artist_ids: Vec<String>,
    /// `MusicBrainz` original release ID.
    pub musicbrainz_original_album_id: Option<String>,
    /// `MusicBrainz` original artist IDs.
    pub musicbrainz_original_artist_ids: Vec<String>,
    /// Tagged release types.
    pub release_types: Vec<String>,
    /// Tagged release status.
    pub release_status: Option<String>,
    /// Tagged release country.
    pub release_country: Option<String>,
    /// Release date, retaining its tagged precision.
    pub date: Option<String>,
    /// Original release date when a date, rather than just a year, is stored.
    pub original_date: Option<String>,
    /// Tagged original release year.
    pub original_year: Option<u32>,
    /// Composer credit.
    pub composer: Option<TextValues>,
    /// Conductor credit.
    pub conductor: Option<TextValues>,
    /// Lyricist credit.
    pub lyricist: Option<TextValues>,
    /// Writer credit.
    pub writer: Option<TextValues>,
    /// Remixer credit.
    pub remixer: Option<TextValues>,
    /// Arranger credit.
    pub arranger: Option<TextValues>,
    /// Engineer credit.
    pub engineer: Option<TextValues>,
    /// Mixer credit.
    pub mixer: Option<TextValues>,
    /// DJ mixer credit.
    pub dj_mixer: Option<TextValues>,
    /// Title of the work.
    pub work: Option<String>,
    /// Movement name.
    pub movement: Option<String>,
    /// Show movement instead of the regular title.
    pub show_movement: Option<bool>,
    /// Explicit title sort text.
    pub title_sort: Option<String>,
    /// Explicit artist sort text.
    pub artist_sort: Option<TextValues>,
    /// Explicit album sort text.
    pub album_sort: Option<String>,
    /// Explicit album artist sort text.
    pub album_artist_sort: Option<TextValues>,
    /// Explicit composer sort text.
    pub composer_sort: Option<TextValues>,
    /// Genre credit.
    pub genre: Option<TextValues>,
    /// Record label.
    pub label: Option<TextValues>,
    /// Tagged mood.
    pub mood: Option<TextValues>,
    /// Beats per minute.
    pub bpm: Option<u32>,
    /// Tagged International Standard Recording Codes.
    pub isrc: Vec<String>,
    /// Tagged album version or edition comment.
    pub album_version: Option<String>,
    /// Content advisory, if tagged.
    pub advisory: Option<Advisory>,
    /// Track number in disc order.
    pub track_number: Option<u32>,
    /// Number of tracks on the disc.
    pub total_tracks: Option<u32>,
    /// Album disc number.
    pub disc_number: Option<u32>,
    /// Number of discs in the album.
    pub total_discs: Option<u32>,
    /// Movement number within the work.
    pub movement_number: Option<u32>,
    /// Number of movements in the work.
    pub movement_total: Option<u32>,
    /// Performers with format-specific instrument credits where available.
    pub performers: Vec<Performer>,
    /// `ReplayGain` and R128 tags, without normalization.
    pub replay_gain: ReplayGain,
}

impl Fields {
    pub(crate) fn read(sources: &[Option<Source<'_>>], opus: bool) -> Self {
        let value = ReadFields(sources);
        Self {
            performers: value.performers(),
            replay_gain: ReplayGain {
                track_gain: value.gain("REPLAYGAIN_TRACK_GAIN"),
                track_peak: value.gain("REPLAYGAIN_TRACK_PEAK"),
                album_gain: value.gain("REPLAYGAIN_ALBUM_GAIN"),
                album_peak: value.gain("REPLAYGAIN_ALBUM_PEAK"),
                r128_track_gain: opus.then(|| value.r128("R128_TRACK_GAIN")).flatten(),
                r128_album_gain: opus.then(|| value.r128("R128_ALBUM_GAIN")).flatten(),
            },
            ..Self::basics(&value)
        }
    }

    fn basics(value: &ReadFields<'_>) -> Self {
        Self {
            title: value.scalar(Key("TIT2", "TITLE", "©nam", "Title", "INAM")),
            album: value.scalar(Key("TALB", "ALBUM", "©alb", "Album", "IPRD")),
            comment: value.scalar(Key("COMM", "COMMENT", "©cmt", "Comment", "ICMT")),
            grouping: value.text(Key("TIT1", "GROUPING", "©grp", "Grouping", "")),
            artist: value.text(Key("TPE1", "ARTIST", "©ART", "Artist", "IART")),
            album_artist: value.text(Key("TPE2", "ALBUMARTIST", "aART", "Album Artist", "")),
            artists: value.list(Key(":Artists", "ARTISTS", ":ARTISTS", "Artists", "")),
            album_artists: value.list(Key(
                ":ALBUMARTISTS",
                "ALBUMARTISTS",
                ":ALBUMARTISTS",
                "ALBUMARTISTS",
                "",
            )),
            disc_subtitle: value.scalar(Key(
                "TSST",
                "DISCSUBTITLE",
                ":DISCSUBTITLE",
                "DiscSubtitle",
                "",
            )),
            compilation: value.flag(Key("TCMP", "COMPILATION", "cpil", "Compilation", "")),
            ..Self::identifiers(value)
        }
    }

    fn identifiers(value: &ReadFields<'_>) -> Self {
        Self {
            musicbrainz_recording_id: value.scalar(Key(
                "@UFID",
                "MUSICBRAINZ_TRACKID",
                ":MusicBrainz Track Id",
                "Musicbrainz_Trackid",
                "",
            )),
            musicbrainz_track_id: value.scalar(Key(
                ":MusicBrainz Release Track Id",
                "MUSICBRAINZ_RELEASETRACKID",
                ":MusicBrainz Release Track Id",
                "Musicbrainz_Releasetrackid",
                "",
            )),
            musicbrainz_album_id: value.scalar(Key(
                ":MusicBrainz Album Id",
                "MUSICBRAINZ_ALBUMID",
                ":MusicBrainz Album Id",
                "MUSICBRAINZ_ALBUMID",
                "",
            )),
            musicbrainz_release_group_id: value.scalar(Key(
                ":MusicBrainz Release Group Id",
                "MUSICBRAINZ_RELEASEGROUPID",
                ":MusicBrainz Release Group Id",
                "MUSICBRAINZ_RELEASEGROUPID",
                "",
            )),
            musicbrainz_artist_ids: value.list(Key(
                ":MusicBrainz Artist Id",
                "MUSICBRAINZ_ARTISTID",
                ":MusicBrainz Artist Id",
                "MUSICBRAINZ_ARTISTID",
                "",
            )),
            musicbrainz_album_artist_ids: value.list(Key(
                ":MusicBrainz Album Artist Id",
                "MUSICBRAINZ_ALBUMARTISTID",
                ":MusicBrainz Album Artist Id",
                "MUSICBRAINZ_ALBUMARTISTID",
                "",
            )),
            musicbrainz_original_album_id: value.scalar(Key(
                ":MusicBrainz Original Album Id",
                "MUSICBRAINZ_ORIGINALALBUMID",
                ":MusicBrainz Original Album Id",
                "",
                "",
            )),
            musicbrainz_original_artist_ids: value.list(Key(
                ":MusicBrainz Original Artist Id",
                "MUSICBRAINZ_ORIGINALARTISTID",
                ":MusicBrainz Original Artist Id",
                "",
                "",
            )),
            release_types: value.list(Key(
                ":MusicBrainz Album Type",
                "RELEASETYPE",
                ":MusicBrainz Album Type",
                "MUSICBRAINZ_ALBUMTYPE",
                "",
            )),
            release_status: value.scalar(Key(
                ":MusicBrainz Album Status",
                "RELEASESTATUS",
                ":MusicBrainz Album Status",
                "MUSICBRAINZ_ALBUMSTATUS",
                "",
            )),
            release_country: value.scalar(Key(
                ":MusicBrainz Album Release Country",
                "RELEASECOUNTRY",
                ":MusicBrainz Album Release Country",
                "Releasecountry",
                "ICNT",
            )),
            ..Self::credits(value)
        }
    }

    fn credits(value: &ReadFields<'_>) -> Self {
        Self {
            date: value.scalar(Key("@TDRC", "DATE", "©day", "Year", "ICRD")),
            original_date: value.scalar(Key("@TDOR", "ORIGINALDATE", "", "", "")),
            original_year: value.number(Key("@TORY", "ORIGINALYEAR", "", "Originalyear", "")),
            composer: value.text(Key("TCOM", "COMPOSER", "©wrt", "Composer", "IMUS")),
            conductor: value.text(Key("TPE3", "CONDUCTOR", ":CONDUCTOR", "Conductor", "")),
            lyricist: value.text(Key("TEXT", "LYRICIST", ":LYRICIST", "Lyricist", "")),
            writer: value.text(Key(":Writer", "WRITER", "", "Writer", "IWRI")),
            remixer: value.text(Key("TPE4", "REMIXER", ":REMIXER", "MixArtist", "")),
            arranger: value.text(Key("#arranger", "ARRANGER", "", "Arranger", "")),
            engineer: value.text(Key(
                "#engineer",
                "ENGINEER",
                ":ENGINEER",
                "Engineer",
                "IENG",
            )),
            mixer: value.text(Key("#mix", "MIXER", ":MIXER", "Mixer", "")),
            dj_mixer: value.text(Key("#DJ-mix", "DJMIXER", ":DJMIXER", "Djmixer", "")),
            work: value.scalar(Key(":WORK|TIT1", "WORK", "©wrk", "Work", "")),
            movement: value.scalar(Key("MVNM", "MOVEMENTNAME", "©mvn", "MOVEMENTNAME", "")),
            show_movement: value.flag(Key(
                ":SHOWMOVEMENT",
                "SHOWMOVEMENT",
                "shwm",
                "SHOWMOVEMENT",
                "",
            )),
            ..Self::descriptors(value)
        }
    }

    fn descriptors(value: &ReadFields<'_>) -> Self {
        Self {
            title_sort: value.scalar(Key("TSOT|:TITLESORT", "TITLESORT", "sonm", "TITLESORT", "")),
            artist_sort: value.text(Key(
                "TSOP|:ARTISTSORT",
                "ARTISTSORT",
                "soar",
                "ARTISTSORT",
                "",
            )),
            album_sort: value.scalar(Key("TSOA|:ALBUMSORT", "ALBUMSORT", "soal", "ALBUMSORT", "")),
            album_artist_sort: value.text(Key(
                "TSO2|:ALBUMARTISTSORT",
                "ALBUMARTISTSORT",
                "soaa",
                "ALBUMARTISTSORT",
                "",
            )),
            composer_sort: value.text(Key(
                "TSOC|:COMPOSERSORT",
                "COMPOSERSORT",
                "soco",
                "COMPOSERSORT",
                "",
            )),
            genre: value.text(Key("TCON", "GENRE", "©gen", "Genre", "IGNR")),
            label: value.text(Key("TPUB", "LABEL", ":LABEL", "Label", "")),
            mood: value.text(Key("TMOO", "MOOD", ":MOOD", "Mood", "")),
            bpm: value.number(Key("TBPM", "BPM", "tmpo", "BPM", "")),
            isrc: value.list(Key("TSRC", "ISRC", ":ISRC", "ISRC", "")),
            album_version: value.scalar(Key(
                ":MUSICBRAINZ_ALBUMCOMMENT",
                "MUSICBRAINZ_ALBUMCOMMENT",
                ":MUSICBRAINZ_ALBUMCOMMENT",
                "MUSICBRAINZ_ALBUMCOMMENT",
                "",
            )),
            advisory: value.advisory(Key(
                ":ITUNESADVISORY",
                "ITUNESADVISORY",
                "rtng",
                "ITUNESADVISORY",
                "",
            )),
            ..Self::numbers(value)
        }
    }

    fn numbers(value: &ReadFields<'_>) -> Self {
        let (track_number, total_tracks) = value.pair(
            Key("TRCK", "TRACKNUMBER", "trkn", "Track", "ITRK"),
            "TRACKTOTAL|TOTALTRACKS",
        );
        let (disc_number, total_discs) = value.pair(
            Key("TPOS", "DISCNUMBER", "disk", "Disc", ""),
            "DISCTOTAL|TOTALDISCS",
        );
        let (movement_number, movement_total) = value.pair(
            Key("MVIN", "MOVEMENT", "©mvi", "MOVEMENT", ""),
            "MOVEMENTTOTAL",
        );
        Self {
            track_number,
            total_tracks,
            disc_number,
            total_discs,
            movement_number,
            movement_total,
            ..Self::default()
        }
    }
}
