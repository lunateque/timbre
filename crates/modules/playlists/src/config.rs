//! Configuration for the `[playlists]` section.

use crate::failures::PlaylistsFailure;
use std::sync::Arc;
use timbre_edge_config::{Config, serde};
use timbre_edge_sqlite::SharedDatabase;
use timbre_kernel::{Ctx, Fallible};

pub(crate) const MAX_PLAYLISTS_CAP: usize = 10_000;
pub(crate) const DEFAULT_PLAYLISTS_CAP: usize = 1_000;

pub(crate) const MAX_TRACKS_CAP: usize = 100_000;
pub(crate) const DEFAULT_TRACKS_CAP: usize = 10_000;

pub(crate) const MAX_TOTAL_ENTRIES_CAP: usize = 500_000;
pub(crate) const DEFAULT_TOTAL_ENTRIES_CAP: usize = 50_000;

#[expect(
    clippy::struct_field_names,
    reason = "TOML keys in [playlists] intentionally share the max_ prefix"
)]
#[derive(Clone, Copy, Debug, serde::Deserialize)]
#[serde(crate = "timbre_edge_config::serde", default, deny_unknown_fields)]
pub(crate) struct Settings {
    pub(crate) max_playlists_per_user: usize,
    pub(crate) max_tracks_per_playlist: usize,
    pub(crate) max_total_entries_per_user: usize,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            max_playlists_per_user: DEFAULT_PLAYLISTS_CAP,
            max_tracks_per_playlist: DEFAULT_TRACKS_CAP,
            max_total_entries_per_user: DEFAULT_TOTAL_ENTRIES_CAP,
        }
    }
}

impl Settings {
    pub(crate) fn load(ctx: &Ctx) -> Fallible<Self> {
        let Some(config) = ctx.resource::<Config>() else {
            return Ok(Self::default());
        };
        let settings: Self = config.section("playlists")?;
        if !(1..=MAX_PLAYLISTS_CAP).contains(&settings.max_playlists_per_user) {
            return Err(PlaylistsFailure::InvalidConfig(
                "[playlists].max_playlists_per_user must be between 1 and 10000.",
            )
            .boxed());
        }
        if !(1..=MAX_TRACKS_CAP).contains(&settings.max_tracks_per_playlist) {
            return Err(PlaylistsFailure::InvalidConfig(
                "[playlists].max_tracks_per_playlist must be between 1 and 100000.",
            )
            .boxed());
        }
        if !(1..=MAX_TOTAL_ENTRIES_CAP).contains(&settings.max_total_entries_per_user) {
            return Err(PlaylistsFailure::InvalidConfig(
                "[playlists].max_total_entries_per_user must be between 1 and 500000.",
            )
            .boxed());
        }
        Ok(settings)
    }
}

pub(crate) fn shared_db(ctx: &Ctx) -> Fallible<Arc<SharedDatabase>> {
    SharedDatabase::require(ctx, PlaylistsFailure::NotConfigured)
}
