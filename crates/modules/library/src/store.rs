use crate::failures::LibraryFailure;
use std::path::{Path, PathBuf};
use timbre_edge_sqlite::failures::query_error;
use timbre_edge_sqlite::rusqlite::{Connection, OptionalExtension as _, Params, Row};
use timbre_kernel::Fallible;

pub(crate) fn one<T>(
    sql: &Connection,
    query: &str,
    values: impl Params,
    map: impl FnOnce(&Row<'_>) -> timbre_edge_sqlite::rusqlite::Result<T>,
) -> Fallible<T> {
    sql.prepare_cached(query)
        .map_err(query_error)?
        .query_row(values, map)
        .optional()
        .map_err(query_error)?
        .ok_or_else(|| LibraryFailure::NotFound.boxed())
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

pub(crate) fn diagnose(sql: &Connection, track: i64, kind: &str, detail: &str) -> Fallible {
    exec(
        sql,
        "INSERT OR REPLACE INTO library_diagnostics(track_id,kind,detail) VALUES(?1,?2,?3)",
        timbre_edge_sqlite::rusqlite::params![track, kind, detail],
    )?;
    Ok(())
}

pub(crate) fn id(value: &str) -> Fallible<i64> {
    let parsed: i64 = value
        .parse()
        .map_err(|_| LibraryFailure::NotFound.boxed())?;
    if parsed <= 0 || parsed.to_string() != value {
        return Err(LibraryFailure::NotFound.boxed());
    }
    Ok(parsed)
}

pub(crate) fn integer(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

pub(crate) fn unsigned(row: &Row<'_>, index: usize) -> timbre_edge_sqlite::rusqlite::Result<u64> {
    let value: i64 = row.get(index)?;
    u64::try_from(value)
        .map_err(|_| timbre_edge_sqlite::rusqlite::Error::IntegralValueOutOfRange(index, value))
}

#[cfg(unix)]
pub(crate) fn path_to_bytes(path: &Path) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    path.as_os_str().as_bytes().to_vec()
}

#[cfg(unix)]
fn bytes_to_path(bytes: &[u8]) -> PathBuf {
    use std::os::unix::ffi::OsStrExt;
    Path::new(std::ffi::OsStr::from_bytes(bytes)).to_path_buf()
}

#[cfg(windows)]
pub(crate) fn path_to_bytes(path: &Path) -> Vec<u8> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str()
        .encode_wide()
        .flat_map(u16::to_le_bytes)
        .collect()
}

#[cfg(windows)]
fn bytes_to_path(bytes: &[u8]) -> PathBuf {
    use std::os::windows::ffi::OsStringExt;
    let units: Vec<_> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .copied()
        .map(u16::from_le_bytes)
        .collect();
    std::ffi::OsString::from_wide(&units).into()
}

pub(crate) fn row_path(
    row: &Row<'_>,
    column: usize,
) -> timbre_edge_sqlite::rusqlite::Result<PathBuf> {
    row.get_ref(column)?
        .as_blob()
        .map(bytes_to_path)
        .map_err(|error| {
            timbre_edge_sqlite::rusqlite::Error::FromSqlConversionFailure(
                column,
                timbre_edge_sqlite::rusqlite::types::Type::Blob,
                Box::new(error),
            )
        })
}

pub(crate) fn optional_path(
    row: &Row<'_>,
    column: usize,
) -> timbre_edge_sqlite::rusqlite::Result<Option<PathBuf>> {
    use timbre_edge_sqlite::rusqlite::types::ValueRef;
    if row.get_ref(column)? == ValueRef::Null {
        Ok(None)
    } else {
        row_path(row, column).map(Some)
    }
}

pub(crate) fn contained(path: &Path, root: &Path) -> Fallible<PathBuf> {
    path.canonicalize()
        .ok()
        .filter(|candidate| candidate.starts_with(root) && candidate.is_file())
        .ok_or_else(|| LibraryFailure::NotFound.boxed())
}
