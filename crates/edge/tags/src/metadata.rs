mod mp4;

use std::borrow::Cow;

use lofty::ape::ApeTag;
use lofty::id3::v2::{Frame, FrameId, Id3v2Tag, Id3v2Version};
use lofty::iff::wav::RiffInfoList;
use lofty::mp4::{AtomData, Ilst};
use lofty::ogg::tag::VorbisComments;

use self::mp4::{freeform, mp4_atom, mp4_number, mp4_pair};
use crate::limits;
use crate::{Advisory, Performer, TextValues};

pub(crate) enum Source<'a> {
    Id3(&'a Id3v2Tag),
    Vorbis(&'a VorbisComments),
    Mp4(&'a Ilst),
    Ape(&'a ApeTag),
    Riff(&'a RiffInfoList),
}

#[derive(Clone, Copy)]
pub(crate) struct Key(
    pub &'static str,
    pub &'static str,
    pub &'static str,
    pub &'static str,
    pub &'static str,
);

pub(crate) struct ReadFields<'a>(pub &'a [Option<Source<'a>>]);

impl<'a> Source<'a> {
    fn key(&self, key: &Key) -> &'static str {
        match self {
            Self::Id3(_) => key.0,
            Self::Vorbis(_) => key.1,
            Self::Mp4(_) => key.2,
            Self::Ape(_) => key.3,
            Self::Riff(_) => key.4,
        }
    }

    fn values(&self, key: &str, mut visit: impl FnMut(&'a str)) {
        match *self {
            Self::Id3(tag) => {
                if let Some(role) = key.strip_prefix('#') {
                    for (kind, name) in people(tag, false) {
                        if kind.eq_ignore_ascii_case(role) {
                            visit(name);
                        }
                    }
                } else if key == "@UFID" {
                    if let Some(id) = tag.into_iter().find_map(|frame| match frame {
                        Frame::UniqueFileIdentifier(frame)
                            if frame.owner == "http://musicbrainz.org" =>
                        {
                            std::str::from_utf8(&frame.identifier).ok()
                        }
                        _ => None,
                    }) {
                        visit(id);
                    }
                } else {
                    let text = key.strip_prefix(':').map_or_else(
                        || tag.get_text(&FrameId::Valid(Cow::Borrowed(key))),
                        |desc| {
                            tag.into_iter().find_map(|frame| match frame {
                                Frame::UserText(text)
                                    if text.description.eq_ignore_ascii_case(desc) =>
                                {
                                    Some(text.content.as_ref())
                                }
                                _ => None,
                            })
                        },
                    );
                    if let Some(text) = text {
                        if tag.original_version() == Id3v2Version::V4 {
                            text.split('\0').for_each(visit);
                        } else {
                            visit(text);
                        }
                    }
                }
            }
            Self::Vorbis(tag) => tag
                .items()
                .filter(|(name, _)| name.eq_ignore_ascii_case(key))
                .for_each(|(_, value)| visit(value)),
            Self::Mp4(tag) => {
                if let Some(atom) = mp4_atom(tag, key) {
                    atom.data()
                        .filter_map(|data| match data {
                            AtomData::UTF8(text) | AtomData::UTF16(text) => Some(text.as_str()),
                            _ => None,
                        })
                        .for_each(visit);
                }
            }
            Self::Ape(tag) => {
                if let Some(text) = tag.get(key).and_then(lofty::ape::ApeItem::text_values) {
                    text.for_each(visit);
                }
            }
            Self::Riff(tag) => {
                if let Some(text) = tag.get(key) {
                    visit(text);
                }
            }
        }
    }

    fn first(&self, key: &str) -> Option<&'a str> {
        let mut found = None;
        self.values(key, |value| {
            if found.is_none() && !value.is_empty() {
                found = Some(value);
            }
        });
        found
    }

    fn number(&self, key: &str) -> Option<u32> {
        if let Self::Mp4(tag) = *self
            && let Some(number) = mp4_number(tag, key)
        {
            return Some(number);
        }
        if key == "@TORY"
            && let Self::Id3(tag) = *self
            && tag.original_version() != Id3v2Version::V4
        {
            return timestamp(tag, "TDOR").map(|date| u32::from(date.timestamp.year));
        }
        self.first(key)?.trim().parse().ok()
    }
}

impl ReadFields<'_> {
    fn pick<T>(
        &self,
        key: &Key,
        mut read: impl FnMut(&Source<'_>, &str) -> Option<T>,
    ) -> Option<T> {
        self.0.iter().flatten().find_map(|source| {
            source
                .key(key)
                .split('|')
                .filter(|key| !key.is_empty())
                .find_map(|key| read(source, key))
        })
    }

    pub(crate) fn scalar(&self, key: Key) -> Option<String> {
        self.pick(&key, |source, key| {
            if let Source::Id3(tag) = *source
                && let Some(id @ ("TDRC" | "TDOR")) = key.strip_prefix('@')
            {
                if id == "TDOR" && tag.original_version() != Id3v2Version::V4 {
                    return None;
                }
                return timestamp(tag, id).map(|date| date.timestamp.to_string());
            }
            source.first(key).map(clamp)
        })
    }

    pub(crate) fn text(&self, key: Key) -> Option<TextValues> {
        self.pick(&key, |source, key| {
            let mut first = None;
            let mut many = Vec::new();
            source.values(key, |text| {
                if text.is_empty() || many.len() >= limits::MAX_VALUES {
                    return;
                }
                if many.is_empty() {
                    if let Some(previous) = first.take() {
                        many.push(previous);
                    } else {
                        first = Some(clamp(text));
                        return;
                    }
                }
                many.push(clamp(text));
            });
            first
                .map(TextValues::Raw)
                .or_else(|| (!many.is_empty()).then_some(TextValues::List(many)))
        })
    }

    pub(crate) fn list(&self, key: Key) -> Vec<String> {
        self.pick(&key, |source, key| {
            let mut values = Vec::new();
            source.values(key, |text| {
                if !text.is_empty() && values.len() < limits::MAX_VALUES {
                    values.push(clamp(text));
                }
            });
            (!values.is_empty()).then_some(values)
        })
        .unwrap_or_default()
    }

    pub(crate) fn number(&self, key: Key) -> Option<u32> {
        // A function item cannot carry the caller's lifetimes here.
        self.pick(&key, |source: &Source<'_>, key: &str| source.number(key))
    }

    pub(crate) fn flag(&self, key: Key) -> Option<bool> {
        self.pick(&key, |source, key| {
            if let Source::Mp4(tag) = *source
                && let Some(number) = mp4_number(tag, key)
            {
                return Some(number != 0);
            }
            let text = source.first(key)?.trim();
            if text == "1" || text.eq_ignore_ascii_case("true") {
                Some(true)
            } else if text == "0" || text.eq_ignore_ascii_case("false") {
                Some(false)
            } else {
                None
            }
        })
    }

    pub(crate) fn advisory(&self, key: Key) -> Option<Advisory> {
        self.number(key)
            .and_then(|n| u8::try_from(n).ok())
            .map(|n| match n {
                0 => Advisory::None,
                1 | 4 => Advisory::Explicit,
                2 => Advisory::Clean,
                other => Advisory::Other(other),
            })
    }

    pub(crate) fn pair(&self, key: Key, totals: &str) -> (Option<u32>, Option<u32>) {
        let mut number = None;
        let mut total = None;
        for source in self.0.iter().flatten() {
            let (n, t) = match *source {
                Source::Mp4(tag) if key.2 == "©mvi" => {
                    (mp4_number(tag, "©mvi"), mp4_number(tag, "©mvc"))
                }
                Source::Mp4(tag) => mp4_pair(tag, key.2),
                Source::Vorbis(_) => {
                    let (n, embedded) = text_pair(source.first(key.1));
                    let explicit = totals.split('|').find_map(|key| source.number(key));
                    (n, explicit.or(embedded))
                }
                Source::Ape(_) if key.3 == "MOVEMENT" => {
                    let (n, embedded) = text_pair(source.first(key.3));
                    (n, source.number("MOVEMENTTOTAL").or(embedded))
                }
                Source::Riff(_) if key.4 == "ITRK" => (
                    source.number("ITRK").or_else(|| source.number("IPRT")),
                    None,
                ),
                _ => text_pair(source.first(source.key(&key))),
            };
            number = number.or(n.filter(|n| *n != 0));
            total = total.or(t.filter(|n| *n != 0));
        }
        (number, total)
    }

    pub(crate) fn gain(&self, key: &'static str) -> Option<f64> {
        let text = self.first_custom(key)?.trim();
        let value = text
            .strip_suffix("dB")
            .or_else(|| text.strip_suffix("db"))
            .unwrap_or(text);
        value
            .trim_end()
            .parse::<f64>()
            .ok()
            .filter(|gain| gain.is_finite())
    }

    pub(crate) fn r128(&self, key: &'static str) -> Option<i16> {
        self.first_custom(key)?.trim().parse().ok()
    }

    fn first_custom(&self, key: &'static str) -> Option<&str> {
        self.0.iter().flatten().find_map(|source| match source {
            Source::Id3(tag) => tag.into_iter().find_map(|frame| match frame {
                Frame::UserText(text) if text.description.eq_ignore_ascii_case(key) => {
                    Some(text.content.as_ref())
                }
                _ => None,
            }),
            Source::Mp4(tag) => freeform(tag, key)?.data().find_map(|data| match data {
                AtomData::UTF8(text) | AtomData::UTF16(text) => Some(text.as_str()),
                _ => None,
            }),
            Source::Riff(_) => None,
            _ => source.first(key),
        })
    }

    pub(crate) fn performers(&self) -> Vec<Performer> {
        for source in self.0.iter().flatten() {
            let mut performers = Vec::new();
            match source {
                Source::Id3(tag) => {
                    performers.extend(people(tag, true).take(limits::MAX_VALUES).map(
                        |(instrument, name)| Performer {
                            name: clamp(name),
                            instrument: Some(clamp(instrument)),
                        },
                    ));
                }
                Source::Vorbis(_) => source.values("PERFORMER", |text| {
                    if performers.len() >= limits::MAX_VALUES {
                        return;
                    }
                    let (name, instrument) = text
                        .strip_suffix(')')
                        .and_then(|text| text.rsplit_once(" ("))
                        .map_or((text, None), |(name, role)| (name, Some(clamp(role))));
                    performers.push(Performer {
                        name: clamp(name),
                        instrument,
                    });
                }),
                Source::Ape(_) => source.values("Performer", |name| {
                    if performers.len() < limits::MAX_VALUES {
                        performers.push(Performer {
                            name: clamp(name),
                            instrument: None,
                        });
                    }
                }),
                Source::Mp4(_) | Source::Riff(_) => (),
            }
            if !performers.is_empty() {
                return performers;
            }
        }
        Vec::new()
    }
}

fn clamp(text: &str) -> String {
    limits::text(text.to_owned(), limits::MAX_TEXT_BYTES)
}

fn people(tag: &Id3v2Tag, performers: bool) -> impl Iterator<Item = (&str, &str)> {
    let legacy = tag.original_version() != Id3v2Version::V4;
    tag.into_iter()
        .filter_map(move |frame| match frame {
            Frame::KeyValue(frame)
                if frame.id().as_str()
                    == if performers && !legacy {
                        "TMCL"
                    } else {
                        "TIPL"
                    } =>
            {
                Some(frame)
            }
            _ => None,
        })
        .flat_map(move |frame| {
            frame.key_value_pairs.iter().filter_map(move |(key, name)| {
                let reserved = ["arranger", "engineer", "mix", "DJ-mix", "producer"]
                    .iter()
                    .any(|role| key.eq_ignore_ascii_case(role));
                (!performers || !legacy || !reserved).then_some((key.as_ref(), name.as_ref()))
            })
        })
}

fn timestamp<'a>(
    tag: &'a Id3v2Tag,
    id: &str,
) -> Option<&'a lofty::id3::v2::TimestampFrame<'static>> {
    match tag.get(&FrameId::Valid(Cow::Borrowed(id)))? {
        Frame::Timestamp(frame) => Some(frame),
        _ => None,
    }
}

fn text_pair(text: Option<&str>) -> (Option<u32>, Option<u32>) {
    text.map_or((None, None), |text| {
        if let Some((number, total)) = text.split_once('/') {
            (number.trim().parse().ok(), total.trim().parse().ok())
        } else {
            (text.trim().parse().ok(), None)
        }
    })
}
