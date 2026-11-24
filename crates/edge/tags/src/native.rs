use std::io::{Read, Seek};

use lofty::ape::ApeFile;
use lofty::config::ParseOptions;
use lofty::file::{AudioFile, FileType};
use lofty::flac::FlacFile;
use lofty::id3::v1::Id3v1Tag;
use lofty::iff::aiff::{AiffCompressionType, AiffFile};
use lofty::iff::wav::{WavFile, WavFormat};
use lofty::mp4::{Mp4Codec, Mp4File};
use lofty::mpeg::{Layer, MpegFile};
use lofty::ogg::{OggPictureStorage, OpusFile, VorbisFile};
use lofty::properties::FileProperties;
use lofty::tag::Accessor;
use lofty::wavpack::WavPackFile;
use timbre_kernel::Fallible;

use crate::embedded::{Embedded, Wanted};
use crate::failures::{TagFailure, invalid, io};
use crate::metadata::Source;
use crate::model::container_of;
use crate::payload::{self, Layout};
use crate::{
    AudioPayload, Codec, Container, DurationSource, Fields, FileTags, Properties, TextValues, mp3,
};

type Described = Fallible<(Properties, Fields)>;

impl Properties {
    fn from_lofty(codec: Codec, container: Container, props: &FileProperties) -> Self {
        Self {
            codec,
            container,
            duration: props.duration(),
            duration_source: DurationSource::Container,
            bitrate_kbps: props.audio_bitrate().filter(|n| *n != 0),
            sample_rate: props.sample_rate().filter(|n| *n != 0),
            channels: props.channels().filter(|n| *n != 0),
            bit_depth: props.bit_depth().filter(|n| *n != 0),
        }
    }
}

pub(crate) fn read(reader: &mut (impl Read + Seek), kind: FileType) -> Fallible<FileTags> {
    let container = container_of(kind)?;
    let mut layout = payload::locate(reader, container).map_err(io)?;
    let rejected_tags = crate::guard::rejects_tags(reader, container);
    reader.rewind().map_err(io)?;
    let mut options = ParseOptions::new();
    let options = options.read_tags(!rejected_tags);
    let mut embedded = Embedded::default();
    let (properties, fields) = match kind {
        FileType::Mpeg => mpeg(reader, options, &mut layout, &mut embedded)?,
        FileType::Flac => flac(reader, options, &mut embedded)?,
        FileType::Opus | FileType::Vorbis => vorbis(reader, options, kind, &layout, &mut embedded)?,
        FileType::Mp4 => mp4(reader, options, &mut embedded)?,
        FileType::Wav => wav(reader, options, &mut embedded)?,
        FileType::Aiff => aiff(reader, options, &mut embedded)?,
        FileType::WavPack => wavpack(reader, options, &mut embedded)?,
        FileType::Ape => monkey(reader, options, &mut embedded)?,
        _ => return Err(Box::new(TagFailure::Unsupported)),
    };
    Ok(FileTags {
        properties,
        payload: AudioPayload(layout),
        fields,
        pictures: embedded.pictures,
        lyrics: embedded.lyrics,
        rejected_tags,
    })
}

pub(crate) fn pictures(
    reader: &mut (impl Read + Seek),
    kind: FileType,
    index: usize,
) -> Fallible<Vec<crate::Picture>> {
    if crate::guard::rejects_tags(reader, container_of(kind)?) {
        // The tag this picture came from is not safe to decode now either.
        return Ok(Vec::new());
    }
    reader.rewind().map_err(io)?;
    let mut options = ParseOptions::new();
    let options = options.read_properties(false);
    let mut output = Embedded::wanting(Wanted::Index(index));
    match kind {
        FileType::Mpeg => {
            let mut file = MpegFile::read_from(reader, options).map_err(invalid)?;
            if let Some(tag) = file.ape() {
                output.ape(tag);
            }
            if let Some(tag) = file.remove_id3v2() {
                output.id3(tag);
            }
        }
        FileType::Flac => {
            let mut file = FlacFile::read_from(reader, options).map_err(invalid)?;
            if let Some(tag) = file.remove_vorbis_comments() {
                output.vorbis(tag);
            }
            for (picture, _) in file.remove_pictures() {
                output.picture(picture, true);
            }
            if let Some(tag) = file.remove_id3v2() {
                output.id3(tag);
            }
        }
        FileType::Opus => output.vorbis(
            OpusFile::read_from(reader, options)
                .map_err(invalid)?
                .remove_vorbis_comments(),
        ),
        FileType::Vorbis => output.vorbis(
            VorbisFile::read_from(reader, options)
                .map_err(invalid)?
                .remove_vorbis_comments(),
        ),
        FileType::Mp4 => {
            if let Some(tag) = Mp4File::read_from(reader, options)
                .map_err(invalid)?
                .remove_ilst()
            {
                output.mp4(tag);
            }
        }
        FileType::Wav => {
            if let Some(tag) = WavFile::read_from(reader, options)
                .map_err(invalid)?
                .remove_id3v2()
            {
                output.id3(tag);
            }
        }
        FileType::Aiff => {
            if let Some(tag) = AiffFile::read_from(reader, options)
                .map_err(invalid)?
                .remove_id3v2()
            {
                output.id3(tag);
            }
        }
        FileType::WavPack => {
            if let Some(tag) = WavPackFile::read_from(reader, options)
                .map_err(invalid)?
                .ape()
            {
                output.ape(tag);
            }
        }
        FileType::Ape => {
            if let Some(tag) = ApeFile::read_from(reader, options).map_err(invalid)?.ape() {
                output.ape(tag);
            }
        }
        _ => return Err(Box::new(TagFailure::Unsupported)),
    }
    Ok(output.pictures)
}

fn mpeg(
    reader: &mut (impl Read + Seek),
    options: ParseOptions,
    layout: &mut Layout,
    embedded: &mut Embedded,
) -> Described {
    let mut file = MpegFile::read_from(reader, options).map_err(invalid)?;
    if *file.properties().layer() != Layer::Layer3 {
        return Err(Box::new(TagFailure::Unsupported));
    }
    let mut properties =
        Properties::from_lofty(Codec::Mp3, Container::Mpeg, &(*file.properties()).into());
    let range = mp3::first_audio(reader, layout.bounds()).map_err(io)?;
    (properties.duration, properties.duration_source) =
        mp3::duration(reader, &range, properties.duration).map_err(io)?;
    *layout = Layout::Span(range);
    let mut fields = Fields::read(
        &[file.id3v2().map(Source::Id3), file.ape().map(Source::Ape)],
        false,
    );
    if let Some(legacy) = file.remove_id3v1() {
        legacy_basics(&mut fields, legacy);
    }
    if let Some(ape) = file.ape() {
        embedded.ape(ape);
    }
    if let Some(id3) = file.remove_id3v2() {
        embedded.id3(id3);
    }
    Ok((properties, fields))
}

// ID3v1 is the last resort: it only fills what the modern tags left empty.
fn legacy_basics(fields: &mut Fields, legacy: Id3v1Tag) {
    let genre = legacy
        .genre()
        .map(|text| TextValues::Raw(text.into_owned()));
    fields.genre = fields.genre.take().or(genre);
    fields.title = fields
        .title
        .take()
        .or(legacy.title.filter(|t| !t.is_empty()));
    fields.album = fields
        .album
        .take()
        .or(legacy.album.filter(|t| !t.is_empty()));
    fields.artist = fields
        .artist
        .take()
        .or(legacy.artist.filter(|t| !t.is_empty()).map(TextValues::Raw));
    fields.date = fields
        .date
        .take()
        .or_else(|| legacy.year.map(|year| year.to_string()));
    fields.track_number = fields
        .track_number
        .or_else(|| legacy.track_number.map(u32::from));
}

fn flac(
    reader: &mut (impl Read + Seek),
    options: ParseOptions,
    embedded: &mut Embedded,
) -> Described {
    let mut file = FlacFile::read_from(reader, options).map_err(invalid)?;
    let properties =
        Properties::from_lofty(Codec::Flac, Container::Flac, &(*file.properties()).into());
    let fields = Fields::read(
        &[
            file.vorbis_comments().map(Source::Vorbis),
            file.id3v2().map(Source::Id3),
        ],
        false,
    );
    if let Some(tag) = file.remove_vorbis_comments() {
        embedded.vorbis(tag);
    }
    for (picture, _) in file.remove_pictures() {
        embedded.picture(picture, true);
    }
    if let Some(id3) = file.remove_id3v2() {
        embedded.id3(id3);
    }
    Ok((properties, fields))
}

fn vorbis(
    reader: &mut (impl Read + Seek),
    mut options: ParseOptions,
    kind: FileType,
    layout: &Layout,
    embedded: &mut Embedded,
) -> Described {
    // The page walk already measured the stream, so lofty may skip it.
    let options = options.read_properties(false);
    let (codec, tag) = if kind == FileType::Opus {
        let mut file = OpusFile::read_from(reader, options).map_err(invalid)?;
        (Codec::Opus, file.remove_vorbis_comments())
    } else {
        let mut file = VorbisFile::read_from(reader, options).map_err(invalid)?;
        (Codec::Vorbis, file.remove_vorbis_comments())
    };
    let Layout::Ogg(stream) = layout else {
        return Err(invalid("missing Ogg extent"));
    };
    let properties = stream.properties(codec);
    let fields = Fields::read(&[Some(Source::Vorbis(&tag))], codec == Codec::Opus);
    embedded.vorbis(tag);
    Ok((properties, fields))
}

fn mp4(
    reader: &mut (impl Read + Seek),
    options: ParseOptions,
    embedded: &mut Embedded,
) -> Described {
    let mut file = Mp4File::read_from(reader, options).map_err(invalid)?;
    let codec = match file.properties().codec() {
        Some(Mp4Codec::AAC) => Codec::Aac,
        Some(Mp4Codec::ALAC) => Codec::Alac,
        Some(Mp4Codec::FLAC) => Codec::Flac,
        Some(Mp4Codec::MP3) => Codec::Mp3,
        _ => return Err(Box::new(TagFailure::Unsupported)),
    };
    let properties =
        Properties::from_lofty(codec, Container::Mp4, &file.properties().clone().into());
    let fields = Fields::read(&[file.ilst().map(Source::Mp4)], false);
    if let Some(tag) = file.remove_ilst() {
        embedded.mp4(tag);
    }
    Ok((properties, fields))
}

fn wav(
    reader: &mut (impl Read + Seek),
    options: ParseOptions,
    embedded: &mut Embedded,
) -> Described {
    let mut file = WavFile::read_from(reader, options).map_err(invalid)?;
    let codec = match file.properties().format() {
        WavFormat::Other(format) => Codec::Wav(*format),
        _ => Codec::Pcm,
    };
    let properties = Properties::from_lofty(codec, Container::Wav, &(*file.properties()).into());
    let fields = Fields::read(
        &[
            file.id3v2().map(Source::Id3),
            file.riff_info().map(Source::Riff),
        ],
        false,
    );
    if let Some(tag) = file.remove_id3v2() {
        embedded.id3(tag);
    }
    Ok((properties, fields))
}

fn aiff(
    reader: &mut (impl Read + Seek),
    options: ParseOptions,
    embedded: &mut Embedded,
) -> Described {
    let mut file = AiffFile::read_from(reader, options).map_err(invalid)?;
    let codec = aiff_codec(file.properties().compression_type());
    let properties =
        Properties::from_lofty(codec, Container::Aiff, &file.properties().clone().into());
    let fields = Fields::read(&[file.id3v2().map(Source::Id3)], false);
    if let Some(tag) = file.remove_id3v2() {
        embedded.id3(tag);
    }
    Ok((properties, fields))
}

fn aiff_codec(compression: Option<&AiffCompressionType>) -> Codec {
    match compression {
        None
        | Some(
            AiffCompressionType::None
            | AiffCompressionType::sowt
            | AiffCompressionType::fl32
            | AiffCompressionType::fl64
            | AiffCompressionType::FL32,
        ) => Codec::Pcm,
        Some(AiffCompressionType::ACE2) => Codec::Aiff(*b"ACE2"),
        Some(AiffCompressionType::ACE8) => Codec::Aiff(*b"ACE8"),
        Some(AiffCompressionType::MAC3) => Codec::Aiff(*b"MAC3"),
        Some(AiffCompressionType::MAC6) => Codec::Aiff(*b"MAC6"),
        Some(AiffCompressionType::alaw) => Codec::Aiff(*b"alaw"),
        Some(AiffCompressionType::ALAW) => Codec::Aiff(*b"ALAW"),
        Some(AiffCompressionType::ulaw) => Codec::Aiff(*b"ulaw"),
        Some(AiffCompressionType::ULAW) => Codec::Aiff(*b"ULAW"),
        Some(AiffCompressionType::Other {
            compression_type, ..
        }) => Codec::Aiff(*compression_type),
    }
}

fn wavpack(
    reader: &mut (impl Read + Seek),
    options: ParseOptions,
    embedded: &mut Embedded,
) -> Described {
    let file = WavPackFile::read_from(reader, options).map_err(invalid)?;
    let properties = Properties::from_lofty(
        Codec::WavPack,
        Container::WavPack,
        &(*file.properties()).into(),
    );
    let fields = Fields::read(&[file.ape().map(Source::Ape)], false);
    if let Some(ape) = file.ape() {
        embedded.ape(ape);
    }
    Ok((properties, fields))
}

fn monkey(
    reader: &mut (impl Read + Seek),
    options: ParseOptions,
    embedded: &mut Embedded,
) -> Described {
    let file = ApeFile::read_from(reader, options).map_err(invalid)?;
    let properties = Properties::from_lofty(
        Codec::MonkeyAudio,
        Container::Ape,
        &file.properties().clone().into(),
    );
    let fields = Fields::read(&[file.ape().map(Source::Ape)], false);
    if let Some(ape) = file.ape() {
        embedded.ape(ape);
    }
    Ok((properties, fields))
}
