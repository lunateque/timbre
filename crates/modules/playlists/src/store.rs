//! SQLite queries and mutations for `playlists`.

use crate::failures::PlaylistsFailure;
use timbre_edge_sqlite::failures::query_error;
use timbre_edge_sqlite::rusqlite::{Connection, OptionalExtension as _, Params, Row, params};
use timbre_kernel::Fallible;
use timbre_kernel::identity::UserName;
use timbre_kernel::page::Window;

pub(crate) struct PlaylistRow {
    pub(crate) id: String,
    pub(crate) owner: String,
    pub(crate) name: String,
    pub(crate) comment: Option<String>,
    pub(crate) public: bool,
    pub(crate) created_at: i64,
    pub(crate) updated_at: i64,
}

fn all<T>(
    sql: &Connection,
    query: &str,
    values: impl Params,
    map: impl FnMut(&Row<'_>) -> timbre_edge_sqlite::rusqlite::Result<T>,
) -> Fallible<Vec<T>> {
    sql.prepare_cached(query)
        .map_err(query_error)?
        .query_map(values, map)
        .map_err(query_error)?
        .collect::<Result<_, _>>()
        .map_err(query_error)
}

fn optional<T>(
    sql: &Connection,
    query: &str,
    values: impl Params,
    map: impl FnOnce(&Row<'_>) -> timbre_edge_sqlite::rusqlite::Result<T>,
) -> Fallible<Option<T>> {
    sql.prepare_cached(query)
        .map_err(query_error)?
        .query_row(values, map)
        .optional()
        .map_err(query_error)
}

fn exec(sql: &Connection, query: &str, values: impl Params) -> Fallible<usize> {
    sql.prepare_cached(query)
        .map_err(query_error)?
        .execute(values)
        .map_err(query_error)
}

fn as_i64(value: usize) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

fn map_row(row: &Row<'_>) -> timbre_edge_sqlite::rusqlite::Result<PlaylistRow> {
    Ok(PlaylistRow {
        id: row.get(0)?,
        owner: row.get(1)?,
        name: row.get(2)?,
        comment: row.get(3)?,
        public: row.get::<_, i64>(4)? != 0,
        created_at: row.get(5)?,
        updated_at: row.get(6)?,
    })
}

const COLUMNS: &str = "id,owner,name,comment,public,created_at,updated_at";

pub(crate) fn by_id(sql: &Connection, id: &str) -> Fallible<Option<PlaylistRow>> {
    optional(
        sql,
        &format!("SELECT {COLUMNS} FROM playlists_lists WHERE id=?1"),
        params![id],
        map_row,
    )
}

pub(crate) fn list_visible(
    sql: &Connection,
    actor: &UserName,
    owner: Option<&UserName>,
    win: Window,
) -> Fallible<Vec<PlaylistRow>> {
    match owner {
        None => all(
            sql,
            &format!(
                "SELECT {COLUMNS} FROM playlists_lists \
                 WHERE owner=?1 OR public=1 ORDER BY name,id LIMIT ?2 OFFSET ?3"
            ),
            params![actor.as_str(), win.limit(), win.skip()],
            map_row,
        ),
        Some(target) if target == actor => all(
            sql,
            &format!(
                "SELECT {COLUMNS} FROM playlists_lists \
                 WHERE owner=?1 ORDER BY name,id LIMIT ?2 OFFSET ?3"
            ),
            params![target.as_str(), win.limit(), win.skip()],
            map_row,
        ),
        Some(target) => all(
            sql,
            &format!(
                "SELECT {COLUMNS} FROM playlists_lists \
                 WHERE owner=?1 AND public=1 ORDER BY name,id LIMIT ?2 OFFSET ?3"
            ),
            params![target.as_str(), win.limit(), win.skip()],
            map_row,
        ),
    }
}

pub(crate) fn entries_of(sql: &Connection, playlist_id: &str) -> Fallible<Vec<String>> {
    all(
        sql,
        "SELECT track_id FROM playlists_entries WHERE playlist_id=?1 ORDER BY position",
        params![playlist_id],
        |row| row.get(0),
    )
}

/// Deduplicated `(track_id, occurrences)` per playlist so `list` avoids
/// materializing duplicate entries in memory.
pub(crate) fn entry_counts_of(sql: &Connection, playlist_id: &str) -> Fallible<Vec<(String, u64)>> {
    all(
        sql,
        "SELECT track_id,count(*) FROM playlists_entries WHERE playlist_id=?1 GROUP BY track_id",
        params![playlist_id],
        |row| {
            let raw: i64 = row.get(1)?;
            let count = u64::try_from(raw).map_err(|_| {
                timbre_edge_sqlite::rusqlite::Error::IntegralValueOutOfRange(1, raw)
            })?;
            Ok((row.get(0)?, count))
        },
    )
}

pub(crate) fn entry_count_of(sql: &Connection, playlist_id: &str) -> Fallible<usize> {
    let raw: i64 = sql
        .prepare_cached("SELECT count(*) FROM playlists_entries WHERE playlist_id=?1")
        .map_err(query_error)?
        .query_row(params![playlist_id], |row| row.get(0))
        .map_err(query_error)?;
    Ok(usize::try_from(raw).unwrap_or(usize::MAX))
}

pub(crate) fn count_for_user(sql: &Connection, owner: &UserName) -> Fallible<usize> {
    let count: i64 = sql
        .prepare_cached("SELECT count(*) FROM playlists_lists WHERE owner=?1")
        .map_err(query_error)?
        .query_row(params![owner.as_str()], |row| row.get(0))
        .map_err(query_error)?;
    Ok(usize::try_from(count).unwrap_or(usize::MAX))
}

pub(crate) fn total_entries_for_user(sql: &Connection, owner: &UserName) -> Fallible<usize> {
    let count: i64 = sql
        .prepare_cached(
            "SELECT count(*) FROM playlists_entries e \
             JOIN playlists_lists l ON l.id=e.playlist_id WHERE l.owner=?1",
        )
        .map_err(query_error)?
        .query_row(params![owner.as_str()], |row| row.get(0))
        .map_err(query_error)?;
    Ok(usize::try_from(count).unwrap_or(usize::MAX))
}

#[derive(Clone, Copy)]
pub(crate) struct NewPlaylist<'a> {
    pub(crate) id: &'a str,
    pub(crate) owner: &'a UserName,
    pub(crate) name: &'a str,
    pub(crate) comment: Option<&'a str>,
    pub(crate) public: bool,
    pub(crate) now: i64,
    pub(crate) tracks: &'a [&'a str],
    pub(crate) max_per_user: usize,
    pub(crate) max_total_entries: usize,
}

pub(crate) fn insert(sql: &Connection, new: NewPlaylist<'_>) -> Fallible {
    if count_for_user(sql, new.owner)? >= new.max_per_user {
        return Err(PlaylistsFailure::LimitReached(
            "Playlist limit for this user has been reached.",
        )
        .boxed());
    }
    let current_total = total_entries_for_user(sql, new.owner)?;
    if current_total.saturating_add(new.tracks.len()) > new.max_total_entries {
        return Err(PlaylistsFailure::LimitReached(
            "Total playlist entry limit for this user has been reached.",
        )
        .boxed());
    }
    exec(
        sql,
        "INSERT INTO playlists_lists(id,owner,name,comment,public,created_at,updated_at) \
         VALUES(?1,?2,?3,?4,?5,?6,?6)",
        params![
            new.id,
            new.owner.as_str(),
            new.name,
            new.comment,
            i64::from(new.public),
            new.now
        ],
    )?;
    write_entries(sql, new.id, 0, new.tracks)
}

pub(crate) fn update_header(
    sql: &Connection,
    id: &str,
    name: &str,
    comment: Option<&str>,
    public: bool,
    updated_at: i64,
) -> Fallible {
    exec(
        sql,
        "UPDATE playlists_lists SET name=?2,comment=?3,public=?4,updated_at=?5 WHERE id=?1",
        params![id, name, comment, i64::from(public), updated_at],
    )?;
    Ok(())
}

pub(crate) fn append_entries(sql: &Connection, id: &str, tracks: &[&str]) -> Fallible {
    let next_pos: i64 = sql
        .prepare_cached(
            "SELECT coalesce(max(position), -1) + 1 FROM playlists_entries WHERE playlist_id=?1",
        )
        .map_err(query_error)?
        .query_row(params![id], |row| row.get(0))
        .map_err(query_error)?;
    write_entries(sql, id, next_pos, tracks)
}

pub(crate) fn replace_entries(sql: &Connection, id: &str, tracks: &[String]) -> Fallible {
    exec(
        sql,
        "DELETE FROM playlists_entries WHERE playlist_id=?1",
        params![id],
    )?;
    for (position, track_id) in tracks.iter().enumerate() {
        exec(
            sql,
            "INSERT INTO playlists_entries(playlist_id,position,track_id) VALUES(?1,?2,?3)",
            params![id, as_i64(position), track_id],
        )?;
    }
    Ok(())
}

fn write_entries(sql: &Connection, id: &str, start_pos: i64, tracks: &[&str]) -> Fallible {
    for (offset, &track_id) in tracks.iter().enumerate() {
        let pos = start_pos.saturating_add(as_i64(offset));
        exec(
            sql,
            "INSERT INTO playlists_entries(playlist_id,position,track_id) VALUES(?1,?2,?3)",
            params![id, pos, track_id],
        )?;
    }
    Ok(())
}

pub(crate) fn delete_by_id(sql: &Connection, id: &str) -> Fallible {
    exec(sql, "DELETE FROM playlists_lists WHERE id=?1", params![id])?;
    Ok(())
}

pub(crate) fn delete_user(sql: &Connection, owner: &UserName) -> Fallible<usize> {
    exec(
        sql,
        "DELETE FROM playlists_lists WHERE owner=?1",
        params![owner.as_str()],
    )
}
