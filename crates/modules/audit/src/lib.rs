//! The security journal: who was let in, who was refused, and every change
//! to who may do what.
//!
//! The module knows nothing about the module that owns users: it listens to
//! the kernel's signals, so a server can run the journal without `auth`, or
//! `auth` without the journal.

#![warn(missing_docs)]

mod commands;
mod config;
pub mod failures;
mod jobs;
pub mod log_events;
mod queue;
mod store;
#[cfg(test)]
mod tests;

use crate::config::Settings;
use crate::queue::{Entry, Queue};
use std::sync::{Arc, OnceLock};
use timbre_kernel::clock::now_unix_millis;
use timbre_kernel::signals::{
    AccessDenied, AuthFailed, DeviceIssued, DeviceRevoked, PermissionsChanged, ServerDrained,
    UserAdded, UserRemoved,
};
use timbre_kernel::{Ctx, Fallible, Port, Registry};

#[derive(Default)]
struct Audit {
    queue: Queue,
    settings: OnceLock<Settings>,
}

impl Port for Audit {}

/// This module's own state; the signal handlers are function pointers and
/// reach it the way every other module reaches its state.
trait AuditState: Port {
    fn queue(&self) -> &Queue;
    fn settings(&self) -> &OnceLock<Settings>;
}

impl AuditState for Audit {
    fn queue(&self) -> &Queue {
        &self.queue
    }

    fn settings(&self) -> &OnceLock<Settings> {
        &self.settings
    }
}

fn state(ctx: &Ctx) -> Fallible<Arc<dyn AuditState>> {
    ctx.ports()
        .get::<dyn AuditState>()
        .ok_or_else(|| failures::AuditFailure::NotConfigured.boxed())
}

fn settings(state: &dyn AuditState) -> Fallible<Settings> {
    state
        .settings()
        .get()
        .copied()
        .ok_or_else(|| failures::AuditFailure::NotConfigured.boxed())
}

fn configure(ctx: &Ctx) -> Fallible {
    let audit = state(ctx)?;
    let _ = audit.settings().set(Settings::load(ctx)?);
    Ok(())
}

/// Encode the fields of one entry as a JSON object.
fn detail(fields: &[(&str, String)]) -> Fallible<String> {
    // A map, so the stored detail is a JSON object whatever order a caller
    // lists its fields in.
    let fields: std::collections::BTreeMap<&str, String> = fields.iter().cloned().collect();
    timbre_edge_sqlite::json::encode(&fields)
}

/// Standalone commands drain inline because they have no background jobs.
fn note(ctx: &Ctx, entry: Entry) -> Fallible {
    let audit = state(ctx)?;
    audit
        .queue()
        .push(entry, settings(audit.as_ref())?.queue_capacity);
    if ctx.is_cli() {
        jobs::drain(ctx)?;
    }
    Ok(())
}

fn auth_failed(ctx: &Ctx, signal: &AuthFailed) -> Fallible {
    let mut fields = vec![("reason", signal.reason.as_str().to_owned())];
    if let Some(label) = signal.detail {
        fields.push(("detail", label.as_str().to_owned()));
    }
    note(
        ctx,
        Entry {
            time: now_unix_millis(),
            kind: "auth_failed",
            user: signal.user.as_ref().map(|user| user.as_str().to_owned()),
            client: Some(signal.client.to_string()),
            detail: detail(&fields)?,
        },
    )
}

fn access_denied(ctx: &Ctx, signal: &AccessDenied) -> Fallible {
    note(
        ctx,
        Entry {
            time: now_unix_millis(),
            kind: "access_denied",
            user: Some(signal.user.as_str().to_owned()),
            client: Some(signal.client.to_string()),
            detail: detail(&[("permission", signal.permission.as_str().to_owned())])?,
        },
    )
}

fn user_added(ctx: &Ctx, signal: &UserAdded) -> Fallible {
    note(
        ctx,
        Entry {
            time: now_unix_millis(),
            kind: "user_added",
            user: Some(signal.user.as_str().to_owned()),
            client: None,
            detail: detail(&[])?,
        },
    )
}

fn user_removed(ctx: &Ctx, signal: &UserRemoved) -> Fallible {
    note(
        ctx,
        Entry {
            time: now_unix_millis(),
            kind: "user_removed",
            user: Some(signal.user.as_str().to_owned()),
            client: None,
            detail: detail(&[])?,
        },
    )
}

fn permissions_changed(ctx: &Ctx, signal: &PermissionsChanged) -> Fallible {
    let list = |permissions: &[timbre_kernel::Permission]| {
        permissions
            .iter()
            .map(|permission| permission.as_str())
            .collect::<Vec<_>>()
            .join(",")
    };
    note(
        ctx,
        Entry {
            time: now_unix_millis(),
            kind: "permissions_changed",
            user: Some(signal.user.as_str().to_owned()),
            client: None,
            detail: detail(&[
                ("granted", list(&signal.granted)),
                ("revoked", signal.revoked.join(",")),
                ("admin", signal.admin.to_string()),
            ])?,
        },
    )
}

/// The secret in the signal is never read: the journal records that a
/// device was issued, not what was issued.
fn device_issued(ctx: &Ctx, signal: &DeviceIssued) -> Fallible {
    note(
        ctx,
        Entry {
            time: now_unix_millis(),
            kind: "device_issued",
            user: Some(signal.user.as_str().to_owned()),
            client: None,
            detail: detail(&[
                ("device", signal.device.clone()),
                ("label", signal.label.clone()),
            ])?,
        },
    )
}

fn device_revoked(ctx: &Ctx, signal: &DeviceRevoked) -> Fallible {
    note(
        ctx,
        Entry {
            time: now_unix_millis(),
            kind: "device_revoked",
            user: Some(signal.user.as_str().to_owned()),
            client: None,
            detail: detail(&[("device", signal.device.clone())])?,
        },
    )
}

/// Register the journal: its table, the signals it records, the jobs that
/// write and prune it, and the command that reads it.
///
/// # Errors
///
/// Returns a registration failure when a declaration conflicts with one
/// another module already made.
pub fn register(registry: &mut Registry) -> Fallible {
    registry.provide::<dyn AuditState>(Arc::new(Audit::default()))?;
    registry.config_check(configure);
    registry.migration("audit", 1, include_str!("schema.sql"))?;
    registry.on::<AuthFailed>(auth_failed);
    registry.on::<AccessDenied>(access_denied);
    registry.on::<UserAdded>(user_added);
    registry.on::<UserRemoved>(user_removed);
    registry.on::<PermissionsChanged>(permissions_changed);
    registry.on::<DeviceIssued>(device_issued);
    registry.on::<DeviceRevoked>(device_revoked);
    registry.on::<ServerDrained>(|ctx, _| jobs::drain(ctx));
    registry.job(log_events::WRITTEN, jobs::draining)?;
    registry.job(log_events::PRUNED, jobs::pruning)?;
    registry.command(
        "audit",
        "Read the security journal: audit [--since <date>] [--user <name>] [--kind <kind>] [--limit <n>].",
        commands::audit,
    )
}
