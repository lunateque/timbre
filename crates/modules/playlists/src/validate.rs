//! Input validation, authorization checks and opaque id generation for `playlists`.

use crate::failures::PlaylistsFailure;
use timbre_edge_crypto::{base32, random_bytes};
use timbre_kernel::identity::UserName;
use timbre_kernel::text::{clean_comment, clean_label, is_valid_id};
use timbre_kernel::{Ctx, Fallible};

/// 128-bit random ids avoid both sequential enumeration and birthday collisions.
const ID_BYTES: usize = 16;
const MAX_ID_BYTES: usize = 256;
const MAX_NAME_CHARS: usize = 200;
const MAX_COMMENT_CHARS: usize = 1_000;

/// Draw a new opaque random playlist id.
pub(crate) fn new_playlist_id() -> Fallible<String> {
    let bytes: [u8; ID_BYTES] = random_bytes()?;
    Ok(base32::encode(&bytes))
}

/// Enforce that a guarded request's principal matches `actor`.
pub(crate) fn check_actor(ctx: &Ctx, actor: &UserName) -> Fallible {
    if let Some(principal) = ctx.principal()
        && principal.user() != actor
    {
        return Err(PlaylistsFailure::Forbidden.boxed());
    }
    Ok(())
}

/// Validate a playlist id supplied by a caller.
pub(crate) fn check_playlist_id(value: &str) -> Fallible<&str> {
    if !is_valid_id(value, MAX_ID_BYTES) {
        return Err(PlaylistsFailure::InvalidInput("Expected a valid playlist id.").boxed());
    }
    Ok(value)
}

/// Validate an opaque catalog track id.
pub(crate) fn check_track_id(value: &str) -> Fallible<&str> {
    if !is_valid_id(value, MAX_ID_BYTES) {
        return Err(PlaylistsFailure::InvalidInput("Expected a valid track id.").boxed());
    }
    Ok(value)
}

/// Validate and trim a playlist title.
pub(crate) fn check_name(value: &str) -> Fallible<&str> {
    clean_label(value, MAX_NAME_CHARS)
        .ok_or_else(|| PlaylistsFailure::InvalidInput("Playlist name is invalid.").boxed())
}

/// Validate an optional comment; empty or whitespace-only text collapses to `None`.
pub(crate) fn check_comment(value: Option<&str>) -> Fallible<Option<&str>> {
    clean_comment(value, MAX_COMMENT_CHARS)
        .map_err(|_| PlaylistsFailure::InvalidInput("Playlist comment is invalid.").boxed())
}

/// Parse a persisted owner column as a canonical user name.
pub(crate) fn stored_owner(value: &str) -> Fallible<UserName> {
    UserName::parse_canonical(value)
        .ok_or_else(|| PlaylistsFailure::StoredOwnerInvalid(value.to_owned()).boxed())
}
