//! The scanned filesystem tree, independent of the tagged catalog.

use crate::{Ctx, Fallible, Page, PageRequest, Port};
use std::ffi::OsString;
use std::path::PathBuf;

/// A configured music root.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LibraryRoot {
    /// Opaque root id.
    pub id: String,
    /// The folder id of the root directory.
    pub folder_id: String,
    /// Canonical root path.
    pub path: PathBuf,
}

/// A directory in a root.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Folder {
    /// Opaque folder id, carried on an unambiguous rename.
    pub id: String,
    /// The root containing this entry.
    pub root_id: String,
    /// The parent folder id, absent at a root.
    pub parent_id: Option<String>,
    /// The name of this directory on disk.
    pub name: OsString,
    /// Its path relative to the root.
    pub relative_path: PathBuf,
    /// Whether this folder directly contains a `.nomedia` or `.timbreignore` marker.
    pub ignored: bool,
    /// Ordered children in the requested page.
    pub children: Page<FolderEntry>,
}

/// A file or folder directly inside a directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FolderEntry {
    /// A directory.
    Folder {
        /// Opaque folder id.
        id: String,
        /// Name of the directory.
        name: OsString,
        /// Whether the directory has an ignore marker.
        ignored: bool,
    },
    /// A file.
    File {
        /// Name of the file.
        name: OsString,
        /// Type of the file and, for audio, its catalog id.
        kind: LibraryFile,
    },
}

/// A scanner-recognized file type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LibraryFile {
    /// A file indexed as audio.
    Audio {
        /// Id of the catalog track.
        track_id: String,
    },
    /// A recognized image file.
    Image,
    /// A lyrics sidecar.
    Lyrics,
}

/// Browse roots and their on-disk trees. Calls may block on storage, so
/// asynchronous callers should use [`Ctx::blocking`]. Every method serves at
/// most [`MAX_PAGE_LIMIT`](crate::MAX_PAGE_LIMIT) items; a larger `limit` is
/// refused with `kernel.page_too_large`. Concurrent inserts or deletes can
/// shift later offset pages.
pub trait LibraryTree: Port {
    /// Roots in configuration order, then id.
    ///
    /// # Errors
    /// Returns a failure if the roots cannot be read.
    fn roots(&self, ctx: &Ctx, page: PageRequest<'_>) -> Fallible<Page<LibraryRoot>>;

    /// One folder by id, with child folders first and then files, each ordered
    /// by filename (ties by id for folders).
    ///
    /// # Errors
    /// Returns a failure if the folder is absent or the page cannot be read.
    fn folder(&self, ctx: &Ctx, id: &str, page: PageRequest<'_>) -> Fallible<Folder>;

    /// The persistent tree version, changed by any visible tree change.
    ///
    /// # Errors
    /// Returns a failure if the version cannot be read.
    fn tree_version(&self, ctx: &Ctx) -> Fallible<u64>;
}
