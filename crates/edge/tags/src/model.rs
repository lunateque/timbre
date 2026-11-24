use std::ops::Range;
use std::time::Duration;

/// File container, distinct from the codec used to encode its audio.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum Container {
    /// MPEG audio elementary stream.
    Mpeg,
    /// Native FLAC.
    Flac,
    /// Ogg pages.
    Ogg,
    /// ISO base media / MPEG-4.
    Mp4,
    /// RIFF WAVE.
    Wav,
    /// AIFF or AIFF-C.
    Aiff,
    /// Native `WavPack`.
    WavPack,
    /// Native Monkey's Audio.
    Ape,
}

/// Audio codec.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum Codec {
    /// MPEG Layer III.
    Mp3,
    /// FLAC.
    Flac,
    /// Vorbis.
    Vorbis,
    /// Opus.
    Opus,
    /// AAC.
    Aac,
    /// Apple Lossless.
    Alac,
    /// Integer or floating-point PCM.
    Pcm,
    /// WAVE codec identified by its numeric format tag.
    Wav(u16),
    /// AIFF-C codec identified by its `FourCC`.
    Aiff([u8; 4]),
    /// `WavPack`.
    WavPack,
    /// Monkey's Audio.
    MonkeyAudio,
}

/// How the duration was determined.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum DurationSource {
    /// Container sample count or duration.
    Container,
    /// Xing, Info or VBRI header (gapless trim when present).
    Encoder,
    /// Sum of samples in consecutive MPEG frames.
    Frames,
    /// A bitrate estimate because a reliable frame count was unavailable.
    Approximate,
}

/// Stream properties, before any display or import policy is applied.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct Properties {
    /// Audio codec.
    pub codec: Codec,
    /// File container.
    pub container: Container,
    /// Playing time.
    pub duration: Duration,
    /// Duration provenance (including whether it is approximate).
    pub duration_source: DurationSource,
    /// Audio bitrate, in kilobits per second, when known.
    pub bitrate_kbps: Option<u32>,
    /// Samples per second, when known.
    pub sample_rate: Option<u32>,
    /// Channel count, when known.
    pub channels: Option<u8>,
    /// Bits per sample, when the format has a meaningful depth.
    pub bit_depth: Option<u8>,
}

/// Location of the audio payload in the file.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AudioPayload(pub(crate) crate::payload::Layout);

impl AudioPayload {
    /// Physical file bounds of the audio, excluding leading and trailing tags.
    /// These bounds enclose all segments; internal MP4 boxes and Ogg page
    /// framing are not part of the logical payload used for fingerprinting.
    pub fn bounds(&self) -> Range<u64> {
        self.0.bounds()
    }

    /// Extents of contiguous audio bytes, if representable without walking Ogg pages.
    pub fn byte_ranges(&self) -> Option<&[Range<u64>]> {
        self.0.byte_ranges()
    }
}

/// Identity of the audio payload independently of mutable metadata.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Fingerprint {
    /// Payload bytes, or playable sample frames for Ogg.
    pub payload_size: u64,
    /// Playing time used in the fingerprint.
    pub duration: Duration,
    /// XXH3-128 of the logical audio windows.
    pub hash: u128,
}

/// Text from a legacy single value or a format's structured list.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum TextValues {
    /// A legacy value whose separators have not been interpreted.
    Raw(String),
    /// Values carried separately by the tag; no heuristic split was used.
    List(Vec<String>),
}

/// Embedded performer credit.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct Performer {
    /// Performer name, as stored.
    pub name: String,
    /// Instrument if the format identifies one.
    pub instrument: Option<String>,
}

/// `ReplayGain` and raw Opus R128 keys.
#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct ReplayGain {
    /// `ReplayGain` track gain in dB.
    pub track_gain: Option<f64>,
    /// `ReplayGain` track peak ratio.
    pub track_peak: Option<f64>,
    /// `ReplayGain` album gain in dB.
    pub album_gain: Option<f64>,
    /// `ReplayGain` album peak ratio.
    pub album_peak: Option<f64>,
    /// Original signed R128 track value (Q7.8, not converted to dB).
    pub r128_track_gain: Option<i16>,
    /// Original signed R128 album value (Q7.8, not converted to dB).
    pub r128_album_gain: Option<i16>,
}

/// Advisory stored by the tag. Absence is represented by `Option`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum Advisory {
    /// Tagged as having no advisory.
    None,
    /// Explicit content.
    Explicit,
    /// Clean version.
    Clean,
    /// Unrecognized advisory code.
    Other(u8),
}

/// Embedded cover or other picture.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Picture {
    /// APIC picture type number, or absent when the format carries no type.
    pub kind: Option<u8>,
    /// Media type, when known.
    pub mime: Option<String>,
    /// Encoded size, even when the bytes were not carried out of the reader.
    pub size: usize,
    /// Encoded picture bytes, empty unless this picture is a cover candidate.
    /// [`crate::read_picture`] returns the bytes of any single picture.
    pub bytes: Vec<u8>,
}

/// Timestamp unit in a binary ID3 synchronized lyrics frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum LyricTime {
    /// Milliseconds from the beginning of the audio.
    Milliseconds,
    /// MPEG frame count from the beginning of the audio.
    MpegFrames,
}

/// Lyric contents before presentation or LRC parsing.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum LyricBody {
    /// Plain text, or original LRC text when `synced` is set.
    Text(String),
    /// Native timed ID3 entries.
    Timed {
        /// Timestamp unit as stored by ID3.
        unit: LyricTime,
        /// Timestamp and lyric fragments in tag order.
        entries: Vec<(u32, String)>,
    },
}

/// An embedded lyric with per-frame/comment language information.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct Lyric {
    /// Language, when the format stores it.
    pub language: Option<String>,
    /// Description, when stored independently of the text.
    pub description: Option<String>,
    /// Whether this lyric contains timestamps.
    pub synced: bool,
    /// Original text or native timed entries.
    pub body: LyricBody,
}

/// Parsed tags and audio bounds from one read of a file.
#[derive(Clone, Debug, PartialEq)]
pub struct FileTags {
    /// Container and codec properties.
    pub properties: Properties,
    /// Location of the audio within the file.
    pub payload: AudioPayload,
    /// Typed metadata fields.
    pub fields: crate::Fields,
    /// Embedded pictures, in tag order.
    pub pictures: Vec<Picture>,
    /// Embedded lyrics, in tag order.
    pub lyrics: Vec<Lyric>,
    /// A tag was skipped because decoding it is unsafe, so these fields may be
    /// missing values the file does carry.
    pub rejected_tags: bool,
}

/// The container a probed file type names.
pub(crate) fn container_of(kind: lofty::file::FileType) -> timbre_kernel::Fallible<Container> {
    use lofty::file::FileType;
    match kind {
        FileType::Mpeg => Ok(Container::Mpeg),
        FileType::Flac => Ok(Container::Flac),
        FileType::Vorbis | FileType::Opus => Ok(Container::Ogg),
        FileType::Mp4 => Ok(Container::Mp4),
        FileType::Wav => Ok(Container::Wav),
        FileType::Aiff => Ok(Container::Aiff),
        FileType::WavPack => Ok(Container::WavPack),
        FileType::Ape => Ok(Container::Ape),
        _ => Err(Box::new(crate::failures::TagFailure::Unsupported)),
    }
}
