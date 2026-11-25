//! Users, the permissions they hold, and the device secrets that speak for
//! them.
//!
//! Nothing here knows what the server serves: a user is a name, a device is
//! a label and a secret, and a permission is whatever some other module
//! declared. The module owns the `ApiKey` credential kind, answers the
//! [`Directory`] port, and publishes a signal for every change it makes, so
//! a journal or a cache never has to ask it anything.

#![warn(missing_docs)]

mod commands;
mod config;
mod devices;
mod directory;
pub mod failures;
mod keys;
mod lastuse;
pub mod log_events;
mod provider;
mod store;
#[cfg(test)]
mod tests;

pub use keys::ApiKey;

use std::sync::{Arc, OnceLock};
use timbre_kernel::credential::CredentialKind;
use timbre_kernel::directory::Directory;
use timbre_kernel::identity::{Permission, UserName};
use timbre_kernel::{Ctx, Fallible, Port, Registry};

/// Administer users, devices and permissions.
const ADMIN: Permission = Permission::new("auth.admin");

/// The module's one instance: pending device activity drained by the last-use job.
#[derive(Default)]
struct Auth {
    activity: lastuse::Pending,
    settings: OnceLock<config::Settings>,
    #[cfg(test)]
    work: provider::Work,
    bad_users: std::sync::Mutex<std::collections::HashSet<String>>,
}

impl Port for Auth {}

/// This module's own state. Jobs and commands are plain function pointers
/// that cannot capture it, so it travels through the port registry like
/// every other port; registering the trait rather than `Auth` keeps the
/// state type itself out of reach.
trait AuthState: Port {
    fn activity(&self) -> &lastuse::Pending;
    fn settings(&self) -> &OnceLock<config::Settings>;
    #[cfg(test)]
    fn work(&self) -> &provider::Work;
}

impl AuthState for Auth {
    fn activity(&self) -> &lastuse::Pending {
        &self.activity
    }

    fn settings(&self) -> &OnceLock<config::Settings> {
        &self.settings
    }

    #[cfg(test)]
    fn work(&self) -> &provider::Work {
        &self.work
    }
}

fn state(ctx: &Ctx) -> Fallible<Arc<dyn AuthState>> {
    ctx.ports()
        .get::<dyn AuthState>()
        .ok_or_else(|| failures::AuthFailure::NotConfigured.boxed())
}

fn configure(ctx: &Ctx) -> Fallible {
    let auth = state(ctx)?;
    let _ = auth.settings().set(config::Settings::load(ctx)?);
    Ok(())
}

fn default_expiry(ctx: &Ctx) -> Fallible<Option<u32>> {
    state(ctx)?
        .settings()
        .get()
        .map(|settings| settings.device_default_expiry_days)
        .ok_or_else(|| failures::AuthFailure::NotConfigured.boxed())
}

/// The permissions a stored account has in effect. An admin holds every
/// permission declared by the modules registered in this process, so a
/// module added later needs no migration of stored rows; a stored
/// permission no module declares is dropped, because nothing would ever
/// check it and showing it would claim access the user does not have.
fn effective(ctx: &Ctx, admin: bool, stored: &[String]) -> Vec<Permission> {
    let declared = ctx.kernel().permissions();
    if admin {
        return declared.iter().map(|entry| entry.permission).collect();
    }
    declared
        .iter()
        .map(|entry| entry.permission)
        .filter(|permission| stored.iter().any(|name| name == permission.as_str()))
        .collect()
}

/// The declared permission spelled `name`, or `auth.unknown_permission`.
fn declared(ctx: &Ctx, name: &str) -> Fallible<Permission> {
    ctx.kernel()
        .permissions()
        .iter()
        .map(|entry| entry.permission)
        .find(|permission| permission.as_str() == name)
        .ok_or_else(|| failures::AuthFailure::UnknownPermission(name.to_owned()).boxed())
}

/// Read a user name from a command argument.
fn user_name(ctx: &Ctx, value: &str) -> Fallible<UserName> {
    UserName::normalize(value).map_err(|_| {
        timbre_kernel::log!(ctx, warn, log_events::INVALID_ARGUMENT, user = value);
        Box::new(timbre_kernel::failures::KernelFailure::Usage(
            "Expected a usable user name.",
        )) as Box<dyn timbre_kernel::Failure>
    })
}

fn stored_user(value: &str) -> Fallible<UserName> {
    UserName::parse_canonical(value)
        .ok_or_else(|| failures::AuthFailure::StoredUserInvalid(value.to_owned()).boxed())
}

/// Register the directory, the `ApiKey` provider, the schema, the last-use
/// job and the user and device commands.
///
/// # Errors
///
/// Returns a registration failure when a declaration conflicts with one
/// another module already made.
pub fn register(registry: &mut Registry) -> Fallible {
    registry.config_check(configure);
    let auth = Arc::new(Auth::default());
    registry.provide::<dyn Directory>(auth.clone())?;
    registry.provide::<dyn AuthState>(auth.clone())?;
    registry.credential_provider(CredentialKind::ApiKey, auth)?;
    registry.migration("auth", 1, include_str!("schema.sql"))?;
    registry.permission(ADMIN, "Administer users, devices and permissions")?;
    registry.job(log_events::LAST_USE_WRITTEN, lastuse::job)?;
    commands::register(registry)
}
