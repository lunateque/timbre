//! User-facing folder browsing, derived from the scanned tree.

use std::ffi::OsString;

use crate::library_tree::{Folder, LibraryRoot};
use crate::{Ctx, Fallible, Page, PageRequest, Port};

/// A top directory shown in the alphabetic index.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndexedFolder {
    /// Opaque, rename-stable directory id.
    pub id: String,
    /// Directory name on disk.
    pub name: OsString,
}

/// Top directories with the same first letter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FolderIndex {
    /// Uppercase initial, or `#` for a non-letter.
    pub letter: char,
    /// In root configuration order, then scanned directory order.
    pub folders: Vec<IndexedFolder>,
}

/// A snapshot identified by the library tree version.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Versioned<T> {
    /// The requested result and its version.
    Changed {
        /// Tree version observed before reading the result.
        version: u64,
        /// Current result.
        value: T,
    },
    /// The supplied version is the current tree version.
    NotModified,
}

/// Browse visible directories; implementations may block on storage, so
/// asynchronous callers should use [`Ctx::blocking`]. Every method serves at
/// most [`MAX_PAGE_LIMIT`](crate::MAX_PAGE_LIMIT) items; a larger `limit` is
/// refused with `kernel.page_too_large`. Concurrent inserts or deletes can
/// shift later offset pages.
pub trait Folders: Port {
    /// Music roots in configuration order, then id.
    ///
    /// # Errors
    /// Returns a failure if the library tree is unavailable or cannot be read.
    fn roots(&self, ctx: &Ctx, page: PageRequest<'_>) -> Fallible<Page<LibraryRoot>>;

    /// Index groups ordered by their letter, optionally restricted to one root.
    /// If `if_modified_since` equals the current tree version, returns `NotModified`.
    ///
    /// # Errors
    /// Returns a failure if the tree is unavailable, the root is absent or results cannot be read.
    fn indexes(
        &self,
        ctx: &Ctx,
        root: Option<&str>,
        if_modified_since: Option<u64>,
        page: PageRequest<'_>,
    ) -> Fallible<Versioned<Page<FolderIndex>>>;

    /// Visible children in scanned directory order (directories first, then files).
    /// If `if_modified_since` equals the current tree version, returns `NotModified`.
    ///
    /// # Errors
    /// Returns a failure if the tree is unavailable, the directory is absent or hidden, or results cannot be read.
    fn folder(
        &self,
        ctx: &Ctx,
        id: &str,
        if_modified_since: Option<u64>,
        page: PageRequest<'_>,
    ) -> Fallible<Versioned<Folder>>;
}
