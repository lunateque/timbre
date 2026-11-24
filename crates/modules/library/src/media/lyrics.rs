//! Lyrics: sidecar files, embedded entries, and the rendered page.

use super::read_bounded;
use crate::lrc;
use crate::store::{all, contained, diagnose, exec, path_to_bytes};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use timbre_edge_sqlite::json;
use timbre_edge_sqlite::rusqlite::{Connection, params};
use timbre_edge_tags::{Lyric, LyricBody, LyricTime, Properties};
use timbre_kernel::Fallible;
use timbre_kernel::catalog::{LyricLine, Lyrics};

const LYRIC_LIMIT: usize = 1024 * 1024;

pub(crate) fn lyric_fits(lyric: &Lyric) -> bool {
    match &lyric.body {
        LyricBody::Text(text) => text.len() <= LYRIC_LIMIT,
        LyricBody::Timed { entries, .. } => {
            entries
                .iter()
                .fold(0usize, |sum, (_, text)| sum.saturating_add(text.len() + 4))
                <= LYRIC_LIMIT
        }
    }
}

pub(crate) fn lyric_filename(audio: &Path, sidecar: &Path) -> Option<(Option<String>, bool)> {
    let track = audio.file_stem()?.to_str()?;
    let name = sidecar.file_stem()?.to_str()?;
    let lrc = sidecar.extension()?.to_str()?.eq_ignore_ascii_case("lrc");
    if track == name {
        return Some((None, lrc));
    }
    let language = name.strip_prefix(track)?.strip_prefix('.')?;
    if lrc
        && !language.is_empty()
        && language
            .bytes()
            .all(|b| b.is_ascii_alphabetic() || b == b'-')
    {
        return Some((Some(language.to_owned()), true));
    }
    None
}

/// Read a lyric sidecar, honouring a UTF-16 byte order mark. The flag reports
/// that bytes were invalid in their encoding and were replaced.
pub(crate) fn lyrics_text(path: &Path) -> Fallible<Option<(String, bool)>> {
    Ok(read_bounded(path, LYRIC_LIMIT)?.map(|bytes| decode_text(&bytes)))
}

fn decode_text(bytes: &[u8]) -> (String, bool) {
    let utf16 = |units: Vec<u16>| {
        let text = String::from_utf16_lossy(&units);
        let lossy = text.contains('\u{fffd}');
        (text, lossy)
    };
    let units = |rest: &[u8], big: bool| {
        rest.as_chunks::<2>()
            .0
            .iter()
            .map(|pair| {
                if big {
                    u16::from_be_bytes(*pair)
                } else {
                    u16::from_le_bytes(*pair)
                }
            })
            .collect()
    };
    match bytes {
        [0xff, 0xfe, rest @ ..] => utf16(units(rest, false)),
        [0xfe, 0xff, rest @ ..] => utf16(units(rest, true)),
        _ => match std::str::from_utf8(bytes) {
            Ok(text) => (text.to_owned(), false),
            Err(_) => (String::from_utf8_lossy(bytes).into_owned(), true),
        },
    }
}

pub(crate) fn save_embedded_lyrics(sql: &Connection, track_id: i64, lyrics: &[Lyric]) -> Fallible {
    for (index, lyric) in lyrics.iter().enumerate() {
        let body = json::encode(&lyric.body)?;
        let position = i64::try_from(index).unwrap_or(i64::MAX);
        let (language, synced) = match &lyric.body {
            LyricBody::Text(text) if lyric.synced => (
                lyric.language.as_deref().or_else(|| lrc::language(text)),
                true,
            ),
            _ => (lyric.language.as_deref(), lyric.synced),
        };
        exec(
            sql,
            "INSERT INTO library_lyrics(track_id,position,language,description,synced,source,text) VALUES(?1,?2,?3,?4,?5,'embedded',?6)",
            params![
                track_id,
                position,
                language,
                lyric.description,
                synced,
                body
            ],
        )?;
    }
    Ok(())
}

/// One lyric sidecar, already read and classified. The scan builds these
/// before it opens the write transaction, so no file IO runs while the
/// single writer connection is held.
pub(crate) struct SidecarLyric {
    position: i64,
    language: Option<String>,
    synced: bool,
    path: PathBuf,
    root: PathBuf,
    lossy: bool,
}

pub(crate) fn read_sidecar_lyric(
    position: usize,
    candidate: (Option<String>, bool),
    path: &Path,
    root: &Path,
) -> Fallible<Option<SidecarLyric>> {
    let path = contained(path, root)?;
    let Some((text, lossy)) = lyrics_text(&path)? else {
        return Ok(None);
    };
    let (mut language, lrc_format) = candidate;
    if lrc_format && language.is_none() {
        language = lrc::language(&text).map(str::to_owned);
    }
    let synced = lrc_format
        && lrc::parse_lrc(&text)
            .first()
            .is_some_and(|line| line.start.is_some());
    Ok(Some(SidecarLyric {
        position: i64::try_from(position).unwrap_or(i64::MAX),
        language,
        synced,
        path,
        root: root.to_path_buf(),
        lossy,
    }))
}

pub(crate) fn save_sidecar_lyric(
    sql: &Connection,
    track_id: i64,
    lyric: &SidecarLyric,
) -> Fallible {
    if lyric.lossy {
        diagnose(
            sql,
            track_id,
            "lyrics_undecodable",
            &lyric.path.to_string_lossy(),
        )?;
    }
    exec(
        sql,
        "INSERT INTO library_lyrics(track_id,position,language,synced,source,path,root) VALUES(?1,?2,?3,?4,'sidecar',?5,?6)",
        params![
            track_id,
            lyric.position,
            lyric.language,
            lyric.synced,
            path_to_bytes(&lyric.path),
            path_to_bytes(&lyric.root)
        ],
    )?;
    Ok(())
}

pub(crate) fn lyrics(
    sql: &Connection,
    track_id: i64,
    properties: Properties,
    window: crate::store::Window,
) -> Fallible<timbre_kernel::Page<Lyrics>> {
    let rows = all(
        sql,
        "SELECT language, description, synced, text, path, root FROM library_lyrics WHERE track_id=?1 ORDER BY position LIMIT ?2 OFFSET ?3",
        params![track_id, window.limit(), window.skip()],
        |row| {
            Ok((
                row.get::<_, Option<String>>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, bool>(2)?,
                row.get::<_, Option<String>>(3)?,
                crate::store::optional_path(row, 4)?,
                crate::store::optional_path(row, 5)?,
            ))
        },
    )?;
    let mut items = Vec::with_capacity(rows.len());
    for (language, description, synced, text, path, root) in rows {
        let lines = if let Some(body) = text {
            render(&json::decode(&body)?, synced, properties)
        } else if let (Some(path), Some(root)) = (path, root) {
            // A sidecar deleted or grown past the cap since the scan must not
            // fail the whole page of lyrics.
            let Some(source) = contained(&path, &root)
                .ok()
                .map(|path| lyrics_text(&path))
                .transpose()?
                .flatten()
            else {
                continue;
            };
            text_lines(&source.0, synced)
        } else {
            continue;
        };
        items.push(Lyrics {
            language,
            description,
            synced,
            lines,
        });
    }
    Ok(window.page(items))
}

fn text_lines(source: &str, synced: bool) -> Vec<LyricLine> {
    if synced {
        lrc::parse_lrc(source)
    } else {
        source
            .lines()
            .map(|text| LyricLine {
                start: None,
                text: Arc::from(text),
                words: Vec::new(),
            })
            .collect()
    }
}

fn render(body: &LyricBody, synced: bool, properties: Properties) -> Vec<LyricLine> {
    match body {
        LyricBody::Text(text) => text_lines(text, synced),
        LyricBody::Timed { unit, entries } => entries
            .iter()
            .map(|(at, text)| {
                let start = match unit {
                    LyricTime::Milliseconds => Some(Duration::from_millis(u64::from(*at))),
                    LyricTime::MpegFrames => {
                        properties.sample_rate.filter(|&hz| hz > 0).map(|hz| {
                            // A low sample rate turns a large frame count into
                            // more nanoseconds than u64 can hold.
                            let samples = if hz >= 32_000 { 1152 } else { 576 };
                            let nanos = u128::from(*at) * samples * 1_000_000_000 / u128::from(hz);
                            Duration::from_nanos(u64::try_from(nanos).unwrap_or(u64::MAX))
                        })
                    }
                };
                LyricLine {
                    start,
                    text: Arc::from(text.as_str()),
                    words: Vec::new(),
                }
            })
            .collect(),
    }
}
