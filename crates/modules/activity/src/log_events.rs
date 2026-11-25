//! Structured log events emitted by the `activity` module.

use timbre_kernel::LogEvent;

/// One or more catalog items were starred.
pub const STARRED: LogEvent = LogEvent::new("activity.starred");
/// One or more catalog items were unstarred.
pub const UNSTARRED: LogEvent = LogEvent::new("activity.unstarred");
/// A user set or cleared a rating.
pub const RATED: LogEvent = LogEvent::new("activity.rated");
/// A completed play submission was stored.
pub const PLAYED: LogEvent = LogEvent::new("activity.played");
/// A non-submission now-playing report was recorded.
pub const NOW_PLAYING: LogEvent = LogEvent::new("activity.now_playing");
/// A now-playing entry was evicted to stay within the per-user or global cap.
pub const NOW_PLAYING_EVICTED: LogEvent = LogEvent::new("activity.now_playing_evicted");
/// A user saved their play queue.
pub const QUEUE_SAVED: LogEvent = LogEvent::new("activity.queue_saved");
/// A user cleared their play queue.
pub const QUEUE_CLEARED: LogEvent = LogEvent::new("activity.queue_cleared");
/// A user created or updated a bookmark.
pub const BOOKMARK_SAVED: LogEvent = LogEvent::new("activity.bookmark_saved");
/// A user deleted a bookmark.
pub const BOOKMARK_DELETED: LogEvent = LogEvent::new("activity.bookmark_deleted");
/// All activity belonging to a removed or recreated user name was purged.
pub const USER_CLEARED: LogEvent = LogEvent::new("activity.user_cleared");
