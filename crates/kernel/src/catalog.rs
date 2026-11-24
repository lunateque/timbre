//! Protocol-independent access to the scanned, tag-based music catalog.

mod metadata;
mod song;

pub use metadata::*;
pub use song::*;

use crate::page::{Page, PageRequest};
use crate::{Ctx, Fallible, Port};
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

/// Ordering or filter for album browsing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AlbumOrder<'a> {
    /// Most recently added first.
    Newest,
    /// By album order name.
    Name,
    /// By album artist order name, then album order name.
    Artist,
    /// Inclusive year range; reversed bounds request descending year order.
    Year {
        /// First year in the requested order.
        from: u32,
        /// Last year in the requested order.
        to: u32,
    },
    /// Albums with this genre, by album order name.
    Genre(&'a str),
    /// A shuffled order held stable through this page chain.
    Random,
}

/// Independent window in one category of a search result.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SearchWindow {
    /// Maximum number of items to return, at most
    /// [`MAX_PAGE_LIMIT`](crate::MAX_PAGE_LIMIT).
    pub count: usize,
    /// Number of matching items to skip.
    pub offset: usize,
}

/// Sizes and offsets of the independent result pages.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SearchWindows {
    /// Artist window.
    pub artists: SearchWindow,
    /// Album window.
    pub albums: SearchWindow,
    /// Track window.
    pub tracks: SearchWindow,
}

/// One result page per search category.
#[derive(Clone, Debug, PartialEq)]
pub struct SearchResults {
    /// Matching artists.
    pub artists: Page<Artist>,
    /// Matching albums.
    pub albums: Page<Album>,
    /// Matching tracks.
    pub tracks: Page<Track>,
}

/// A genre and its counts of distinct albums and tracks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Genre {
    /// Display name.
    pub name: String,
    /// Number of albums.
    pub album_count: u64,
    /// Number of tracks.
    pub track_count: u64,
}

/// An artist and a page of their albums.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArtistDetail {
    /// Artist metadata.
    pub artist: Artist,
    /// Albums credited to the artist.
    pub albums: Page<Album>,
}

/// An album and a page of its tracks.
#[derive(Clone, Debug, PartialEq)]
pub struct AlbumDetail {
    /// Album metadata.
    pub album: Album,
    /// Tracks in disc and track number order.
    pub tracks: Page<Track>,
}

/// The audio source selected for streaming a track.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MediaSource {
    /// Canonical path recorded by the scanner.
    pub path: PathBuf,
    /// File container identifier.
    pub container: String,
    /// Audio codec identifier.
    pub codec: String,
    /// Audio bit rate, in kilobits per second, when known.
    pub bitrate_kbps: Option<u32>,
    /// Playing time.
    pub duration: Duration,
    /// File size in bytes.
    pub size: u64,
}

/// Progress of the current scan, or its last result when idle.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScanStatus {
    /// Whether a scan is active.
    pub scanning: bool,
    /// Number of current catalog tracks.
    pub tracks: u64,
    /// Number of folders inspected in this or the last scan.
    pub folders: u64,
    /// Number of files the tag reader opened in this or the last scan.
    pub tag_reads: u64,
    /// Files that could not be read in this or the last scan.
    pub unreadable: u64,
    /// Time when the last scan ended, when known.
    pub finished: Option<SystemTime>,
}

/// Read the music catalog and control rescanning. Calls may block on storage;
/// asynchronous callers should use [`Ctx::blocking`]. Every method serves at
/// most [`MAX_PAGE_LIMIT`](crate::MAX_PAGE_LIMIT) items: a larger `limit`, or
/// a larger search `count`, is refused with `kernel.page_too_large`.
/// Concurrent inserts or deletes can shift later offset pages.
pub trait Catalog: Port {
    /// Artists by order name, then id.
    ///
    /// # Errors
    /// Returns a failure if the page cannot be read.
    fn artists(&self, ctx: &Ctx, page: PageRequest<'_>) -> Fallible<Page<Artist>>;

    /// An artist by id, with albums by album order name, then id.
    ///
    /// # Errors
    /// Returns a failure if the artist is absent or the page cannot be read.
    fn artist(&self, ctx: &Ctx, id: &str, albums: PageRequest<'_>) -> Fallible<ArtistDetail>;

    /// An album by id, with tracks by disc number, track number, then id.
    ///
    /// # Errors
    /// Returns a failure if the album is absent or the page cannot be read.
    fn album(&self, ctx: &Ctx, id: &str, tracks: PageRequest<'_>) -> Fallible<AlbumDetail>;

    /// One track by id.
    ///
    /// # Errors
    /// Returns a failure if the track is absent or cannot be read.
    fn track(&self, ctx: &Ctx, id: &str) -> Fallible<Track>;

    /// Tracks in input order, including repeated ids, omitting missing ids.
    ///
    /// # Errors
    /// Returns a failure if the page cannot be read.
    fn tracks(&self, ctx: &Ctx, ids: &[&str], page: PageRequest<'_>) -> Fallible<Page<Track>>;

    /// Albums in the chosen [`AlbumOrder`], with ties by id.
    ///
    /// # Errors
    /// Returns a failure if the page cannot be read.
    fn albums(
        &self,
        ctx: &Ctx,
        order: AlbumOrder<'_>,
        page: PageRequest<'_>,
    ) -> Fallible<Page<Album>>;

    /// Genres by folded name, then display name.
    ///
    /// # Errors
    /// Returns a failure if the page cannot be read.
    fn genres(&self, ctx: &Ctx, page: PageRequest<'_>) -> Fallible<Page<Genre>>;

    /// Search names and display credits. Each category is ordered by order
    /// name, then id, including when the query is empty. This is the one
    /// method that pages by count and offset instead of a cursor: a client
    /// asks for three independent windows in one call.
    ///
    /// # Errors
    /// Returns a failure if the result pages cannot be read.
    fn search(&self, ctx: &Ctx, query: &str, windows: SearchWindows) -> Fallible<SearchResults>;

    /// Lyrics in scan order: embedded entries first, or, when absent,
    /// sidecars ordered by language and filename.
    ///
    /// # Errors
    /// Returns a failure if the track is absent or the page cannot be read.
    fn lyrics(&self, ctx: &Ctx, id: &str, page: PageRequest<'_>) -> Fallible<Page<Lyrics>>;

    /// The scanned filesystem source for this track id.
    ///
    /// # Errors
    /// Returns a failure if the track has no readable source.
    fn media_source(&self, ctx: &Ctx, track: &str) -> Fallible<MediaSource>;

    /// Current scan status.
    ///
    /// # Errors
    /// Returns a failure if the catalog cannot be read.
    fn scan_status(&self, ctx: &Ctx) -> Fallible<ScanStatus>;

    /// Start a background scan, bypassing folder hashes when `full` is true.
    ///
    /// # Errors
    /// Returns a failure if a scan is running or the caller lacks permission.
    fn start_scan(&self, ctx: &Ctx, full: bool) -> Fallible<ScanStatus>;
}
