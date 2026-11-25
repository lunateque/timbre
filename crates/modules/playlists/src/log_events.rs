//! Structured log events emitted by the `playlists` module.

use timbre_kernel::LogEvent;

/// A new playlist was created.
pub const CREATED: LogEvent = LogEvent::new("playlists.created");
/// An existing playlist's metadata or entries changed.
pub const UPDATED: LogEvent = LogEvent::new("playlists.updated");
/// A playlist was deleted by its owner.
pub const DELETED: LogEvent = LogEvent::new("playlists.deleted");
/// All playlists belonging to a removed or recreated user name were purged.
pub const USER_CLEARED: LogEvent = LogEvent::new("playlists.user_cleared");
/// A persisted playlist row had an invalid owner name and was skipped.
pub const STORED_OWNER_INVALID: LogEvent = LogEvent::new("playlists.stored_owner_invalid");
