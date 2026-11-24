//! The `[auth_guard]` section.

use crate::failures::GuardFailure;
use std::time::Duration;
use timbre_edge_config::{Config, serde};
use timbre_kernel::{Ctx, Fallible};

/// The largest number of failure timestamps a configured table may hold,
/// about 320 MB at 16 bytes each: past this a limiter would be the thing
/// that exhausts the server it protects.
const MAX_TIMESTAMPS: u64 = 20_000_000;

/// The defaults: ten failures per address and twenty per named user in a
/// quarter of an hour, and at most a hundred thousand remembered windows,
/// which is a few megabytes and far more than a home server ever sees.
///
/// Every remembered window can hold one timestamp per failure it is allowed,
/// so the table's worst case is `max_tracked * (address_failures +
/// user_failures)` timestamps. That product is capped at [`MAX_TIMESTAMPS`],
/// about 320 MB of failure history, so a configuration cannot promise more
/// memory than a server has.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize)]
#[serde(crate = "timbre_edge_config::serde", default, deny_unknown_fields)]
pub(crate) struct Settings {
    pub(crate) address_failures: u32,
    pub(crate) user_failures: u32,
    pub(crate) window_minutes: u32,
    pub(crate) max_tracked: usize,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            address_failures: 10,
            user_failures: 20,
            window_minutes: 15,
            max_tracked: 100_000,
        }
    }
}

impl Settings {
    pub(crate) fn load(ctx: &Ctx) -> Fallible<Self> {
        let Some(config) = ctx.resource::<Config>() else {
            return Ok(Self::default());
        };
        let settings: Self = config.section("auth_guard")?;
        settings.check()?;
        Ok(settings)
    }

    fn check(self) -> Fallible {
        if !(1..=10_000).contains(&self.address_failures)
            || !(1..=10_000).contains(&self.user_failures)
        {
            return Err(GuardFailure::InvalidConfig(
                "address_failures and user_failures must be in 1..=10000",
            )
            .boxed());
        }
        if !(1..=1440).contains(&self.window_minutes) {
            return Err(GuardFailure::InvalidConfig("window_minutes must be in 1..=1440").boxed());
        }
        if !(1..=1_000_000).contains(&self.max_tracked) {
            return Err(GuardFailure::InvalidConfig("max_tracked must be in 1..=1000000").boxed());
        }
        if self.timestamp_bound() > MAX_TIMESTAMPS {
            return Err(GuardFailure::InvalidConfig(
                "max_tracked * (address_failures + user_failures) must not exceed 20000000",
            )
            .boxed());
        }
        Ok(())
    }

    /// How many failure timestamps the table can hold at its configured size.
    fn timestamp_bound(self) -> u64 {
        let per_window = u64::from(self.address_failures) + u64::from(self.user_failures);
        u64::try_from(self.max_tracked)
            .unwrap_or(u64::MAX)
            .saturating_mul(per_window)
    }

    pub(crate) fn window(&self) -> Duration {
        Duration::from_secs(u64::from(self.window_minutes) * 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_outside_the_configured_ranges_are_refused() {
        // The largest limits the product bound still allows together.
        let max = Settings {
            address_failures: 10_000,
            user_failures: 10_000,
            window_minutes: 1440,
            max_tracked: 1_000,
        };
        max.check().unwrap();
        Settings::default().check().unwrap();
        for invalid in [
            Settings {
                address_failures: 0,
                ..max
            },
            Settings {
                address_failures: 10_001,
                ..max
            },
            Settings {
                user_failures: 0,
                ..max
            },
            Settings {
                user_failures: 10_001,
                ..max
            },
            Settings {
                window_minutes: 0,
                ..max
            },
            Settings {
                window_minutes: 1441,
                ..max
            },
            Settings {
                max_tracked: 0,
                ..max
            },
            Settings {
                max_tracked: 1_000_001,
                ..max
            },
        ] {
            assert_eq!(
                invalid.check().unwrap_err().code().as_str(),
                "auth_guard.invalid_config"
            );
        }
    }

    #[test]
    fn a_table_promising_more_memory_than_the_bound_is_refused() {
        // Each limit passes alone; together they would remember twenty
        // million failure timestamps plus one.
        let over = Settings {
            address_failures: 10_000,
            user_failures: 10_000,
            window_minutes: 15,
            max_tracked: 1_001,
        };
        assert_eq!(over.timestamp_bound(), 20_020_000);
        assert_eq!(
            over.check().unwrap_err().code().as_str(),
            "auth_guard.invalid_config"
        );
        let at_bound = Settings {
            max_tracked: 1_000,
            ..over
        };
        at_bound.check().unwrap();
    }
}
