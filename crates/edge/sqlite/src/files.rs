//! Owner-only file modes around the database and its backups.
//!
//! SQLite opens a database with the process umask applied, which on a shared
//! machine leaves the file (and its `-wal`/`-shm` sidecars) world-readable;
//! everything stored -- credentials included -- then belongs to every local
//! user. Both entry points here run once per file the edge creates.

use crate::failures::SqliteFailure;
use std::path::Path;
use timbre_kernel::Fallible;

/// The sidecar names SQLite may create next to a database it has open.
const SIDECARS: [&str; 3] = ["", "-wal", "-shm"];

/// The path with one of [`SIDECARS`] appended; `""` yields the path itself.
pub(crate) fn with_suffix(path: &Path, suffix: &str) -> std::path::PathBuf {
    if suffix.is_empty() {
        return path.to_path_buf();
    }
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    std::path::PathBuf::from(name)
}

pub(crate) fn open_failure(
    target: &Path,
    error: &std::io::Error,
) -> Box<dyn timbre_kernel::Failure> {
    Box::new(SqliteFailure::Open {
        reason: format!("{}: {error}", target.display()),
    }) as Box<dyn timbre_kernel::Failure>
}

/// Whether `a` and `b` name the same existing file. On Unix identity is
/// the device plus inode `metadata` reports after following symlinks, so a
/// symlink at the destination resolves to the file it guards against;
/// elsewhere the canonical path is the closest std equivalent.
#[cfg(unix)]
pub(crate) fn same_file(a: &Path, b: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    let (Ok(left), Ok(right)) = (std::fs::metadata(a), std::fs::metadata(b)) else {
        return false;
    };
    left.dev() == right.dev() && left.ino() == right.ino()
}

/// Whether `a` and `b` name the same existing file, by canonical path: the closest std
/// equivalent of device plus inode where those are unavailable.
#[cfg(not(unix))]
pub(crate) fn same_file(a: &Path, b: &Path) -> bool {
    let (Ok(left), Ok(right)) = (std::fs::canonicalize(a), std::fs::canonicalize(b)) else {
        return false;
    };
    left == right
}

/// Create `path` exclusively with mode 0600 (Unix): the file exists under
/// its final mode before a single byte is written, so a copy never has a
/// world-readable window, and a destination that raced in -- symlink or
/// not -- fails with `AlreadyExists` instead of being followed.
pub(crate) fn create_exclusive(path: &Path) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

/// Refuse to serve a database file a different user planted. `restrict`
/// already cannot raise the mode of a file we do not own, and opening a
/// foreign-owned file trusts whatever its owner arranged (hard links,
/// later renames). The expected owner is established without libc by
/// creating a probe file in the same directory and reading its uid.
///
/// On platforms without Unix ownership this is a no-op.
///
/// # Errors
///
/// `sqlite.open_failed` when the probe cannot be created, either file
/// cannot be stat'ed, or the owners differ.
#[cfg_attr(
    not(unix),
    expect(
        clippy::unnecessary_wraps,
        reason = "the Unix build returns ownership failures; the signature is shared"
    )
)]
pub(crate) fn verify_owner(path: &Path) -> Fallible {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        // The probe carries the pid: two processes opening the same data dir
        // concurrently must not make the other's probe look like a planted
        // obstacle.
        let probe = with_suffix(path, &format!(".owner-probe-{}", std::process::id()));
        let created = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&probe)
            .and_then(|file| file.metadata().map(|meta| meta.uid()));
        let probe_uid = match created {
            Ok(uid) => uid,
            Err(error) => {
                let _ = std::fs::remove_file(&probe);
                return Err(open_failure(&probe, &error));
            }
        };
        let db_uid = std::fs::metadata(path).map(|meta| meta.uid());
        let _ = std::fs::remove_file(&probe);
        let db_uid = match db_uid {
            Ok(uid) => uid,
            Err(error) => return Err(open_failure(path, &error)),
        };
        if db_uid != probe_uid {
            return Err(Box::new(SqliteFailure::Open {
                reason: format!(
                    "{} is owned by uid {db_uid}, not the running uid {probe_uid}",
                    path.display()
                ),
            }) as Box<dyn timbre_kernel::Failure>);
        }
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

/// Restrict `path` and its `-wal`/`-shm` sidecars to the owner (mode 0600).
/// A sidecar that does not exist yet (before the first write, or after a
/// clean close) is skipped; every other failure -- a vanished main file or a
/// refused chmod -- stops the open, because an unprotected database is the
/// very thing this function exists to prevent. On platforms without Unix
/// modes this is a no-op: file privacy there rides on the data directory's
/// ACLs.
#[cfg_attr(
    not(unix),
    expect(
        clippy::unnecessary_wraps,
        reason = "the Unix build returns chmod failures; the signature is shared"
    )
)]
pub(crate) fn restrict_to_owner(path: &Path) -> Fallible {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for suffix in SIDECARS {
            let target = with_suffix(path, suffix);
            match std::fs::metadata(&target) {
                Ok(_) => std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o600))
                    .map_err(|error| open_failure(&target, &error))?,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(open_failure(&target, &error)),
            }
        }
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

/// Remove `path` and its sidecars, ignoring the ones that do not exist: a
/// forced backup replaces whatever previous run left behind.
pub(crate) fn remove_with_sidecars(path: &Path) -> Fallible {
    for suffix in SIDECARS {
        let target = with_suffix(path, suffix);
        match std::fs::remove_file(&target) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(open_failure(&target, &error)),
        }
    }
    Ok(())
}
