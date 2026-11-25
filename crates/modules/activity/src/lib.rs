//! Per-user listening activity: stars, ratings, plays, now playing, play
//! queues and bookmarks.

#![warn(missing_docs)]

mod config;
pub mod failures;
mod filter;
pub mod log_events;
mod now_playing;
mod playback;
mod service;
mod store;
mod validate;

use crate::config::{Settings, shared_db};
use crate::failures::ActivityFailure;
use crate::now_playing::NowPlayingTable;
use std::sync::{Arc, OnceLock};
use timbre_kernel::activity::Activity;
use timbre_kernel::identity::UserName;
use timbre_kernel::signals::{UserAdded, UserRemoved};
use timbre_kernel::{Ctx, Fallible, Port, Registry, log};

#[derive(Default)]
struct ActivityModule {
    now_playing: NowPlayingTable,
    settings: OnceLock<Settings>,
}

impl Port for ActivityModule {}

impl ActivityModule {
    fn settings(&self) -> Fallible<Settings> {
        self.settings
            .get()
            .copied()
            .ok_or_else(|| ActivityFailure::NotConfigured.boxed())
    }
}

trait ActivityState: Port {
    fn module(&self) -> &ActivityModule;
}

impl ActivityState for ActivityModule {
    fn module(&self) -> &ActivityModule {
        self
    }
}

fn state(ctx: &Ctx) -> Fallible<Arc<dyn ActivityState>> {
    ctx.ports()
        .get::<dyn ActivityState>()
        .ok_or_else(|| ActivityFailure::NotConfigured.boxed())
}

fn configure(ctx: &Ctx) -> Fallible {
    let holder = state(ctx)?;
    let _ = holder.module().settings.set(Settings::load(ctx)?);
    Ok(())
}

/// Purge all stored and in-memory activity for `user`. Runs on both
/// `UserRemoved` and `UserAdded` so a newly created account can never inherit
/// orphaned rows left by an earlier failed cleanup or racing write.
fn purge_user(ctx: &Ctx, user: &UserName, reason: &'static str) -> Fallible {
    let dropped_now_playing = state(ctx)?.module().now_playing.remove_user(user);
    let removed_rows = shared_db(ctx)?
        .get(ctx)?
        .write(|sql| store::delete_user(sql, user))?;
    log!(
        ctx,
        info,
        log_events::USER_CLEARED,
        user = user.as_str(),
        reason = reason,
        rows = removed_rows,
        now_playing = dropped_now_playing
    );
    Ok(())
}

fn user_removed(ctx: &Ctx, signal: &UserRemoved) -> Fallible {
    purge_user(ctx, &signal.user, "removed")
}

fn user_added(ctx: &Ctx, signal: &UserAdded) -> Fallible {
    purge_user(ctx, &signal.user, "added")
}

/// Register the `Activity` port, its SQLite schema, config validation and
/// user lifecycle cleanup handlers.
///
/// # Errors
///
/// Returns a registration failure when a declaration conflicts with one
/// another module already made.
pub fn register(registry: &mut Registry) -> Fallible {
    let module = Arc::new(ActivityModule::default());
    registry.config_check(configure);
    registry.provide::<dyn Activity>(module.clone())?;
    registry.provide::<dyn ActivityState>(module)?;
    registry.migration("activity", 1, include_str!("schema.sql"))?;
    registry.on::<UserRemoved>(user_removed);
    registry.on::<UserAdded>(user_added);
    Ok(())
}
