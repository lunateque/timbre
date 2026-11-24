use super::paths::Stamp;
use super::read::Read;
use crate::log_events::RELINK_AMBIGUOUS;
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use timbre_kernel::{Ctx, log};

pub(super) struct Known {
    pub(super) id: i64,
    pub(super) fingerprint: Option<String>,
    pub(super) recording: Option<String>,
    pub(super) size: u64,
    pub(super) mtime: String,
}

pub(super) struct Imported {
    pub(super) root: usize,
    pub(super) folder: PathBuf,
    pub(super) stamp: Stamp,
    pub(super) previous: Option<i64>,
    pub(super) ambiguous: Option<String>,
    pub(super) tags: Read,
}

#[derive(Default)]
struct Match<'a> {
    missing: Vec<&'a Path>,
    added: Vec<PathBuf>,
}

pub(super) fn assign(
    ctx: &Ctx,
    known: &BTreeMap<PathBuf, Known>,
    present: &HashSet<&Path>,
    imports: &mut BTreeMap<PathBuf, Imported>,
) {
    let mut fingerprints: BTreeMap<&str, Match<'_>> = BTreeMap::new();
    for (path, track) in known {
        let Some(fp) = track.fingerprint.as_deref() else {
            continue;
        };
        if !present.contains(path.as_path()) {
            fingerprints.entry(fp).or_default().missing.push(path);
        }
    }
    for (path, track) in imports.iter_mut() {
        if let Some(old) = known.get(path) {
            track.previous = Some(old.id);
            track.tags.fingerprint.clone_from(&old.fingerprint);
        } else if let Some(fp) = track.tags.fingerprint.as_deref() {
            fingerprints.entry(fp).or_default().added.push(path.clone());
        }
    }
    let candidates: Vec<_> = fingerprints
        .into_iter()
        .map(|(key, matches)| (key.to_owned(), matches))
        .collect();
    let mut taken = HashSet::new();
    for (key, matches) in candidates {
        apply(
            ctx,
            matches,
            &format!("fingerprint:{key}"),
            known,
            imports,
            &mut taken,
        );
    }

    let mut recordings = BTreeMap::<(PathBuf, String), Match<'_>>::new();
    for (path, track) in known {
        if present.contains(path.as_path()) || taken.contains(path.as_path()) {
            continue;
        }
        if let Some(key) = recording(path, track.recording.as_deref()) {
            recordings.entry(key).or_default().missing.push(path);
        }
    }
    for (path, track) in imports.iter() {
        if track.previous.is_some() || track.ambiguous.is_some() {
            continue;
        }
        if let Some(key) = recording(
            path,
            track.tags.source.fields.musicbrainz_recording_id.as_deref(),
        ) {
            recordings.entry(key).or_default().added.push(path.clone());
        }
    }
    for ((folder, key), matches) in recordings {
        apply(
            ctx,
            matches,
            &format!("recording:{}:{key}", folder.display()),
            known,
            imports,
            &mut taken,
        );
    }
}

fn recording(path: &Path, id: Option<&str>) -> Option<(PathBuf, String)> {
    Some((
        path.parent()?.to_path_buf(),
        id?.trim().to_ascii_lowercase(),
    ))
}

fn apply<'a>(
    ctx: &Ctx,
    group: Match<'a>,
    key: &str,
    known: &BTreeMap<PathBuf, Known>,
    imports: &mut BTreeMap<PathBuf, Imported>,
    taken: &mut HashSet<&'a Path>,
) {
    if let ([old], [new]) = (group.missing.as_slice(), group.added.as_slice()) {
        if let (Some(previous), Some(track)) = (known.get(*old), imports.get_mut(new)) {
            track.previous = Some(previous.id);
            taken.insert(*old);
        }
        return;
    }
    if group.missing.is_empty() || group.added.is_empty() {
        return;
    }
    let detail = format!(
        "{key}; missing={}; new={}",
        group.missing.len(),
        group.added.len()
    );
    log!(
        ctx,
        warn,
        RELINK_AMBIGUOUS,
        key = key,
        missing = group.missing.len(),
        new = group.added.len()
    );
    for path in group.added {
        if let Some(track) = imports.get_mut(&path) {
            track.ambiguous = Some(detail.clone());
        }
    }
}
