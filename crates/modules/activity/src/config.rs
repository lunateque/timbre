//! Configuration for the `[activity]` section.

use crate::failures::ActivityFailure;
use std::sync::Arc;
use timbre_edge_config::{Config, serde};
use timbre_edge_sqlite::SharedDatabase;
use timbre_kernel::{Ctx, Fallible};

pub(crate) const MIN_TTL_SECS: u64 = 60;
pub(crate) const MAX_TTL_SECS: u64 = 86_400;
pub(crate) const DEFAULT_TTL_SECS: u64 = 1_800;

pub(crate) const MAX_NOW_PLAYING_CAP: usize = 100_000;
pub(crate) const DEFAULT_NOW_PLAYING_CAP: usize = 10_000;

pub(crate) const MAX_QUEUE_CAP: usize = 100_000;
pub(crate) const DEFAULT_QUEUE_CAP: usize = 10_000;

pub(crate) const MAX_BOOKMARKS_CAP: usize = 10_000;
pub(crate) const DEFAULT_BOOKMARKS_CAP: usize = 1_000;

pub(crate) const MAX_STARS_CAP: usize = 500_000;
pub(crate) const DEFAULT_STARS_CAP: usize = 50_000;

pub(crate) const MAX_RATINGS_CAP: usize = 500_000;
pub(crate) const DEFAULT_RATINGS_CAP: usize = 50_000;

#[derive(Clone, Copy, Debug, serde::Deserialize)]
#[serde(crate = "timbre_edge_config::serde", default, deny_unknown_fields)]
pub(crate) struct Settings {
    pub(crate) now_playing_ttl_secs: u64,
    pub(crate) now_playing_max_entries: usize,
    pub(crate) max_queue_tracks: usize,
    pub(crate) max_bookmarks_per_user: usize,
    pub(crate) max_stars_per_user: usize,
    pub(crate) max_ratings_per_user: usize,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            now_playing_ttl_secs: DEFAULT_TTL_SECS,
            now_playing_max_entries: DEFAULT_NOW_PLAYING_CAP,
            max_queue_tracks: DEFAULT_QUEUE_CAP,
            max_bookmarks_per_user: DEFAULT_BOOKMARKS_CAP,
            max_stars_per_user: DEFAULT_STARS_CAP,
            max_ratings_per_user: DEFAULT_RATINGS_CAP,
        }
    }
}

impl Settings {
    pub(crate) fn load(ctx: &Ctx) -> Fallible<Self> {
        let Some(config) = ctx.resource::<Config>() else {
            return Ok(Self::default());
        };
        let settings: Self = config.section("activity")?;
        if !(MIN_TTL_SECS..=MAX_TTL_SECS).contains(&settings.now_playing_ttl_secs) {
            return Err(ActivityFailure::InvalidConfig(
                "[activity].now_playing_ttl_secs must be between 60 and 86400 seconds.",
            )
            .boxed());
        }
        if !(1..=MAX_NOW_PLAYING_CAP).contains(&settings.now_playing_max_entries) {
            return Err(ActivityFailure::InvalidConfig(
                "[activity].now_playing_max_entries must be between 1 and 100000.",
            )
            .boxed());
        }
        if !(1..=MAX_QUEUE_CAP).contains(&settings.max_queue_tracks) {
            return Err(ActivityFailure::InvalidConfig(
                "[activity].max_queue_tracks must be between 1 and 100000.",
            )
            .boxed());
        }
        if !(1..=MAX_BOOKMARKS_CAP).contains(&settings.max_bookmarks_per_user) {
            return Err(ActivityFailure::InvalidConfig(
                "[activity].max_bookmarks_per_user must be between 1 and 10000.",
            )
            .boxed());
        }
        if !(1..=MAX_STARS_CAP).contains(&settings.max_stars_per_user) {
            return Err(ActivityFailure::InvalidConfig(
                "[activity].max_stars_per_user must be between 1 and 500000.",
            )
            .boxed());
        }
        if !(1..=MAX_RATINGS_CAP).contains(&settings.max_ratings_per_user) {
            return Err(ActivityFailure::InvalidConfig(
                "[activity].max_ratings_per_user must be between 1 and 500000.",
            )
            .boxed());
        }
        Ok(settings)
    }
}

pub(crate) fn shared_db(ctx: &Ctx) -> Fallible<Arc<SharedDatabase>> {
    SharedDatabase::require(ctx, ActivityFailure::NotConfigured)
}
