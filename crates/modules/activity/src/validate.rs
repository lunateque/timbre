//! Input validation, authorization checks and SQLite tag encoding for `activity`.

use crate::failures::ActivityFailure;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use timbre_kernel::activity::ItemKind;
use timbre_kernel::identity::UserName;
use timbre_kernel::text::{clean_comment, clean_label, is_valid_id};
use timbre_kernel::{Ctx, Fallible};

const MAX_ID_BYTES: usize = 256;
const MAX_CLIENT_CHARS: usize = 128;
const MAX_COMMENT_CHARS: usize = 1_000;
/// Maximum allowed clock skew for client-supplied scrobble timestamps.
const MAX_FUTURE_SKEW: Duration = Duration::from_mins(15);

/// Stable integer tag stored in `activity_stars` and `activity_ratings`.
pub(crate) const fn kind_code(kind: ItemKind) -> i64 {
    match kind {
        ItemKind::Track => 1,
        ItemKind::Album => 2,
        ItemKind::Artist => 3,
    }
}

/// Stable short label for structured logs.
pub(crate) const fn kind_name(kind: ItemKind) -> &'static str {
    match kind {
        ItemKind::Track => "track",
        ItemKind::Album => "album",
        ItemKind::Artist => "artist",
    }
}

/// Enforce that a guarded request's principal matches the target user.
pub(crate) fn check_actor(ctx: &Ctx, user: &UserName) -> Fallible {
    if let Some(principal) = ctx.principal()
        && principal.user() != user
    {
        return Err(ActivityFailure::Forbidden.boxed());
    }
    Ok(())
}

/// Validate an opaque catalog id before storing or querying it.
pub(crate) fn check_id(value: &str) -> Fallible<&str> {
    if !is_valid_id(value, MAX_ID_BYTES) {
        return Err(ActivityFailure::InvalidInput("Expected a valid catalog item id.").boxed());
    }
    Ok(value)
}

/// Validate a client or `changed_by` label; returns the trimmed slice.
pub(crate) fn check_client(value: &str) -> Fallible<&str> {
    clean_label(value, MAX_CLIENT_CHARS)
        .ok_or_else(|| ActivityFailure::InvalidInput("Expected a valid client label.").boxed())
}

/// Validate an optional bookmark comment.
pub(crate) fn check_comment(value: Option<&str>) -> Fallible<Option<&str>> {
    clean_comment(value, MAX_COMMENT_CHARS)
        .map_err(|_| ActivityFailure::InvalidInput("Bookmark comment is invalid.").boxed())
}

/// Validate that a millisecond playback offset fits in a signed 64-bit SQLite integer.
pub(crate) fn check_position_ms(value: u64) -> Fallible<i64> {
    i64::try_from(value)
        .map_err(|_| ActivityFailure::InvalidInput("Playback position is out of range.").boxed())
}

/// Validate a client-supplied play timestamp against Unix epoch and future skew.
pub(crate) fn check_scrobble_time(at: SystemTime, now: SystemTime) -> Fallible<SystemTime> {
    if at.duration_since(UNIX_EPOCH).is_err() {
        return Err(ActivityFailure::InvalidInput(
            "Play timestamp must not precede the Unix epoch.",
        )
        .boxed());
    }
    if let Ok(ahead) = at.duration_since(now)
        && ahead > MAX_FUTURE_SKEW
    {
        return Err(
            ActivityFailure::InvalidInput("Play timestamp is too far in the future.").boxed(),
        );
    }
    Ok(at)
}
