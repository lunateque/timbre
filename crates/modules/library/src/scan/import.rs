use super::errors::{Previous, Unreadable};
use super::paths::{File, Folder, Kind};
use super::read::{self, Need};
use super::relink::{Imported, Known};
use super::sidecars::{self, folder_art, save_lyrics};
use super::tree::Tree;
use super::walk::Forest;
use crate::log_events::FILE_UNREADABLE;
use crate::media;
use crate::store::{all, exec, integer, one, path_to_bytes, row_path, unsigned};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use timbre_edge_sqlite::rusqlite::{Connection, params};
use timbre_edge_sqlite::{Database, json};
use timbre_edge_text::{fold, order_name};
use timbre_kernel::{Ctx, Fallible, log};

pub(super) struct Location<'a> {
    pub(super) folder: &'a Folder,
    pub(super) file: &'a File,
}

pub(super) fn audio_locations(forest: &Forest) -> BTreeMap<&Path, Location<'_>> {
    let mut located = BTreeMap::new();
    for folder in forest.values() {
        for file in folder.files.iter().filter(|file| file.kind == Kind::Audio) {
            located
                .entry(file.canonical.as_path())
                .or_insert(Location { folder, file });
        }
    }
    located
}

pub(super) fn known(db: &Database) -> Fallible<BTreeMap<PathBuf, Known>> {
    db.read(|sql| {
        Ok(all(
            sql,
            "SELECT canonical,id,fingerprint,recording_id,size,mtime FROM library_tracks",
            [],
            |row| {
                Ok((
                    row_path(row, 0)?,
                    Known {
                        id: row.get(1)?,
                        fingerprint: row.get(2)?,
                        recording: row.get(3)?,
                        size: unsigned(row, 4)?,
                        mtime: row.get(5)?,
                    },
                ))
            },
        )?
        .into_iter()
        .collect())
    })
}

pub(super) fn retained_paths(
    db: &Database,
    folders: &HashSet<i64>,
) -> Fallible<BTreeMap<PathBuf, usize>> {
    if folders.is_empty() {
        return Ok(BTreeMap::new());
    }
    db.read(|sql| {
        use timbre_edge_sqlite::failures::query_error;
        let mut statement = sql.prepare("SELECT p.folder_id,t.canonical,r.ordinal FROM library_track_paths p JOIN library_tracks t ON t.id=p.track_id JOIN library_roots r ON r.id=p.root_id ORDER BY r.ordinal").map_err(query_error)?;
        let mut rows = statement.query([]).map_err(query_error)?;
        let mut paths = BTreeMap::new();
        while let Some(row) = rows.next().map_err(query_error)? {
            let folder = row.get(0).map_err(query_error)?;
            if !folders.contains(&folder) { continue; }
            let path = row_path(row, 1).map_err(query_error)?;
            let ordinal = unsigned(row, 2).map_err(query_error)?;
            paths.entry(path).or_insert(usize::try_from(ordinal).unwrap_or(usize::MAX));
        }
        Ok(paths)
    })
}

pub(super) fn requests(
    locations: &BTreeMap<&Path, Location<'_>>,
    roots: &[PathBuf],
    indexed: &BTreeMap<PathBuf, Known>,
    full: bool,
    previous_errors: &Previous,
    errors: &mut Unreadable,
) -> Vec<Need> {
    locations
        .iter()
        .filter_map(|(&path, location)| {
            let stamp = location.file.stamp;
            let old = indexed.get(path);
            if !full
                && old.is_some_and(|track| track.size == stamp.size && track.mtime == stamp.mtime())
            {
                return None;
            }
            if let Some(reason) = previous_errors.reason(path, stamp) {
                errors.insert(path.to_path_buf(), reason.to_owned());
                return None;
            }
            Some(Need {
                canonical: path.to_path_buf(),
                root: roots.get(location.folder.root)?.clone(),
                stamp,
                fingerprint: old.is_none(),
            })
        })
        .collect()
}

type Imports = BTreeMap<PathBuf, Imported>;

pub(super) fn read_tags(
    ctx: &Ctx,
    requests: &[Need],
    locations: &BTreeMap<&Path, Location<'_>>,
    workers: usize,
    stop: &(impl Fn() -> bool + Sync),
    mut progress: impl FnMut(bool),
) -> Fallible<(Imports, Unreadable)> {
    let mut imports = BTreeMap::new();
    let mut failures = BTreeMap::new();
    read::reads(requests, workers, stop, |received| {
        for (index, read) in received {
            let Some(need) = requests.get(index) else {
                continue;
            };
            let Some(location) = locations.get(need.canonical.as_path()) else {
                continue;
            };
            progress(read.is_err());
            match read {
                Ok(tags) => {
                    imports.insert(
                        need.canonical.clone(),
                        Imported {
                            root: location.folder.root,
                            folder: location.folder.relative.clone(),
                            stamp: need.stamp,
                            previous: None,
                            ambiguous: None,
                            tags,
                        },
                    );
                }
                Err(error) => {
                    let reason = format!("{}: {error:?}", error.code().as_str());
                    log!(
                        ctx,
                        warn,
                        FILE_UNREADABLE,
                        path = need.canonical.to_string_lossy().as_ref(),
                        reason = reason.as_str()
                    );
                    failures.insert(need.canonical.clone(), reason);
                }
            }
        }
        Ok(())
    })?;
    Ok((imports, failures))
}

pub(super) fn apply(
    ctx: &Ctx,
    db: &Database,
    imports: Imports,
    indexed: &BTreeMap<PathBuf, Known>,
    errors: &mut Unreadable,
) -> Fallible<BTreeMap<PathBuf, i64>> {
    let mut result = indexed
        .iter()
        .map(|(path, track)| (path.clone(), track.id))
        .collect::<BTreeMap<_, _>>();
    let mut groups = BTreeMap::<_, Vec<_>>::new();
    for (path, mut item) in imports {
        let folder = std::mem::take(&mut item.folder);
        groups
            .entry((item.root, folder))
            .or_default()
            .push((path, item));
    }
    for batch in groups.into_values() {
        // One unwritable track must not cost the rest of the scan: it is
        // recorded as unreadable and the batch carries on.
        let (written, failed) = db.write(|sql| {
            let mut written = Vec::new();
            let mut failed = Vec::new();
            for (path, item) in &batch {
                // One track's rows are all written or none: a failure half
                // way through must not leave a row the next scan trusts.
                exec(sql, "SAVEPOINT track", [])?;
                match write_track(sql, path, item) {
                    Ok(track) => {
                        exec(sql, "RELEASE track", [])?;
                        written.push((path.clone(), track));
                    }
                    Err(error) => {
                        exec(sql, "ROLLBACK TO track", [])?;
                        exec(sql, "RELEASE track", [])?;
                        failed.push((
                            path.clone(),
                            format!("{}: {error:?}", error.code().as_str()),
                        ));
                    }
                }
            }
            Ok((written, failed))
        })?;
        result.extend(written);
        for (path, reason) in failed {
            log!(
                ctx,
                warn,
                FILE_UNREADABLE,
                path = path.to_string_lossy().as_ref(),
                reason = reason.as_str()
            );
            result.remove(&path);
            errors.insert(path, reason);
        }
    }
    Ok(result)
}

fn write_track(sql: &Connection, path: &Path, item: &Imported) -> Fallible<i64> {
    let source = &item.tags.source;
    let fields = &source.fields;
    let title = fields.title.clone().unwrap_or_else(|| {
        path.file_stem()
            .unwrap_or(path.as_os_str())
            .to_string_lossy()
            .into_owned()
    });
    let credit = crate::credits::display(fields.artist.as_ref(), &fields.artists);
    let order = order_name(fields.title_sort.as_deref().unwrap_or(&title));
    let created = crate::store::unix_millis(std::time::SystemTime::now());
    let art = item.tags.art.as_ref();
    if let Some(art) = art {
        art.art.write(sql)?;
    }
    let track = one(
        sql,
        "INSERT INTO library_tracks(id,canonical,fingerprint,recording_id,size,mtime,source,art_hash,art_front,title,order_name,folded_name,folded_artist,track_number,created) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15) ON CONFLICT(id) DO UPDATE SET canonical=excluded.canonical,fingerprint=excluded.fingerprint,recording_id=excluded.recording_id,size=excluded.size,mtime=excluded.mtime,source=excluded.source,art_hash=excluded.art_hash,art_front=excluded.art_front,title=excluded.title,order_name=excluded.order_name,folded_name=excluded.folded_name,folded_artist=excluded.folded_artist,track_number=excluded.track_number RETURNING id",
        params![
            item.previous,
            path_to_bytes(path),
            item.tags.fingerprint,
            fields.musicbrainz_recording_id,
            integer(item.stamp.size),
            item.stamp.mtime(),
            json::encode(source)?,
            art.map(|art| &art.art.hash),
            art.is_some_and(|art| art.front),
            title,
            order,
            fold(&title),
            fold(&credit),
            fields.track_number,
            created
        ],
        |row| row.get(0),
    )?;
    if let Some(art) = art {
        art.write(sql, track)?;
    } else {
        exec(
            sql,
            "DELETE FROM library_embedded_art WHERE track_id=?1",
            [track],
        )?;
    }
    exec(
        sql,
        "DELETE FROM library_lyrics WHERE track_id=?1 AND source='embedded'",
        [track],
    )?;
    media::save_embedded_lyrics(sql, track, &item.tags.lyrics)?;
    if let Some(detail) = &item.ambiguous {
        exec(
            sql,
            "INSERT OR REPLACE INTO library_diagnostics(track_id,kind,detail) VALUES(?1,'relink_ambiguous',?2)",
            params![track, detail],
        )?;
    }
    if item.tags.rejected {
        crate::store::diagnose(
            sql,
            track,
            "tag_rejected",
            "the embedded tag was skipped as unsafe to decode",
        )?;
    } else {
        exec(
            sql,
            "DELETE FROM library_diagnostics WHERE track_id=?1 AND kind='tag_rejected'",
            [track],
        )?;
    }
    Ok(track)
}

pub(super) struct PathUpdates<'a> {
    pub(super) tracks: &'a BTreeMap<PathBuf, i64>,
    pub(super) errors: &'a Unreadable,
    pub(super) retained: &'a BTreeMap<PathBuf, usize>,
}

pub(super) fn update_folders(
    ctx: &Ctx,
    db: &Database,
    tree: &Tree,
    inventory: &super::walk::Inventory,
    roots: &[PathBuf],
    locations: &BTreeMap<&Path, Location<'_>>,
    paths: &PathUpdates<'_>,
) -> Fallible {
    let PathUpdates {
        tracks,
        errors,
        retained,
    } = paths;
    let mut art = sidecars::ArtScan::default();
    for key in &tree.changed {
        let Some(folder) = inventory.forest.get(key) else {
            continue;
        };
        let (Some(&id), Some(&root_id), Some(root)) = (
            tree.folders.get(key),
            tree.roots.get(folder.root),
            roots.get(folder.root),
        ) else {
            continue;
        };
        let picked_art = folder_art(ctx, folder, root, &mut art);
        let lyric_files = sidecars::lyric_sidecars(folder);
        // Reading sidecars is file IO; the transaction below only writes.
        let mut prepared = BTreeMap::new();
        for file in &folder.files {
            let primary = retained
                .get(file.canonical.as_path())
                .is_none_or(|root| *root > folder.root);
            if file.kind == Kind::Audio
                && primary
                && tracks.contains_key(&file.canonical)
                && locations
                    .get(file.canonical.as_path())
                    .is_some_and(|location| {
                        location.file.absolute == file.absolute
                            && location.folder.root == folder.root
                    })
            {
                prepared.insert(
                    file.canonical.as_path(),
                    sidecars::read_lyrics(ctx, file, root, &lyric_files),
                );
            }
        }
        db.write(|sql| {
            exec(sql, "DELETE FROM library_track_paths WHERE folder_id=?1", [id])?;
            exec(sql, "DELETE FROM library_files WHERE folder_id=?1", [id])?;
            exec(sql, "DELETE FROM library_unreadable WHERE folder_id=?1", [id])?;
            exec(sql, "DELETE FROM library_folder_art WHERE folder_id=?1", [id])?;
            if let Some((art, path)) = picked_art {
                art.write(sql)?;
                exec(sql, "INSERT INTO library_folder_art(folder_id,hash,path) VALUES(?1,?2,?3)", params![id, art.hash, path_to_bytes(&path)])?;
            }
            for file in &folder.files {
                let track = tracks.get(&file.canonical).copied().filter(|_| file.kind == Kind::Audio);
                if file.kind == Kind::Audio && let Some(reason) = errors.get(&file.canonical) {
                    exec(sql, "INSERT OR REPLACE INTO library_unreadable(path,folder_id,reason,size,mtime) VALUES(?1,?2,?3,?4,?5)", params![path_to_bytes(&file.canonical), id, reason, integer(file.stamp.size), file.stamp.mtime()])?;
                }
                if let Some(track) = track {
                    exec(sql, "INSERT OR REPLACE INTO library_track_paths(track_id,root_id,folder_id,relative,absolute,size) VALUES(?1,?2,?3,?4,?5,?6)", params![track, root_id, id, path_to_bytes(&file.relative), path_to_bytes(&file.absolute), integer(file.stamp.size)])?;
                    if let Some(read) = prepared.get(file.canonical.as_path()) {
                        save_lyrics(sql, track, read)?;
                    }
                } else if file.kind == Kind::Audio { continue; }
                if let Some(name) = file.relative.file_name() {
                    exec(sql, "INSERT INTO library_files(folder_id,name,sort_name,kind,track_id) VALUES(?1,?2,?3,?4,?5)", params![id, path_to_bytes(Path::new(name)), name.to_string_lossy().as_ref(), file.kind.name(), track])?;
                }
            }
            Ok(())
        })?;
    }
    art.summarize(ctx);
    Ok(())
}
