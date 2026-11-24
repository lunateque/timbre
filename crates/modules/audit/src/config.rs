//! The `[audit]` section.

use crate::failures::AuditFailure;
use std::sync::Arc;
use std::time::Duration;
use timbre_edge_config::{Config, serde};
use timbre_edge_sqlite::SharedDatabase;
use timbre_kernel::{Ctx, Fallible};

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize)]
#[serde(crate = "timbre_edge_config::serde", default, deny_unknown_fields)]
pub(crate) struct Settings {
    /// Entries that may wait for the drain job before any are dropped. At
    /// least two, so the slots reserved for other kinds cannot swallow the
    /// whole queue and silently drop every failed login.
    pub(crate) queue_capacity: usize,
    /// How long journal rows are kept.
    pub(crate) keep_days: u32,
    /// How often the drain job writes what has queued up.
    pub(crate) drain_seconds: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            queue_capacity: 4096,
            keep_days: 365,
            drain_seconds: 5,
        }
    }
}

impl Settings {
    pub(crate) fn load(ctx: &Ctx) -> Fallible<Self> {
        let Some(config) = ctx.resource::<Config>() else {
            return Ok(Self::default());
        };
        let settings: Self = config.section("audit")?;
        settings.check()?;
        Ok(settings)
    }

    /// A capacity of one would be entirely reserved for kinds other than
    /// `auth_failed`, so every failed login would be dropped.
    fn check(self) -> Fallible {
        if self.queue_capacity < 2 || self.keep_days == 0 || self.drain_seconds == 0 {
            return Err(AuditFailure::InvalidConfig.boxed());
        }
        Ok(())
    }

    pub(crate) fn drain_every(self) -> Duration {
        Duration::from_secs(u64::from(self.drain_seconds))
    }
}

pub(crate) fn shared_db(ctx: &Ctx) -> Fallible<Arc<SharedDatabase>> {
    SharedDatabase::require(ctx, AuditFailure::NotConfigured)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_queue_too_small_for_both_kinds_is_refused() {
        let settings = Settings::default();
        settings.check().unwrap();
        for invalid in [
            Settings {
                queue_capacity: 0,
                ..settings
            },
            Settings {
                queue_capacity: 1,
                ..settings
            },
            Settings {
                keep_days: 0,
                ..settings
            },
            Settings {
                drain_seconds: 0,
                ..settings
            },
        ] {
            assert_eq!(
                invalid.check().unwrap_err().code().as_str(),
                "audit.invalid_config"
            );
        }
        Settings {
            queue_capacity: 2,
            ..settings
        }
        .check()
        .unwrap();
    }
}
