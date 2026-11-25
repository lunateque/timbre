//! SQLite queries and transactions for stars, ratings and user cleanup.

pub(crate) mod playback;

use crate::failures::ActivityFailure;
use crate::validate::kind_code;
use timbre_edge_sqlite::failures::query_error;
use timbre_edge_sqlite::rusqlite::{Connection, OptionalExtension as _, Params, Row, params};
use timbre_kernel::Fallible;
use timbre_kernel::activity::{ItemKind, RatedItem, StarredItem};
use timbre_kernel::clock::from_unix_millis;
use timbre_kernel::identity::UserName;

pub(crate) fn all<T>(
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

pub(crate) fn optional<T>(
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

pub(crate) fn exec(sql: &Connection, query: &str, values: impl Params) -> Fallible<usize> {
    sql.prepare_cached(query)
        .map_err(query_error)?
        .execute(values)
        .map_err(query_error)
}

pub(crate) fn as_i64(value: usize) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

pub(crate) fn u64_col(row: &Row<'_>, idx: usize) -> timbre_edge_sqlite::rusqlite::Result<u64> {
    let raw: i64 = row.get(idx)?;
    u64::try_from(raw)
        .map_err(|_| timbre_edge_sqlite::rusqlite::Error::IntegralValueOutOfRange(idx, raw))
}

fn count_rows(sql: &Connection, query: &str, user: &UserName) -> Fallible<usize> {
    let raw: i64 = sql
        .prepare_cached(query)
        .map_err(query_error)?
        .query_row(params![user.as_str()], |row| row.get(0))
        .map_err(query_error)?;
    Ok(usize::try_from(raw).unwrap_or(usize::MAX))
}

pub(crate) fn star_many(
    sql: &Connection,
    user: &UserName,
    kind: ItemKind,
    ids: &[String],
    now: i64,
    max_per_user: usize,
) -> Fallible<usize> {
    let code = kind_code(kind);
    let mut existing = count_rows(
        sql,
        "SELECT count(*) FROM activity_stars WHERE user=?1",
        user,
    )?;
    let mut inserted = 0usize;
    for id in ids {
        let already = optional(
            sql,
            "SELECT 1 FROM activity_stars WHERE user=?1 AND kind=?2 AND item_id=?3",
            params![user.as_str(), code, id.as_str()],
            |row| row.get::<_, i64>(0),
        )?
        .is_some();
        if already {
            continue;
        }
        if existing >= max_per_user {
            return Err(ActivityFailure::LimitReached(
                "Star limit for this user has been reached.",
            )
            .boxed());
        }
        inserted += exec(
            sql,
            "INSERT OR IGNORE INTO activity_stars(user,kind,item_id,starred_at) VALUES(?1,?2,?3,?4)",
            params![user.as_str(), code, id.as_str(), now],
        )?;
        existing += 1;
    }
    Ok(inserted)
}

pub(crate) fn unstar_many(
    sql: &Connection,
    user: &UserName,
    kind: ItemKind,
    ids: &[&str],
) -> Fallible<usize> {
    let mut removed = 0usize;
    let code = kind_code(kind);
    for &id in ids {
        removed += exec(
            sql,
            "DELETE FROM activity_stars WHERE user=?1 AND kind=?2 AND item_id=?3",
            params![user.as_str(), code, id],
        )?;
    }
    Ok(removed)
}

pub(crate) fn starred_chunk(
    sql: &Connection,
    user: &UserName,
    kind: ItemKind,
    offset: usize,
    limit: usize,
) -> Fallible<Vec<StarredItem>> {
    all(
        sql,
        "SELECT item_id,starred_at FROM activity_stars \
         WHERE user=?1 AND kind=?2 ORDER BY starred_at DESC,item_id LIMIT ?3 OFFSET ?4",
        params![
            user.as_str(),
            kind_code(kind),
            as_i64(limit),
            as_i64(offset)
        ],
        |row| {
            Ok(StarredItem {
                id: row.get(0)?,
                starred_at: from_unix_millis(row.get(1)?),
            })
        },
    )
}

pub(crate) fn starred_lookup(
    sql: &Connection,
    user: &UserName,
    kind: ItemKind,
    sorted_ids: &[&str],
) -> Fallible<Vec<StarredItem>> {
    let mut found = Vec::new();
    let code = kind_code(kind);
    for &id in sorted_ids {
        if let Some(at) = optional(
            sql,
            "SELECT starred_at FROM activity_stars WHERE user=?1 AND kind=?2 AND item_id=?3",
            params![user.as_str(), code, id],
            |row| row.get::<_, i64>(0),
        )? {
            found.push(StarredItem {
                id: id.to_owned(),
                starred_at: from_unix_millis(at),
            });
        }
    }
    Ok(found)
}

pub(crate) fn set_rating(
    sql: &Connection,
    user: &UserName,
    kind: ItemKind,
    id: &str,
    rating: u8,
    now: i64,
    max_per_user: usize,
) -> Fallible {
    let code = kind_code(kind);
    if rating == 0 {
        exec(
            sql,
            "DELETE FROM activity_ratings WHERE user=?1 AND kind=?2 AND item_id=?3",
            params![user.as_str(), code, id],
        )?;
        return Ok(());
    }
    let already = optional(
        sql,
        "SELECT 1 FROM activity_ratings WHERE user=?1 AND kind=?2 AND item_id=?3",
        params![user.as_str(), code, id],
        |row| row.get::<_, i64>(0),
    )?
    .is_some();
    if !already {
        let count = count_rows(
            sql,
            "SELECT count(*) FROM activity_ratings WHERE user=?1",
            user,
        )?;
        if count >= max_per_user {
            return Err(ActivityFailure::LimitReached(
                "Rating limit for this user has been reached.",
            )
            .boxed());
        }
    }
    exec(
        sql,
        "INSERT INTO activity_ratings(user,kind,item_id,rating,updated_at) \
         VALUES(?1,?2,?3,?4,?5) \
         ON CONFLICT(user,kind,item_id) DO UPDATE SET rating=excluded.rating,updated_at=excluded.updated_at",
        params![user.as_str(), code, id, i64::from(rating), now],
    )?;
    Ok(())
}

pub(crate) fn ratings_lookup(
    sql: &Connection,
    user: &UserName,
    kind: ItemKind,
    sorted_ids: &[&str],
) -> Fallible<Vec<RatedItem>> {
    let mut found = Vec::new();
    let code = kind_code(kind);
    for &id in sorted_ids {
        let agg: (i64, Option<f64>) = sql
            .prepare_cached(
                "SELECT count(*),avg(CAST(rating AS REAL)) FROM activity_ratings WHERE kind=?1 AND item_id=?2",
            )
            .map_err(query_error)?
            .query_row(params![code, id], |row| Ok((row.get(0)?, row.get(1)?)))
            .map_err(query_error)?;
        let count = u64::try_from(agg.0).unwrap_or_default();
        let Some(average) = agg.1.filter(|_| count > 0) else {
            continue;
        };
        let mine = optional(
            sql,
            "SELECT rating FROM activity_ratings WHERE user=?1 AND kind=?2 AND item_id=?3",
            params![user.as_str(), code, id],
            |row| row.get::<_, i64>(0),
        )?
        .and_then(|val| u8::try_from(val).ok());
        found.push(RatedItem {
            id: id.to_owned(),
            user_rating: mine,
            average,
            count,
        });
    }
    Ok(found)
}

pub(crate) fn delete_user(sql: &Connection, user: &UserName) -> Fallible<usize> {
    let name = user.as_str();
    let mut rows = 0usize;
    rows += exec(
        sql,
        "DELETE FROM activity_stars WHERE user=?1",
        params![name],
    )?;
    rows += exec(
        sql,
        "DELETE FROM activity_ratings WHERE user=?1",
        params![name],
    )?;
    rows += exec(
        sql,
        "DELETE FROM activity_track_plays WHERE user=?1",
        params![name],
    )?;
    rows += exec(
        sql,
        "DELETE FROM activity_album_plays WHERE user=?1",
        params![name],
    )?;
    rows += exec(
        sql,
        "DELETE FROM activity_play_queues WHERE user=?1",
        params![name],
    )?;
    rows += exec(
        sql,
        "DELETE FROM activity_bookmarks WHERE user=?1",
        params![name],
    )?;
    Ok(rows)
}
