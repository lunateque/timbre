//! Per-user listening activity: stars, ratings, plays, now playing, play
//! queues and bookmarks.
//!
//! Rows reference catalog entities by opaque id without foreign keys so the
//! module works even when `library` is absent and tolerates removed tracks,
//! albums or artists by skipping stale ids on read when a
//! [`Catalog`](crate::catalog::Catalog) port is registered.

use crate::identity::UserName;
use crate::page::{Page, PageRequest};
use crate::{Ctx, Fallible, Port};
use std::time::SystemTime;

/// Which catalog entity a star or rating targets.
///
/// Separate kinds keep namespaces disjoint when a catalog issues numeric ids
/// per entity table.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ItemKind {
    /// A scanned track.
    Track,
    /// A scanned album.
    Album,
    /// A scanned artist.
    Artist,
}

/// Which activity-backed album list to read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlbumActivityList {
    /// Albums played most often by the user, then most recently, then by id.
    Frequent,
    /// Albums played most recently by the user, then by id.
    Recent,
    /// Highest average-rated albums across users, then rating count, then id.
    Highest,
    /// Albums starred by the user, most recently starred first, then by id.
    Starred,
}

/// One starred catalog item and when the user starred it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StarredItem {
    /// Opaque catalog id.
    pub id: String,
    /// When the user starred this item.
    pub starred_at: SystemTime,
}

/// Rating state for one rated catalog item (`count >= 1`).
#[derive(Clone, Debug, PartialEq)]
pub struct RatedItem {
    /// Opaque catalog id.
    pub id: String,
    /// The requesting user's own rating (`1..=5`), if set.
    pub user_rating: Option<u8>,
    /// Mean rating across all users who rated this item.
    pub average: f64,
    /// Number of users who currently rate this item.
    pub count: u64,
}

/// Play counts and recency for one track.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrackStats {
    /// Opaque catalog track id.
    pub track: String,
    /// Total completed play submissions.
    pub play_count: u64,
    /// Most recent play submission timestamp (`None` on global aggregates so
    /// cross-user queries do not leak exact listening timestamps).
    pub last_played: Option<SystemTime>,
}

/// Play counts and recency for one album.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AlbumStats {
    /// Opaque catalog album id.
    pub album: String,
    /// Total completed play submissions for tracks on this album by the user.
    pub play_count: u64,
    /// Most recent play submission timestamp for this album.
    pub last_played: Option<SystemTime>,
}

/// One active non-submission playback report.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NowPlayingEntry {
    /// Canonical user who reported playback.
    pub user: UserName,
    /// Opaque catalog track id.
    pub track: String,
    /// Client application label reported with the playback.
    pub client: String,
    /// When the playback report was received.
    pub updated_at: SystemTime,
}

/// A playback report: either a completed play submission or a now-playing
/// status update.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Scrobble<'a> {
    /// Opaque catalog track id.
    pub track: &'a str,
    /// Client application label (non-empty, bounded).
    pub client: &'a str,
    /// Client-reported timestamp for a submission; defaults to server time.
    pub at: Option<SystemTime>,
    /// `true` records a play and publishes [`Played`](crate::signals::Played);
    /// `false` updates in-memory now-playing state only.
    pub submission: bool,
}

/// A user's saved playback queue.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlayQueue {
    /// Ordered opaque track ids.
    pub tracks: Vec<String>,
    /// Zero-based index of the current track inside `tracks`, if set.
    pub current_index: Option<usize>,
    /// Playback position inside the current track, in milliseconds.
    pub position_ms: u64,
    /// Client label that last saved the queue.
    pub changed_by: String,
    /// When the queue was last saved.
    pub changed_at: SystemTime,
}

/// Input for saving or clearing a user's play queue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SavePlayQueue<'a> {
    /// Ordered opaque track ids; an empty slice clears the saved queue.
    pub tracks: &'a [&'a str],
    /// Zero-based index into `tracks` for the current track, if set.
    pub current_index: Option<usize>,
    /// Playback position inside the current track, in milliseconds.
    pub position_ms: u64,
    /// Client label saving the queue.
    pub changed_by: &'a str,
}

/// A saved playback bookmark on one track.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bookmark {
    /// Opaque catalog track id.
    pub track: String,
    /// Saved playback offset in milliseconds.
    pub position_ms: u64,
    /// Optional user note attached to the bookmark.
    pub comment: Option<String>,
    /// When the bookmark was first created.
    pub created_at: SystemTime,
    /// When the bookmark was last updated.
    pub updated_at: SystemTime,
}

/// Input for creating or updating a bookmark on one track.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SaveBookmark<'a> {
    /// Opaque catalog track id.
    pub track: &'a str,
    /// Playback offset in milliseconds.
    pub position_ms: u64,
    /// Optional user note.
    pub comment: Option<&'a str>,
}

/// Read and update per-user listening activity.
///
/// Calls may block on storage; asynchronous callers should use
/// [`Ctx::blocking`](crate::Ctx::blocking). Every paged method and every batch
/// id slice enforces [`MAX_PAGE_LIMIT`](crate::MAX_PAGE_LIMIT), refusing larger
/// requests with `kernel.page_too_large`. When [`Ctx::principal`](crate::Ctx::principal)
/// is present, per-user operations refuse calls targeting a different user.
pub trait Activity: Port {
    /// Star one or more catalog items for `user`. Idempotent: already-starred
    /// items keep their original timestamp.
    ///
    /// # Errors
    /// Returns a failure if `ids` exceeds `MAX_PAGE_LIMIT`, contains an invalid
    /// id, an item is absent from a registered catalog, the per-user cap is
    /// exceeded, the principal does not match `user`, or storage fails.
    fn star(&self, ctx: &Ctx, user: &UserName, kind: ItemKind, ids: &[&str]) -> Fallible;

    /// Remove stars on one or more catalog items for `user`. Idempotent.
    ///
    /// # Errors
    /// Returns a failure if `ids` exceeds `MAX_PAGE_LIMIT`, contains an invalid
    /// id, the principal does not match `user`, or storage fails.
    fn unstar(&self, ctx: &Ctx, user: &UserName, kind: ItemKind, ids: &[&str]) -> Fallible;

    /// Items of `kind` starred by `user`, most recently starred first, then id.
    ///
    /// # Errors
    /// Returns a failure if the page cannot be read, `limit` exceeds the cap,
    /// or the principal does not match `user`.
    fn starred(
        &self,
        ctx: &Ctx,
        user: &UserName,
        kind: ItemKind,
        page: PageRequest<'_>,
    ) -> Fallible<Page<StarredItem>>;

    /// Batch lookup of stars held by `user` among `ids`, in ascending id order,
    /// omitting unstarred or vanished catalog ids.
    ///
    /// # Errors
    /// Returns a failure if `ids` exceeds `MAX_PAGE_LIMIT`, contains an invalid
    /// id, the principal does not match `user`, or storage fails.
    fn starred_by_ids(
        &self,
        ctx: &Ctx,
        user: &UserName,
        kind: ItemKind,
        ids: &[&str],
    ) -> Fallible<Vec<StarredItem>>;

    /// Set `user`'s rating for one item (`1..=5`) or clear it (`0`).
    ///
    /// # Errors
    /// Returns a failure if `rating > 5`, `id` is invalid, the item is absent
    /// from a registered catalog (when `rating > 0`), the per-user cap is
    /// exceeded, the principal does not match `user`, or storage fails.
    fn set_rating(
        &self,
        ctx: &Ctx,
        user: &UserName,
        kind: ItemKind,
        id: &str,
        rating: u8,
    ) -> Fallible;

    /// Batch lookup of user and average ratings for `ids`, in ascending id
    /// order, omitting unrated or vanished catalog items.
    ///
    /// # Errors
    /// Returns a failure if `ids` exceeds `MAX_PAGE_LIMIT`, contains an invalid
    /// id, the principal does not match `user`, or storage fails.
    fn ratings(
        &self,
        ctx: &Ctx,
        user: &UserName,
        kind: ItemKind,
        ids: &[&str],
    ) -> Fallible<Vec<RatedItem>>;

    /// Record either a completed play submission or a now-playing update.
    ///
    /// Accepted submissions publish [`Played`](crate::signals::Played) after
    /// the write commits and clear any matching now-playing entry for the same
    /// user, client and track.
    ///
    /// # Errors
    /// Returns a failure if the input or timestamp is invalid, the track is
    /// absent from a registered catalog, the principal does not match `user`,
    /// or storage fails.
    fn scrobble(&self, ctx: &Ctx, user: &UserName, entry: &Scrobble<'_>) -> Fallible;

    /// Active now-playing entries, optionally filtered to one `user`, newest
    /// first, then user and client.
    ///
    /// # Errors
    /// Returns a failure if the page cannot be read, `limit` exceeds the cap,
    /// or a present principal asks for a different specific `user`.
    fn now_playing(
        &self,
        ctx: &Ctx,
        user: Option<&UserName>,
        page: PageRequest<'_>,
    ) -> Fallible<Page<NowPlayingEntry>>;

    /// Batch lookup of `user`'s play counts and recency for `ids`, in
    /// ascending track id order, omitting unplayed or vanished catalog tracks.
    ///
    /// # Errors
    /// Returns a failure if `ids` exceeds `MAX_PAGE_LIMIT`, contains an invalid
    /// id, the principal does not match `user`, or storage fails.
    fn track_stats(&self, ctx: &Ctx, user: &UserName, ids: &[&str]) -> Fallible<Vec<TrackStats>>;

    /// Batch lookup of `user`'s album play counts and recency for `ids`, in
    /// ascending album id order, omitting unplayed or vanished catalog albums.
    ///
    /// # Errors
    /// Returns a failure if `ids` exceeds `MAX_PAGE_LIMIT`, contains an invalid
    /// id, the principal does not match `user`, or storage fails.
    fn album_stats(&self, ctx: &Ctx, user: &UserName, ids: &[&str]) -> Fallible<Vec<AlbumStats>>;

    /// Most played tracks for one user (`Some(user)`) or across all users
    /// (`None`), ordered by play count descending, then track id (per-user
    /// breaks ties by `last_played` descending; global aggregates omit
    /// `last_played`).
    ///
    /// # Errors
    /// Returns a failure if the page cannot be read, `limit` exceeds the cap,
    /// or a present principal asks for a different specific `user`.
    fn top_tracks(
        &self,
        ctx: &Ctx,
        user: Option<&UserName>,
        page: PageRequest<'_>,
    ) -> Fallible<Page<TrackStats>>;

    /// Activity-ordered album ids for `list`.
    ///
    /// # Errors
    /// Returns a failure if the page cannot be read, `limit` exceeds the cap,
    /// or the principal does not match `user`.
    fn albums(
        &self,
        ctx: &Ctx,
        user: &UserName,
        list: AlbumActivityList,
        page: PageRequest<'_>,
    ) -> Fallible<Page<String>>;

    /// The user's saved play queue, or `None` when none is saved.
    ///
    /// # Errors
    /// Returns a failure if the principal does not match `user` or storage
    /// cannot be read.
    fn play_queue(&self, ctx: &Ctx, user: &UserName) -> Fallible<Option<PlayQueue>>;

    /// Save a play queue for `user`, or clear it when `tracks` is empty.
    ///
    /// # Errors
    /// Returns a failure if the input is invalid, exceeds configured bounds,
    /// the principal does not match `user`, or storage fails.
    fn save_play_queue(&self, ctx: &Ctx, user: &UserName, queue: &SavePlayQueue<'_>) -> Fallible;

    /// Bookmarks belonging to `user`, most recently updated first, then track.
    ///
    /// # Errors
    /// Returns a failure if the page cannot be read, `limit` exceeds the cap,
    /// or the principal does not match `user`.
    fn bookmarks(
        &self,
        ctx: &Ctx,
        user: &UserName,
        page: PageRequest<'_>,
    ) -> Fallible<Page<Bookmark>>;

    /// Create or update `user`'s bookmark on a track.
    ///
    /// # Errors
    /// Returns a failure if the input is invalid, exceeds configured bounds,
    /// the principal does not match `user`, or storage fails.
    fn save_bookmark(&self, ctx: &Ctx, user: &UserName, bookmark: &SaveBookmark<'_>) -> Fallible;

    /// Delete `user`'s bookmark on `track`.
    ///
    /// # Errors
    /// Returns a failure if `track` is invalid, the bookmark does not exist,
    /// the principal does not match `user`, or storage fails.
    fn delete_bookmark(&self, ctx: &Ctx, user: &UserName, track: &str) -> Fallible;
}
