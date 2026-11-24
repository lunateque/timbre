//! The journal table: appending entries, pruning old ones, reading them
//! back newest first.

use crate::queue;
use timbre_edge_sqlite::failures::query_error;
use timbre_edge_sqlite::rusqlite::{Connection, Params, Row, params};
use timbre_kernel::Fallible;

/// One stored journal row.
pub(crate) struct Record {
    pub(crate) time: i64,
    pub(crate) kind: String,
    pub(crate) user: Option<String>,
    pub(crate) client: Option<String>,
    pub(crate) detail: String,
    pub(crate) count: i64,
}

/// What a reader asks for.
#[derive(Default)]
pub(crate) struct Query {
    pub(crate) since: Option<i64>,
    pub(crate) user: Option<String>,
    pub(crate) kind: Option<String>,
    pub(crate) limit: usize,
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

/// Append `entries` to the journal.
pub(crate) fn append(sql: &Connection, entries: &[queue::Row]) -> Fallible {
    let mut statement = sql
        .prepare_cached(
            "INSERT INTO audit_events(time,kind,user,client,detail,count) VALUES(?1,?2,?3,?4,?5,?6)",
        )
        .map_err(query_error)?;
    for entry in entries {
        statement
            .execute(params![
                entry.time,
                entry.key.kind,
                entry.key.user,
                entry.key.client,
                entry.key.detail,
                entry.count,
            ])
            .map_err(query_error)?;
    }
    Ok(())
}

/// Delete rows stamped before `cutoff`, returning how many went.
pub(crate) fn prune(sql: &Connection, cutoff: i64) -> Fallible<usize> {
    sql.prepare_cached("DELETE FROM audit_events WHERE time < ?1")
        .map_err(query_error)?
        .execute(params![cutoff])
        .map_err(query_error)
}

/// The matching rows, newest first.
pub(crate) fn read(sql: &Connection, query: &Query) -> Fallible<Vec<Record>> {
    // Every filter is a bound parameter that is simply ignored when absent,
    // so one prepared statement serves every combination.
    all(
        sql,
        "SELECT time,kind,user,client,detail,count FROM audit_events \
         WHERE (?1 IS NULL OR time >= ?1) AND (?2 IS NULL OR user = ?2) \
         AND (?3 IS NULL OR kind = ?3) ORDER BY time DESC, id DESC LIMIT ?4",
        params![
            query.since,
            query.user,
            query.kind,
            i64::try_from(query.limit).unwrap_or(i64::MAX)
        ],
        |row| {
            Ok(Record {
                time: row.get(0)?,
                kind: row.get(1)?,
                user: row.get(2)?,
                client: row.get(3)?,
                detail: row.get(4)?,
                count: row.get(5)?,
            })
        },
    )
}
