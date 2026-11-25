//! User-owned playlists of ordered catalog track ids.
//!
//! Access control lives inside the port implementation: every operation names
//! the acting user, private playlists are invisible to other users (returning
//! not found rather than revealing existence), and only the owner may modify
//! or delete a playlist. Entries reference catalog tracks without foreign keys
//! and skip stale ids when a [`Catalog`](crate::catalog::Catalog) port is
//! registered.

use crate::identity::UserName;
use crate::page::{Page, PageRequest};
use crate::{Ctx, Fallible, Port};
use std::time::{Duration, SystemTime};

/// Metadata of one playlist without its track entries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlaylistSummary {
    /// Opaque playlist id.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Optional user description.
    pub comment: Option<String>,
    /// Canonical user who owns the playlist.
    pub owner: UserName,
    /// Whether other users may view this playlist.
    pub public: bool,
    /// Number of visible entries (live tracks when a catalog is registered).
    pub song_count: u64,
    /// Combined duration of visible tracks (`Duration::ZERO` without a catalog).
    pub duration: Duration,
    /// When the playlist was created.
    pub created_at: SystemTime,
    /// When the playlist metadata or entries last changed.
    pub updated_at: SystemTime,
}

/// A playlist's summary alongside one page of its ordered track ids.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlaylistDetail {
    /// Playlist metadata.
    pub summary: PlaylistSummary,
    /// Ordered opaque track ids (duplicates preserved).
    pub entries: Page<String>,
}

/// Input for creating a new playlist owned by the acting user.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CreatePlaylist<'a> {
    /// Playlist title (trimmed, non-empty, bounded).
    pub name: &'a str,
    /// Optional description.
    pub comment: Option<&'a str>,
    /// Whether other users may view the playlist.
    pub public: bool,
    /// Initial ordered track ids (duplicates allowed).
    pub tracks: &'a [&'a str],
}

/// Incremental or replacement edits to an existing playlist.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UpdatePlaylist<'a> {
    /// New title, when changing it.
    pub name: Option<&'a str>,
    /// `None` leaves the comment unchanged, `Some(None)` clears it, and
    /// `Some(Some(text))` replaces it.
    pub comment: Option<Option<&'a str>>,
    /// New visibility, when changing it.
    pub public: Option<bool>,
    /// Replace every entry with this ordered list before applying any
    /// `remove_indices` or `add_tracks`.
    pub replace_tracks: Option<&'a [&'a str]>,
    /// Track ids appended to the end of the playlist.
    pub add_tracks: &'a [&'a str],
    /// Zero-based indices to remove from the visible entry list before
    /// `add_tracks` are appended.
    pub remove_indices: &'a [usize],
}

/// Read and manage user playlists.
///
/// Calls may block on storage; asynchronous callers should use
/// [`Ctx::blocking`](crate::Ctx::blocking). Every paged method enforces
/// [`MAX_PAGE_LIMIT`](crate::MAX_PAGE_LIMIT) and refuses larger limits with
/// `kernel.page_too_large`.
pub trait Playlists: Port {
    /// Playlists visible to `actor`, ordered by name then id.
    ///
    /// When `owner` is `None`, returns playlists owned by `actor` together
    /// with public playlists owned by other users. When `owner` is `Some(u)`,
    /// returns every playlist of `u` if `u == actor` and only `u`'s public
    /// playlists otherwise.
    ///
    /// # Errors
    /// Returns a failure if the page cannot be read or `limit` exceeds the cap.
    fn list(
        &self,
        ctx: &Ctx,
        actor: &UserName,
        owner: Option<&UserName>,
        page: PageRequest<'_>,
    ) -> Fallible<Page<PlaylistSummary>>;

    /// One playlist visible to `actor` and a page of its ordered track ids.
    ///
    /// Another user's private playlist is reported as not found so callers
    /// cannot probe playlist ids.
    ///
    /// # Errors
    /// Returns a failure if `id` is invalid, the playlist is absent or
    /// invisible to `actor`, `entries.limit` exceeds the cap, a present
    /// principal does not match `actor`, or storage fails.
    fn get(
        &self,
        ctx: &Ctx,
        actor: &UserName,
        id: &str,
        entries: PageRequest<'_>,
    ) -> Fallible<PlaylistDetail>;

    /// Create a playlist owned by `actor`.
    ///
    /// # Errors
    /// Returns a failure if input validation fails, per-user or per-playlist
    /// caps would be exceeded, or storage fails.
    fn create(
        &self,
        ctx: &Ctx,
        actor: &UserName,
        input: &CreatePlaylist<'_>,
    ) -> Fallible<PlaylistSummary>;

    /// Edit an existing playlist owned by `actor`.
    ///
    /// Targeting another user's private playlist fails as not found; targeting
    /// another user's public playlist fails as forbidden.
    ///
    /// # Errors
    /// Returns a failure if the playlist is absent, not owned by `actor`, any
    /// edit or index is invalid, caps would be exceeded, or storage fails.
    fn update(
        &self,
        ctx: &Ctx,
        actor: &UserName,
        id: &str,
        changes: &UpdatePlaylist<'_>,
    ) -> Fallible<PlaylistSummary>;

    /// Delete a playlist owned by `actor`.
    ///
    /// Targeting another user's private playlist fails as not found; targeting
    /// another user's public playlist fails as forbidden.
    ///
    /// # Errors
    /// Returns a failure if `id` is invalid, the playlist is absent, not owned
    /// by `actor`, a present principal does not match `actor`, or storage fails.
    fn delete(&self, ctx: &Ctx, actor: &UserName, id: &str) -> Fallible;
}
