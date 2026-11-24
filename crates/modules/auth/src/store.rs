//! The stored side of users and permissions, and the small helpers the
//! device queries share.

use crate::failures::AuthFailure;
use timbre_edge_sqlite::failures::query_error;
use timbre_edge_sqlite::rusqlite::{Connection, OptionalExtension as _, Params, Row, params};
use timbre_kernel::identity::{Permission, UserName};
use timbre_kernel::{Fallible, page::Window};

/// A stored account.
pub(crate) struct UserRow {
    pub(crate) id: i64,
    pub(crate) name: String,
    pub(crate) admin: bool,
    pub(crate) created: i64,
}

pub(crate) fn optional<T>(
    sql: &Connection,
    query: &str,
    values: impl Params,
    map: impl FnOnce(&Row<'_>) -> timbre_edge_sqlite::rusqlite::Result<T>,
) -> Fallible<Option<T>> {
    sql.query_row(query, values, map)
        .optional()
        .map_err(query_error)
}

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

pub(crate) fn exec(sql: &Connection, query: &str, values: impl Params) -> Fallible<usize> {
    sql.prepare_cached(query)
        .map_err(query_error)?
        .execute(values)
        .map_err(query_error)
}

fn user_row(row: &Row<'_>) -> timbre_edge_sqlite::rusqlite::Result<UserRow> {
    Ok(UserRow {
        id: row.get(0)?,
        name: row.get(1)?,
        admin: row.get::<_, i64>(2)? != 0,
        created: row.get(3)?,
    })
}

pub(crate) fn user_by_name(sql: &Connection, name: &UserName) -> Fallible<Option<UserRow>> {
    optional(
        sql,
        "SELECT id,name,admin,created FROM auth_users WHERE name=?1",
        params![name.as_str()],
        user_row,
    )
}

/// The account, or `auth.not_found`.
pub(crate) fn require_user(sql: &Connection, name: &UserName) -> Fallible<UserRow> {
    user_by_name(sql, name)?.ok_or_else(|| {
        AuthFailure::NotFound {
            kind: "user",
            value: name.as_str().to_owned(),
        }
        .boxed()
    })
}

pub(crate) fn users_page(sql: &Connection, window: Window) -> Fallible<Vec<UserRow>> {
    all(
        sql,
        "SELECT id,name,admin,created FROM auth_users ORDER BY name LIMIT ?1 OFFSET ?2",
        params![window.limit(), window.skip()],
        user_row,
    )
}

pub(crate) fn insert_user(
    sql: &Connection,
    name: &UserName,
    admin: bool,
    now: i64,
) -> Fallible<i64> {
    if user_by_name(sql, name)?.is_some() {
        return Err(AuthFailure::Exists(name.as_str().to_owned()).boxed());
    }
    exec(
        sql,
        "INSERT INTO auth_users(name,admin,created) VALUES(?1,?2,?3)",
        params![name.as_str(), i64::from(admin), now],
    )?;
    Ok(sql.last_insert_rowid())
}

pub(crate) fn delete_user(sql: &Connection, id: i64) -> Fallible {
    exec(sql, "DELETE FROM auth_users WHERE id=?1", params![id])?;
    Ok(())
}

pub(crate) fn permissions_of(sql: &Connection, user: i64) -> Fallible<Vec<String>> {
    all(
        sql,
        "SELECT permission FROM auth_user_permissions WHERE user_id=?1 ORDER BY permission",
        params![user],
        |row| row.get(0),
    )
}

/// Store `permission` for `user`; `false` when the user already had it.
pub(crate) fn grant(sql: &Connection, user: i64, permission: Permission) -> Fallible<bool> {
    let changed = exec(
        sql,
        "INSERT OR IGNORE INTO auth_user_permissions(user_id,permission) VALUES(?1,?2)",
        params![user, permission.as_str()],
    )?;
    Ok(changed != 0)
}

/// Drop `permission` from `user`; `false` when they did not have it.
pub(crate) fn revoke(sql: &Connection, user: i64, permission: &str) -> Fallible<bool> {
    let changed = exec(
        sql,
        "DELETE FROM auth_user_permissions WHERE user_id=?1 AND permission=?2",
        params![user, permission],
    )?;
    Ok(changed != 0)
}
