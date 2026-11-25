//! User-owned playlists of ordered catalog track ids.

#![warn(missing_docs)]

mod config;
mod edits;
pub mod failures;
pub mod log_events;
mod service;
mod store;
mod validate;

use crate::config::{Settings, shared_db};
use crate::failures::PlaylistsFailure;
use std::sync::{Arc, OnceLock};
use timbre_kernel::identity::UserName;
use timbre_kernel::playlists::Playlists;
use timbre_kernel::signals::{UserAdded, UserRemoved};
use timbre_kernel::{Ctx, Fallible, Port, Registry, log};

#[derive(Default)]
struct PlaylistsModule {
    settings: OnceLock<Settings>,
}

impl Port for PlaylistsModule {}

impl PlaylistsModule {
    fn settings(&self) -> Fallible<Settings> {
        self.settings
            .get()
            .copied()
            .ok_or_else(|| PlaylistsFailure::NotConfigured.boxed())
    }
}

trait PlaylistsState: Port {
    fn module(&self) -> &PlaylistsModule;
}

impl PlaylistsState for PlaylistsModule {
    fn module(&self) -> &PlaylistsModule {
        self
    }
}

fn state(ctx: &Ctx) -> Fallible<Arc<dyn PlaylistsState>> {
    ctx.ports()
        .get::<dyn PlaylistsState>()
        .ok_or_else(|| PlaylistsFailure::NotConfigured.boxed())
}

fn configure(ctx: &Ctx) -> Fallible {
    let holder = state(ctx)?;
    let _ = holder.module().settings.set(Settings::load(ctx)?);
    Ok(())
}

/// Purge all playlists owned by `user`. Runs on both `UserRemoved` and
/// `UserAdded` so a newly created account can never inherit orphaned playlists
/// left by an earlier failed cleanup or racing write.
fn purge_user(ctx: &Ctx, user: &UserName, reason: &'static str) -> Fallible {
    let removed = shared_db(ctx)?
        .get(ctx)?
        .write(|sql| store::delete_user(sql, user))?;
    log!(
        ctx,
        info,
        log_events::USER_CLEARED,
        user = user.as_str(),
        reason = reason,
        playlists = removed
    );
    Ok(())
}

fn user_removed(ctx: &Ctx, signal: &UserRemoved) -> Fallible {
    purge_user(ctx, &signal.user, "removed")
}

fn user_added(ctx: &Ctx, signal: &UserAdded) -> Fallible {
    purge_user(ctx, &signal.user, "added")
}

/// Register the `Playlists` port, its SQLite schema, config validation and
/// user lifecycle cleanup handlers.
///
/// # Errors
///
/// Returns a registration failure when a declaration conflicts with one
/// another module already made.
pub fn register(registry: &mut Registry) -> Fallible {
    let module = Arc::new(PlaylistsModule::default());
    registry.config_check(configure);
    registry.provide::<dyn Playlists>(module.clone())?;
    registry.provide::<dyn PlaylistsState>(module)?;
    registry.migration("playlists", 1, include_str!("schema.sql"))?;
    registry.on::<UserRemoved>(user_removed);
    registry.on::<UserAdded>(user_added);
    Ok(())
}
