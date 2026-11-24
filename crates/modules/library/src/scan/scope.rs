//! What a pass has to regroup after its tracks were written.

use crate::store::all;
use std::collections::BTreeSet;
use timbre_edge_sqlite::rusqlite::Connection;
use timbre_kernel::Fallible;

/// Delete the tracks that lost their last path and name what has to be
/// regrouped: the albums of the changed folders, the albums those folders'
/// tracks belonged to before this pass, and the albums that lost a track.
/// A full or root-level pass regroups everything instead.
pub(super) fn regroup_scope(
    db: &timbre_edge_sqlite::Database,
    tree: &super::tree::Tree,
    incremental: bool,
) -> Fallible<Option<crate::albums::Scope>> {
    let folders: BTreeSet<i64> = tree
        .changed
        .iter()
        .filter_map(|key| tree.folders.get(key).copied())
        .collect();
    let touched: Vec<i64> = db.write(|sql| {
        let mut albums = all(sql, "SELECT DISTINCT album_id FROM library_tracks WHERE album_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM library_track_paths WHERE track_id=library_tracks.id)", [], |row| row.get(0))?;
        if incremental {
            // A retagged track leaves its old album behind in an untouched
            // folder; that album has to be rewritten with one song less.
            albums.extend(previous_albums(sql, &folders)?);
        }
        let gone: Vec<i64> = all(sql, "DELETE FROM library_tracks WHERE NOT EXISTS(SELECT 1 FROM library_track_paths WHERE track_id=library_tracks.id) RETURNING id", [], |row| row.get(0))?;
        crate::search::forget(sql, crate::search::Entity::Track, &gone)?;
        Ok(albums)
    })?;
    Ok(incremental.then(|| crate::albums::Scope {
        folders,
        albums: touched.into_iter().collect(),
    }))
}

/// The albums the tracks of these folders belong to right now.
fn previous_albums(sql: &Connection, folders: &BTreeSet<i64>) -> Fallible<Vec<i64>> {
    let list = folders
        .iter()
        .map(i64::to_string)
        .collect::<Vec<_>>()
        .join(",");
    if list.is_empty() {
        return Ok(Vec::new());
    }
    all(
        sql,
        &format!(
            "SELECT DISTINCT t.album_id FROM library_tracks t JOIN library_track_paths p ON p.track_id=t.id WHERE t.album_id IS NOT NULL AND p.folder_id IN ({list})"
        ),
        [],
        |row| row.get(0),
    )
}
