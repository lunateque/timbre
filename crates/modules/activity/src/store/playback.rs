//! SQLite queries and mutations for plays, album rankings, queues and bookmarks.

use super::{all, as_i64, exec, optional, u64_col};
use crate::failures::ActivityFailure;
use crate::validate::kind_code;
use timbre_edge_sqlite::failures::query_error;
use timbre_edge_sqlite::rusqlite::{Connection, params};
use timbre_kernel::Fallible;
use timbre_kernel::activity::{AlbumActivityList, AlbumStats, Bookmark, ItemKind, TrackStats};
use timbre_kernel::clock::from_unix_millis;
use timbre_kernel::identity::UserName;

pub(crate) fn record_play(
    sql: &Connection,
    user: &UserName,
    track: &str,
    album: Option<&str>,
    at: i64,
) -> Fallible {
    exec(
        sql,
        "INSERT INTO activity_track_plays(user,track_id,play_count,last_played) \
         VALUES(?1,?2,1,?3) \
         ON CONFLICT(user,track_id) DO UPDATE SET \
           play_count=CASE WHEN play_count < 9223372036854775807 THEN play_count + 1 ELSE play_count END, \
           last_played=CASE WHEN excluded.last_played > last_played THEN excluded.last_played ELSE last_played END",
        params![user.as_str(), track, at],
    )?;
    if let Some(album_id) = album {
        exec(
            sql,
            "INSERT INTO activity_album_plays(user,album_id,play_count,last_played) \
             VALUES(?1,?2,1,?3) \
             ON CONFLICT(user,album_id) DO UPDATE SET \
               play_count=CASE WHEN play_count < 9223372036854775807 THEN play_count + 1 ELSE play_count END, \
               last_played=CASE WHEN excluded.last_played > last_played THEN excluded.last_played ELSE last_played END",
            params![user.as_str(), album_id, at],
        )?;
    }
    Ok(())
}

pub(crate) fn track_stats_lookup(
    sql: &Connection,
    user: &UserName,
    sorted_ids: &[&str],
) -> Fallible<Vec<TrackStats>> {
    let mut found = Vec::new();
    for &id in sorted_ids {
        if let Some((count, last)) = optional(
            sql,
            "SELECT play_count,last_played FROM activity_track_plays WHERE user=?1 AND track_id=?2",
            params![user.as_str(), id],
            |row| Ok((u64_col(row, 0)?, row.get::<_, i64>(1)?)),
        )? {
            found.push(TrackStats {
                track: id.to_owned(),
                play_count: count,
                last_played: Some(from_unix_millis(last)),
            });
        }
    }
    Ok(found)
}

pub(crate) fn album_stats_lookup(
    sql: &Connection,
    user: &UserName,
    sorted_ids: &[&str],
) -> Fallible<Vec<AlbumStats>> {
    let mut found = Vec::new();
    for &id in sorted_ids {
        if let Some((count, last)) = optional(
            sql,
            "SELECT play_count,last_played FROM activity_album_plays WHERE user=?1 AND album_id=?2",
            params![user.as_str(), id],
            |row| Ok((u64_col(row, 0)?, row.get::<_, i64>(1)?)),
        )? {
            found.push(AlbumStats {
                album: id.to_owned(),
                play_count: count,
                last_played: Some(from_unix_millis(last)),
            });
        }
    }
    Ok(found)
}

pub(crate) fn top_tracks_chunk(
    sql: &Connection,
    user: Option<&UserName>,
    offset: usize,
    limit: usize,
) -> Fallible<Vec<TrackStats>> {
    if let Some(account) = user {
        all(
            sql,
            "SELECT track_id,play_count,last_played FROM activity_track_plays \
             WHERE user=?1 ORDER BY play_count DESC,last_played DESC,track_id LIMIT ?2 OFFSET ?3",
            params![account.as_str(), as_i64(limit), as_i64(offset)],
            |row| {
                Ok(TrackStats {
                    track: row.get(0)?,
                    play_count: u64_col(row, 1)?,
                    last_played: Some(from_unix_millis(row.get(2)?)),
                })
            },
        )
    } else {
        // Global aggregation intentionally omits `last_played` so one user
        // cannot infer exact listening timestamps of other users.
        all(
            sql,
            "SELECT track_id,sum(play_count) AS total \
             FROM activity_track_plays \
             GROUP BY track_id ORDER BY total DESC,track_id LIMIT ?1 OFFSET ?2",
            params![as_i64(limit), as_i64(offset)],
            |row| {
                Ok(TrackStats {
                    track: row.get(0)?,
                    play_count: u64_col(row, 1)?,
                    last_played: None,
                })
            },
        )
    }
}

pub(crate) fn albums_chunk(
    sql: &Connection,
    user: &UserName,
    list: AlbumActivityList,
    offset: usize,
    limit: usize,
) -> Fallible<Vec<String>> {
    match list {
        AlbumActivityList::Frequent => all(
            sql,
            "SELECT album_id FROM activity_album_plays \
             WHERE user=?1 ORDER BY play_count DESC,last_played DESC,album_id LIMIT ?2 OFFSET ?3",
            params![user.as_str(), as_i64(limit), as_i64(offset)],
            |row| row.get(0),
        ),
        AlbumActivityList::Recent => all(
            sql,
            "SELECT album_id FROM activity_album_plays \
             WHERE user=?1 ORDER BY last_played DESC,album_id LIMIT ?2 OFFSET ?3",
            params![user.as_str(), as_i64(limit), as_i64(offset)],
            |row| row.get(0),
        ),
        AlbumActivityList::Highest => all(
            sql,
            "SELECT item_id FROM activity_ratings WHERE kind=?1 \
             GROUP BY item_id ORDER BY avg(CAST(rating AS REAL)) DESC,count(*) DESC,item_id \
             LIMIT ?2 OFFSET ?3",
            params![kind_code(ItemKind::Album), as_i64(limit), as_i64(offset)],
            |row| row.get(0),
        ),
        AlbumActivityList::Starred => all(
            sql,
            "SELECT item_id FROM activity_stars \
             WHERE user=?1 AND kind=?2 ORDER BY starred_at DESC,item_id LIMIT ?3 OFFSET ?4",
            params![
                user.as_str(),
                kind_code(ItemKind::Album),
                as_i64(limit),
                as_i64(offset)
            ],
            |row| row.get(0),
        ),
    }
}

pub(crate) struct RawQueue {
    pub(crate) tracks: Vec<String>,
    pub(crate) current_index: Option<usize>,
    pub(crate) position_ms: u64,
    pub(crate) changed_by: String,
    pub(crate) changed_at: i64,
}

pub(crate) fn read_queue(sql: &Connection, user: &UserName) -> Fallible<Option<RawQueue>> {
    let Some((current_index, position_ms, changed_by, changed_at)) = optional(
        sql,
        "SELECT current_index,position_ms,changed_by,changed_at FROM activity_play_queues WHERE user=?1",
        params![user.as_str()],
        |row| {
            let idx: Option<i64> = row.get(0)?;
            Ok((
                idx.and_then(|v| usize::try_from(v).ok()),
                u64_col(row, 1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
            ))
        },
    )?
    else {
        return Ok(None);
    };
    let tracks = all(
        sql,
        "SELECT track_id FROM activity_play_queue_tracks WHERE user=?1 ORDER BY position",
        params![user.as_str()],
        |row| row.get(0),
    )?;
    Ok(Some(RawQueue {
        tracks,
        current_index,
        position_ms,
        changed_by,
        changed_at,
    }))
}

pub(crate) fn save_queue(
    sql: &Connection,
    user: &UserName,
    tracks: &[&str],
    current_index: Option<usize>,
    position_ms: i64,
    changed_by: &str,
    now: i64,
) -> Fallible {
    if tracks.is_empty() {
        exec(
            sql,
            "DELETE FROM activity_play_queues WHERE user=?1",
            params![user.as_str()],
        )?;
        return Ok(());
    }
    let idx = current_index.map(as_i64);
    exec(
        sql,
        "INSERT INTO activity_play_queues(user,current_index,position_ms,changed_by,changed_at) \
         VALUES(?1,?2,?3,?4,?5) \
         ON CONFLICT(user) DO UPDATE SET \
           current_index=excluded.current_index, \
           position_ms=excluded.position_ms, \
           changed_by=excluded.changed_by, \
           changed_at=excluded.changed_at",
        params![user.as_str(), idx, position_ms, changed_by, now],
    )?;
    exec(
        sql,
        "DELETE FROM activity_play_queue_tracks WHERE user=?1",
        params![user.as_str()],
    )?;
    for (position, &track_id) in tracks.iter().enumerate() {
        exec(
            sql,
            "INSERT INTO activity_play_queue_tracks(user,position,track_id) VALUES(?1,?2,?3)",
            params![user.as_str(), as_i64(position), track_id],
        )?;
    }
    Ok(())
}

pub(crate) fn bookmarks_chunk(
    sql: &Connection,
    user: &UserName,
    offset: usize,
    limit: usize,
) -> Fallible<Vec<Bookmark>> {
    all(
        sql,
        "SELECT track_id,position_ms,comment,created_at,updated_at FROM activity_bookmarks \
         WHERE user=?1 ORDER BY updated_at DESC,track_id LIMIT ?2 OFFSET ?3",
        params![user.as_str(), as_i64(limit), as_i64(offset)],
        |row| {
            Ok(Bookmark {
                track: row.get(0)?,
                position_ms: u64_col(row, 1)?,
                comment: row.get(2)?,
                created_at: from_unix_millis(row.get(3)?),
                updated_at: from_unix_millis(row.get(4)?),
            })
        },
    )
}

pub(crate) fn upsert_bookmark(
    sql: &Connection,
    user: &UserName,
    track: &str,
    position_ms: i64,
    comment: Option<&str>,
    now: i64,
    max_per_user: usize,
) -> Fallible {
    let exists = optional(
        sql,
        "SELECT 1 FROM activity_bookmarks WHERE user=?1 AND track_id=?2",
        params![user.as_str(), track],
        |row| row.get::<_, i64>(0),
    )?
    .is_some();
    if !exists {
        let count: i64 = sql
            .prepare_cached("SELECT count(*) FROM activity_bookmarks WHERE user=?1")
            .map_err(query_error)?
            .query_row(params![user.as_str()], |row| row.get(0))
            .map_err(query_error)?;
        if usize::try_from(count).unwrap_or(usize::MAX) >= max_per_user {
            return Err(ActivityFailure::LimitReached(
                "Bookmark limit for this user has been reached.",
            )
            .boxed());
        }
    }
    exec(
        sql,
        "INSERT INTO activity_bookmarks(user,track_id,position_ms,comment,created_at,updated_at) \
         VALUES(?1,?2,?3,?4,?5,?5) \
         ON CONFLICT(user,track_id) DO UPDATE SET \
           position_ms=excluded.position_ms, \
           comment=excluded.comment, \
           updated_at=excluded.updated_at",
        params![user.as_str(), track, position_ms, comment, now],
    )?;
    Ok(())
}

pub(crate) fn delete_bookmark(sql: &Connection, user: &UserName, track: &str) -> Fallible {
    let changed = exec(
        sql,
        "DELETE FROM activity_bookmarks WHERE user=?1 AND track_id=?2",
        params![user.as_str(), track],
    )?;
    if changed == 0 {
        return Err(ActivityFailure::NotFound("The requested bookmark was not found.").boxed());
    }
    Ok(())
}
