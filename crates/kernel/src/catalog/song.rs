use super::metadata::{Credit, ExplicitStatus};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

/// Playback gain tags. Unspecified values are not computed.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ReplayGain {
    /// Track gain in decibels.
    pub track_gain: Option<f64>,
    /// Track peak amplitude ratio.
    pub track_peak: Option<f64>,
    /// Album gain in decibels.
    pub album_gain: Option<f64>,
    /// Album peak amplitude ratio.
    pub album_peak: Option<f64>,
}

/// One named movement in a classical work.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Movement {
    /// Tagged movement title.
    pub name: Option<String>,
    /// Position in the work.
    pub number: Option<u32>,
    /// Total movements in the work.
    pub total: Option<u32>,
}

/// A scanned song and its tag metadata.
#[derive(Clone, Debug, PartialEq)]
pub struct Track {
    /// Opaque track id.
    pub id: String,
    /// Track title.
    pub title: String,
    /// Tagged sort title, if known.
    pub sort_name: Option<String>,
    /// Opaque id of the containing album.
    pub album_id: String,
    /// Display name of the containing album.
    pub album: String,
    /// Display artist text.
    pub display_artist: String,
    /// Display album artist text.
    pub display_album_artist: String,
    /// Display composer text.
    pub display_composer: String,
    /// Ordered artist, album artist and contributor links.
    pub credits: Vec<Credit>,
    /// Tagged track number within the disc.
    pub track_number: Option<u32>,
    /// Tagged or folder-inferred disc number.
    pub disc_number: Option<u32>,
    /// Tagged release year.
    pub year: Option<u32>,
    /// Genre names.
    pub genres: Vec<String>,
    /// `MusicBrainz` recording id.
    pub musicbrainz_id: Option<String>,
    /// Tagged recording identifiers.
    pub isrc: Vec<String>,
    /// Tagged moods.
    pub moods: Vec<String>,
    /// Tempo in beats per minute.
    pub bpm: Option<u32>,
    /// Tagged comment.
    pub comment: Option<String>,
    /// Media type, when known.
    pub media_type: Option<String>,
    /// Tagged content advisory, if known.
    pub explicit_status: Option<ExplicitStatus>,
    /// Tagged movements.
    pub movements: Vec<Movement>,
    /// Tagged groupings.
    pub groupings: Vec<String>,
    /// Tagged work titles.
    pub works: Vec<String>,
    /// Whether tags request showing movement metadata instead of the title.
    pub show_movement: Option<bool>,
    /// Playback gain metadata.
    pub replay_gain: ReplayGain,
    /// Selected artwork id, when available.
    pub cover_art: Option<String>,
    /// Playing time.
    pub duration: Duration,
    /// Encoded file size in bytes.
    pub size: u64,
    /// Audio bit rate in kilobits per second.
    pub bitrate_kbps: Option<u32>,
    /// Samples per second.
    pub sample_rate: Option<u32>,
    /// Number of audio channels.
    pub channels: Option<u8>,
    /// Bits per sample when meaningful for the codec.
    pub bit_depth: Option<u8>,
    /// Filename extension of the indexed source.
    pub suffix: String,
    /// Time when the track was first indexed.
    pub created: SystemTime,
}

/// A transcription in one language and format.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Lyrics {
    /// Language label, if present.
    pub language: Option<String>,
    /// Embedded tag description, if present.
    pub description: Option<String>,
    /// Whether the lines carry timestamps.
    pub synced: bool,
    /// Transcribed lines in display order.
    pub lines: Vec<LyricLine>,
}

/// A timed or untimed line in a transcription.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LyricLine {
    /// When to display this line, relative to audio start.
    pub start: Option<Duration>,
    /// Complete line text. One LRC line repeated at several timestamps shares
    /// its text with every repetition.
    pub text: Arc<str>,
    /// Inline cues in text order.
    pub words: Vec<WordCue>,
}

/// A timestamp aligned with a text fragment in a line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WordCue {
    /// When to display this fragment, relative to audio start.
    pub start: Duration,
    /// Text following the cue, up to the next cue or line end.
    pub text: Arc<str>,
}
