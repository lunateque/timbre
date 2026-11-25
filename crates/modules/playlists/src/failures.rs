//! Failure codes and typed errors for the `playlists` module.

use timbre_kernel::{Failure, FailureCode};

/// A `[playlists]` configuration value is outside its supported bounds.
pub const INVALID_CONFIG: FailureCode = FailureCode::new("playlists.invalid_config");
/// A caller supplied an invalid title, comment, id or entry index.
pub const INVALID_INPUT: FailureCode = FailureCode::new("playlists.invalid_input");
/// The playlist does not exist or is private to another user.
pub const NOT_FOUND: FailureCode = FailureCode::new("playlists.not_found");
/// The caller is not allowed to act on this playlist.
pub const FORBIDDEN: FailureCode = FailureCode::new("playlists.forbidden");
/// Concurrent edits shifted the playlist during an index-based removal.
pub const CONFLICT: FailureCode = FailureCode::new("playlists.conflict");
/// The operation would exceed a configured per-user or per-playlist cap.
pub const LIMIT_REACHED: FailureCode = FailureCode::new("playlists.limit_reached");
/// A persisted playlist row carries an invalid owner name.
pub const STORED_OWNER_INVALID: FailureCode = FailureCode::new("playlists.stored_owner_invalid");
/// The host installed no shared database resource or skipped config validation.
pub const NOT_CONFIGURED: FailureCode = FailureCode::new("playlists.not_configured");

/// Errors produced by the `playlists` module.
#[derive(Debug)]
pub enum PlaylistsFailure {
    /// A setting under `[playlists]` failed range validation.
    InvalidConfig(&'static str),
    /// A caller supplied a malformed argument.
    InvalidInput(&'static str),
    /// The playlist does not exist or is private to another user.
    NotFound,
    /// The caller does not own the target playlist or mismatched the principal.
    Forbidden,
    /// Concurrent edits raced with an index-based removal.
    Conflict,
    /// A configured per-user or per-playlist cap was exceeded.
    LimitReached(&'static str),
    /// A stored owner column could not be parsed as a canonical user name.
    StoredOwnerInvalid(String),
    /// The shared database resource or startup configuration is absent.
    NotConfigured,
}

impl PlaylistsFailure {
    /// Box this failure for `Fallible` returns.
    pub(crate) fn boxed(self) -> Box<dyn Failure> {
        Box::new(self)
    }
}

impl Failure for PlaylistsFailure {
    fn code(&self) -> FailureCode {
        match self {
            Self::InvalidConfig(_) => INVALID_CONFIG,
            Self::InvalidInput(_) => INVALID_INPUT,
            Self::NotFound => NOT_FOUND,
            Self::Forbidden => FORBIDDEN,
            Self::Conflict => CONFLICT,
            Self::LimitReached(_) => LIMIT_REACHED,
            Self::StoredOwnerInvalid(_) => STORED_OWNER_INVALID,
            Self::NotConfigured => NOT_CONFIGURED,
        }
    }

    fn status(&self) -> u16 {
        match self {
            Self::InvalidConfig(_) | Self::StoredOwnerInvalid(_) => 500,
            Self::InvalidInput(_) => 400,
            Self::NotFound => 404,
            Self::Forbidden => 403,
            Self::Conflict => 409,
            Self::LimitReached(_) => 422,
            Self::NotConfigured => 503,
        }
    }

    fn public_message(&self) -> &str {
        match self {
            Self::InvalidConfig(reason)
            | Self::InvalidInput(reason)
            | Self::LimitReached(reason) => reason,
            Self::NotFound => "The requested playlist was not found.",
            Self::Forbidden => "Only the playlist owner may change or delete it.",
            Self::Conflict => "The playlist changed while it was being updated; try again.",
            Self::StoredOwnerInvalid(_) => "A stored playlist has an invalid owner.",
            Self::NotConfigured => "Playlist storage is not configured.",
        }
    }
}
