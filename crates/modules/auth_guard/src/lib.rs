//! Brute-force limiting: how many credential checks one client address or
//! one named user may fail before the server stops asking at all.
//!
//! Without this module registered the kernel has no throttle and every
//! attempt reaches its provider, which is exactly what a server behind
//! somebody else's limiter wants.
//!
//! The trade-off of the per-user window is deliberate and worth knowing: an
//! attacker who knows a name can fill that user's window from anywhere and
//! lock them out for the rest of it. `ApiKey` credentials name no user, so
//! modern clients are limited by address only; the per-user window exists
//! for the protocols that do send a name with every request.

#![warn(missing_docs)]

mod config;
pub mod failures;
pub mod log_events;
#[cfg(test)]
mod tests;
mod windows;

use crate::config::Settings;
use crate::log_events::{NOT_CONFIGURED, TABLE_FULL, WINDOW_FULL};
use crate::windows::{Key, Windows};
use std::net::IpAddr;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};
use timbre_kernel::credential::{RejectReason, Throttle};
use timbre_kernel::identity::UserName;
use timbre_kernel::{Ctx, Fallible, Port, Registry, log};

/// Settings are loaded by the startup check before serving requests.
#[derive(Default)]
struct State {
    windows: Windows,
    settings: Option<Settings>,
    overflow: Option<(Instant, u64)>,
}

#[derive(Default)]
struct Guard {
    state: Mutex<State>,
}

impl Port for Guard {}

impl Guard {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn reserve_windows(
        &self,
        ctx: &Ctx,
        client: IpAddr,
        user: Option<&UserName>,
    ) -> Option<[bool; 2]> {
        let (reservation, report) = {
            let mut state = self.lock();
            let Some(settings) = state.settings else {
                drop(state);
                ctx.log_security(&NOT_CONFIGURED, &[]);
                return None;
            };
            let now = Instant::now();
            let result = state.windows.reserve(keys(client, user), now, &settings);
            let (reservation, untracked) =
                result.map_or((None, 0), |(slots, skipped)| (Some(slots), skipped));
            let report = collect_overflow(&mut state.overflow, now, settings.window(), untracked);
            (reservation, report)
        };
        if let Some(count) = report {
            ctx.log_security(&TABLE_FULL, &[("count", &count)]);
        }
        reservation
    }
}

/// The module's own state, reachable by the signal handlers, which are
/// plain function pointers.
trait GuardState: Port {
    fn guard(&self) -> &Guard;
}

impl GuardState for Guard {
    fn guard(&self) -> &Guard {
        self
    }
}

fn collect_overflow(
    interval: &mut Option<(Instant, u64)>,
    now: Instant,
    window: Duration,
    untracked: u64,
) -> Option<u64> {
    let report =
        if interval.is_some_and(|(start, _)| now.saturating_duration_since(start) >= window) {
            interval.take().map(|(_, count)| count)
        } else {
            None
        };
    if untracked != 0 {
        let (_, count) = interval.get_or_insert((now, 0));
        *count = count.saturating_add(untracked);
    }
    report
}

fn keys(client: IpAddr, user: Option<&UserName>) -> [Option<Key>; 2] {
    [
        Some(Key::client(client)),
        user.map(|name| Key::user(name.as_str())),
    ]
}

impl Throttle for Guard {
    fn reserve(&self, ctx: &Ctx, client: IpAddr, user: Option<&UserName>) -> Option<[bool; 2]> {
        self.reserve_windows(ctx, client, user)
    }

    fn finish(
        &self,
        ctx: &Ctx,
        client: IpAddr,
        user: Option<&UserName>,
        reserved: [bool; 2],
        reason: Option<RejectReason>,
    ) {
        let failed = matches!(reason, Some(RejectReason::Invalid | RejectReason::Disabled));
        let notifications = {
            let mut state = self.lock();
            let Some(settings) = state.settings else {
                return;
            };
            let now = Instant::now();
            let [address, user] = keys(client, user);
            let [reserved_address, reserved_user] = reserved;
            [(address, reserved_address), (user, reserved_user)].map(|(key, reserved)| {
                let key = key.filter(|_| reserved)?;
                let scope = key.scope();
                state
                    .windows
                    .finish(key, now, &settings, failed)
                    .map(|limit| (scope, limit))
            })
        };
        for (scope, limit) in notifications.into_iter().flatten() {
            log!(ctx, warn, WINDOW_FULL, scope = scope, limit = limit);
        }
    }

    fn report(&self, ctx: &Ctx) {
        let report = {
            let mut state = self.lock();
            let Some(settings) = state.settings else {
                return;
            };
            collect_overflow(&mut state.overflow, Instant::now(), settings.window(), 0)
        };
        if let Some(count) = report {
            ctx.log_security(&TABLE_FULL, &[("count", &count)]);
        }
    }
}

fn state(ctx: &Ctx) -> Option<Arc<dyn GuardState>> {
    ctx.ports().get::<dyn GuardState>()
}

fn configure(ctx: &Ctx) -> Fallible {
    let state = state(ctx).ok_or_else(|| failures::GuardFailure::NotConfigured.boxed())?;
    let mut guard = state.guard().lock();
    if guard.settings.is_none() {
        guard.settings = Some(Settings::load(ctx)?);
    }
    Ok(())
}

/// Register the throttle.
///
/// # Errors
///
/// `kernel.throttle_taken` when another module already registered one.
pub fn register(registry: &mut Registry) -> Fallible {
    let guard = Arc::new(Guard::default());
    registry.provide::<dyn GuardState>(guard.clone())?;
    registry.throttle(guard)?;
    registry.config_check(configure);
    Ok(())
}
