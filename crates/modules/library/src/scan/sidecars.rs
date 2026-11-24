//! Folder artwork and lyric sidecars picked for a track being imported.

use super::paths::{File, Folder, Kind};
use crate::log_events::FILE_UNREADABLE;
use crate::media::{self, SidecarLyric};
use crate::store::{exec, one};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use timbre_edge_sqlite::rusqlite::Connection;
use timbre_kernel::{Ctx, Fallible, log};

/// Artwork already decoded in this scan, keyed by the canonical path. The
/// same picture is often the cover of every disc folder of one release.
#[derive(Default)]
pub(super) struct ArtScan {
    decoded: HashMap<PathBuf, Option<(media::ArtData, PathBuf)>>,
    warnings: super::warnings::Warnings,
}

impl ArtScan {
    pub(super) fn summarize(&self, ctx: &Ctx) {
        self.warnings.summarize(ctx, "art");
    }
}

pub(super) fn folder_art(
    ctx: &Ctx,
    folder: &Folder,
    root: &Path,
    scan: &mut ArtScan,
) -> Option<(media::ArtData, PathBuf)> {
    for stem in ["cover", "folder", "front"] {
        for file in folder.files.iter().filter(|file| {
            file.kind == Kind::Image
                && file
                    .relative
                    .file_stem()
                    .is_some_and(|name| name.eq_ignore_ascii_case(stem))
        }) {
            if let Some(known) = scan.decoded.get(&file.canonical) {
                if let Some(art) = known {
                    return Some(art.clone());
                }
                continue;
            }
            match media::sidecar_art(&file.canonical, root) {
                Ok(art) => {
                    scan.decoded.insert(file.canonical.clone(), art.clone());
                    if let Some(art) = art {
                        return Some(art);
                    }
                }
                Err(error) => {
                    if scan.warnings.allow() {
                        log!(
                            ctx,
                            warn,
                            FILE_UNREADABLE,
                            path = file.absolute.to_string_lossy().as_ref(),
                            reason = format!("{error:?}").as_str()
                        );
                    }
                }
            }
        }
    }
    None
}

/// The lyric sidecars of one folder, built once and matched against every
/// audio file in it.
pub(super) fn lyric_sidecars(folder: &Folder) -> Vec<&File> {
    folder
        .files
        .iter()
        .filter(|sidecar| sidecar.kind == Kind::Lyrics)
        .collect()
}

/// Read the sidecars that belong to one track. File IO happens here so that
/// the write transaction only inserts rows.
pub(super) fn read_lyrics(
    ctx: &Ctx,
    file: &File,
    root: &Path,
    sidecars: &[&File],
) -> Vec<SidecarLyric> {
    let mut candidates: Vec<_> = sidecars
        .iter()
        .filter_map(|sidecar| {
            Some((
                media::lyric_filename(&file.relative, &sidecar.relative)?,
                *sidecar,
            ))
        })
        .collect();
    candidates.sort_by(|(a, first), (b, second)| {
        a.0.cmp(&b.0)
            .then_with(|| b.1.cmp(&a.1))
            .then_with(|| first.relative.cmp(&second.relative))
    });
    let mut seen = HashSet::new();
    let mut read = Vec::new();
    for (position, (candidate, sidecar)) in candidates.into_iter().enumerate() {
        if !seen.insert(candidate.0.clone()) {
            continue;
        }
        match media::read_sidecar_lyric(position, candidate, &sidecar.canonical, root) {
            Ok(Some(lyric)) => read.push(lyric),
            Ok(None) => (),
            Err(error) => log!(
                ctx,
                warn,
                FILE_UNREADABLE,
                path = sidecar.absolute.to_string_lossy().as_ref(),
                reason = format!("{error:?}").as_str()
            ),
        }
    }
    read
}

pub(super) fn save_lyrics(sql: &Connection, track: i64, read: &[SidecarLyric]) -> Fallible {
    exec(
        sql,
        "DELETE FROM library_lyrics WHERE track_id=?1 AND source='sidecar'",
        [track],
    )?;
    let embedded: i64 = one(
        sql,
        "SELECT count(*) FROM library_lyrics WHERE track_id=?1 AND source='embedded'",
        [track],
        |row| row.get(0),
    )?;
    if embedded > 0 {
        return Ok(());
    }
    for lyric in read {
        media::save_sidecar_lyric(sql, track, lyric)?;
    }
    Ok(())
}
