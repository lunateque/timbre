use lofty::ape::ApeTag;
use lofty::id3::v2::{
    Frame, Id3v2Tag, SyncTextContentType, SynchronizedTextFrame, TimestampFormat,
};
use lofty::mp4::{AtomData, AtomIdent, Ilst};
use lofty::ogg::OggPictureStorage;
use lofty::ogg::tag::VorbisComments;
use lofty::picture::Picture as NativePicture;
use lofty::tag::ItemValue;

use crate::limits;
use crate::{Lyric, LyricBody, LyricTime, Picture};

/// Which pictures keep their bytes. A file may carry sixteen covers and only
/// one is ever used, so the rest are described without being copied.
pub(crate) enum Wanted {
    /// The first picture and the first typed front cover.
    Candidates,
    /// Only the picture at this tag position.
    Index(usize),
}

pub(crate) struct Embedded {
    pub(crate) pictures: Vec<Picture>,
    pub(crate) lyrics: Vec<Lyric>,
    wanted: Wanted,
    kept_any: bool,
    kept_front: bool,
}

impl Default for Embedded {
    fn default() -> Self {
        Self {
            pictures: Vec::new(),
            lyrics: Vec::new(),
            wanted: Wanted::Candidates,
            kept_any: false,
            kept_front: false,
        }
    }
}

const FRONT_COVER: u8 = 3;
/// Media types the image edge crate decodes. An unsupported picture is still
/// described, but it never takes the candidate slot a usable cover needs.
const SUPPORTED: [&str; 3] = ["image/jpeg", "image/png", "image/webp"];

impl Embedded {
    pub(crate) fn wanting(wanted: Wanted) -> Self {
        Self {
            wanted,
            ..Self::default()
        }
    }

    pub(crate) fn picture(&mut self, picture: NativePicture, typed: bool) {
        let position = self.pictures.len();
        if picture.data().is_empty() || position >= limits::MAX_PICTURES {
            return;
        }
        let kind = typed.then(|| picture.pic_type().as_u8());
        let size = picture.data().len();
        let mime = picture
            .mime_type()
            .map(|mime| limits::text(mime.as_str().to_owned(), limits::MAX_TEXT_BYTES));
        let usable = mime
            .as_deref()
            .is_some_and(|mime| SUPPORTED.contains(&mime.to_ascii_lowercase().as_str()));
        let wanted = match self.wanted {
            Wanted::Index(index) => index == position,
            Wanted::Candidates => {
                usable && (!self.kept_any || (kind == Some(FRONT_COVER) && !self.kept_front))
            }
        };
        let keep = wanted && size <= limits::MAX_PICTURE_BYTES;
        self.kept_any |= keep;
        self.kept_front |= keep && kind == Some(FRONT_COVER);
        self.pictures.push(Picture {
            kind,
            mime,
            size,
            bytes: if keep {
                picture.into_data()
            } else {
                Vec::new()
            },
        });
    }

    pub(crate) fn id3(&mut self, tag: Id3v2Tag) {
        for frame in tag {
            match frame {
                Frame::Picture(frame) => self.picture(frame.picture.into_owned(), true),
                Frame::UnsynchronizedText(frame) => self.text(
                    frame.content.into_owned(),
                    language(frame.language),
                    Some(frame.description.into_owned()),
                    false,
                ),
                Frame::Binary(frame) if frame.id().as_str() == "SYLT" => {
                    if let Ok(mut frame) = SynchronizedTextFrame::parse(&frame.data, frame.flags())
                        && frame.content_type == SyncTextContentType::Lyrics
                        && self.lyrics.len() < limits::MAX_LYRICS
                    {
                        let unit = match frame.timestamp_format {
                            TimestampFormat::MPEG => LyricTime::MpegFrames,
                            TimestampFormat::MS => LyricTime::Milliseconds,
                        };
                        frame.content.truncate(limits::MAX_CUES);
                        for (_, text) in &mut frame.content {
                            let cue = std::mem::take(text);
                            *text = limits::text(cue, limits::MAX_TEXT_BYTES);
                        }
                        self.lyrics.push(Lyric {
                            language: language(frame.language),
                            description: frame.description,
                            synced: true,
                            body: LyricBody::Timed {
                                unit,
                                entries: frame.content,
                            },
                        });
                    }
                }
                _ => (),
            }
        }
    }

    pub(crate) fn vorbis(&mut self, mut tag: VorbisComments) {
        for (picture, _) in tag.remove_pictures() {
            self.picture(picture, true);
        }
        for (key, value) in tag.take_items() {
            let (name, language) = key
                .split_once(':')
                .map_or((key.as_str(), None), |(key, lang)| {
                    (key, (!lang.is_empty()).then(|| lang.to_owned()))
                });
            if name.eq_ignore_ascii_case("LYRICS") {
                let synced = value.lines().any(lrc_timestamp);
                self.text(value, language, None, synced);
            }
        }
    }

    pub(crate) fn mp4(&mut self, tag: Ilst) {
        for atom in tag {
            match atom.ident() {
                AtomIdent::Fourcc([0xa9, b'l', b'y', b'r']) => {
                    for data in atom.into_data() {
                        if let AtomData::UTF8(text) | AtomData::UTF16(text) = data {
                            self.text(text, None, None, false);
                        }
                    }
                }
                AtomIdent::Fourcc([b'c', b'o', b'v', b'r']) => {
                    for data in atom.into_data() {
                        if let AtomData::Picture(picture) = data {
                            self.picture(picture, false);
                        }
                    }
                }
                _ => (),
            }
        }
    }

    pub(crate) fn ape(&mut self, tag: &ApeTag) {
        for item in tag {
            if item.key().eq_ignore_ascii_case("Lyrics") {
                if let Some(text) = item.text_values() {
                    for lyric in text {
                        self.text(lyric.to_owned(), None, None, false);
                    }
                }
            } else if let ItemValue::Binary(bytes) = item.value()
                && let Some(key) = lofty::ape::APE_PICTURE_TYPES
                    .iter()
                    .find(|key| key.eq_ignore_ascii_case(item.key()))
                && let Ok(picture) = NativePicture::from_ape_bytes(key, bytes)
            {
                self.picture(picture, true);
            }
        }
    }

    fn text(
        &mut self,
        text: String,
        language: Option<String>,
        description: Option<String>,
        synced: bool,
    ) {
        if self.lyrics.len() >= limits::MAX_LYRICS {
            return;
        }
        self.lyrics.push(Lyric {
            language,
            description: description.map(|text| limits::text(text, limits::MAX_TEXT_BYTES)),
            synced,
            body: LyricBody::Text(limits::text(text, limits::MAX_LYRIC_BYTES)),
        });
    }
}

fn language(bytes: [u8; 3]) -> Option<String> {
    std::str::from_utf8(&bytes).ok().map(str::to_owned)
}

fn lrc_timestamp(line: &str) -> bool {
    let Some((time, _)) = line
        .trim_start()
        .strip_prefix('[')
        .and_then(|line| line.split_once(']'))
    else {
        return false;
    };
    let Some((minutes, seconds)) = time.split_once(':') else {
        return false;
    };
    let (seconds, fraction) = seconds
        .split_once(['.', ':'])
        .map_or((seconds, None), |(s, f)| (s, Some(f)));
    !minutes.is_empty()
        && minutes.bytes().all(|b| b.is_ascii_digit())
        && seconds.len() == 2
        && seconds.parse::<u8>().is_ok_and(|n| n < 60)
        && fraction.is_none_or(|f| !f.is_empty() && f.bytes().all(|b| b.is_ascii_digit()))
}
