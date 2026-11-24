//! The stored side of devices. Only the digest of a secret is ever written
//! here, and only the prefix is ever read back as text.

use crate::failures::AuthFailure;
use crate::store::{all, exec, optional};
use timbre_edge_crypto::Digest;
use timbre_edge_sqlite::rusqlite::{Connection, Row, params};
use timbre_kernel::{Fallible, page::Window};

/// A device as the directory and the CLI show it.
pub(crate) struct DeviceRow {
    pub(crate) id: String,
    pub(crate) user: String,
    pub(crate) label: String,
    pub(crate) created: i64,
    pub(crate) expires: Option<i64>,
    pub(crate) last_used: Option<i64>,
    pub(crate) last_client: Option<String>,
}

/// What the credential check needs about the device a prefix names.
pub(crate) struct Candidate {
    pub(crate) digest: Vec<u8>,
    pub(crate) expires: Option<i64>,
    pub(crate) user: String,
    pub(crate) user_id: i64,
    pub(crate) admin: bool,
}

const COLUMNS: &str = "d.id,u.name,d.label,d.created,d.expires,d.last_used,d.last_client \
     FROM auth_devices d JOIN auth_users u ON u.id=d.user_id";

fn device_row(row: &Row<'_>) -> timbre_edge_sqlite::rusqlite::Result<DeviceRow> {
    Ok(DeviceRow {
        id: row.get(0)?,
        user: row.get(1)?,
        label: row.get(2)?,
        created: row.get(3)?,
        expires: row.get(4)?,
        last_used: row.get(5)?,
        last_client: row.get(6)?,
    })
}

pub(crate) fn by_prefix(sql: &Connection, prefix: &str) -> Fallible<Option<Candidate>> {
    optional(
        sql,
        "SELECT d.digest,d.expires,u.name,u.id,u.admin \
         FROM auth_devices d JOIN auth_users u ON u.id=d.user_id WHERE d.prefix=?1",
        params![prefix],
        |row| {
            Ok(Candidate {
                digest: row.get(0)?,
                expires: row.get(1)?,
                user: row.get(2)?,
                user_id: row.get(3)?,
                admin: row.get::<_, i64>(4)? != 0,
            })
        },
    )
}

pub(crate) fn by_id(sql: &Connection, id: &str) -> Fallible<Option<DeviceRow>> {
    optional(
        sql,
        &format!("SELECT {COLUMNS} WHERE d.id=?1"),
        params![id],
        device_row,
    )
}

pub(crate) fn require(sql: &Connection, id: &str) -> Fallible<DeviceRow> {
    by_id(sql, id)?.ok_or_else(|| {
        AuthFailure::NotFound {
            kind: "device",
            value: id.to_owned(),
        }
        .boxed()
    })
}

pub(crate) fn of_user(sql: &Connection, user: i64, window: Window) -> Fallible<Vec<DeviceRow>> {
    all(
        sql,
        &format!("SELECT {COLUMNS} WHERE d.user_id=?1 ORDER BY d.created,d.id LIMIT ?2 OFFSET ?3"),
        params![user, window.limit(), window.skip()],
        device_row,
    )
}

pub(crate) fn every(sql: &Connection, window: Window) -> Fallible<Vec<DeviceRow>> {
    all(
        sql,
        &format!("SELECT {COLUMNS} ORDER BY u.name,d.created,d.id LIMIT ?1 OFFSET ?2"),
        params![window.limit(), window.skip()],
        device_row,
    )
}

pub(crate) fn ids_of_user(sql: &Connection, user: i64) -> Fallible<Vec<String>> {
    all(
        sql,
        "SELECT id FROM auth_devices WHERE user_id=?1 ORDER BY created,id",
        params![user],
        |row| row.get(0),
    )
}

pub(crate) fn count_for_user(sql: &Connection, user: i64) -> Fallible<i64> {
    sql.query_row(
        "SELECT count(*) FROM auth_devices WHERE user_id=?1",
        params![user],
        |row| row.get(0),
    )
    .map_err(timbre_edge_sqlite::failures::query_error)
}

#[derive(Clone, Copy)]
pub(crate) struct NewDevice<'a> {
    pub(crate) id: &'a str,
    pub(crate) user: i64,
    pub(crate) label: &'a str,
    pub(crate) prefix: &'a str,
    pub(crate) digest: &'a Digest,
    pub(crate) created: i64,
    pub(crate) expires: Option<i64>,
}

pub(crate) fn insert(sql: &Connection, device: NewDevice<'_>) -> Fallible {
    exec(
        sql,
        "INSERT INTO auth_devices(id,user_id,label,prefix,digest,created,expires) \
         VALUES(?1,?2,?3,?4,?5,?6,?7)",
        params![
            device.id,
            device.user,
            device.label,
            device.prefix,
            device.digest.as_bytes().as_slice(),
            device.created,
            device.expires
        ],
    )?;
    Ok(())
}

/// Rotate the secret of an existing device. The id, label and expiry stay as
/// they are - the point of a reissue is that everything referring to the
/// device keeps referring to it - and the recorded last use is cleared,
/// because it belongs to the secret that just stopped working.
pub(crate) fn rotate(sql: &Connection, id: &str, prefix: &str, digest: &Digest) -> Fallible {
    exec(
        sql,
        "UPDATE auth_devices SET prefix=?2,digest=?3,last_used=NULL,last_client=NULL WHERE id=?1",
        params![id, prefix, digest.as_bytes().as_slice()],
    )?;
    Ok(())
}

pub(crate) fn delete(sql: &Connection, id: &str) -> Fallible {
    exec(sql, "DELETE FROM auth_devices WHERE id=?1", params![id])?;
    Ok(())
}

/// Update only the secret that was verified. A rotation or deletion in
/// between makes this note stale rather than activity on the new secret.
pub(crate) fn record_use(
    sql: &Connection,
    prefix: &str,
    when: i64,
    client: Option<&str>,
) -> Fallible {
    exec(
        sql,
        "UPDATE auth_devices SET last_used=?2,last_client=?3 WHERE prefix=?1",
        params![prefix, when, client],
    )?;
    Ok(())
}
