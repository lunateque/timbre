//! The `[auth]` section. Nothing in it is required: a server with no
//! settings at all still has users, devices and secrets.

use crate::failures::AuthFailure;
use std::sync::Arc;
use timbre_edge_config::{Config, serde};
use timbre_edge_sqlite::SharedDatabase;
use timbre_kernel::{Ctx, Fallible};

pub(crate) const MAX_EXPIRY_DAYS: u32 = 36_500;

#[derive(Default, serde::Deserialize)]
#[serde(crate = "timbre_edge_config::serde", default, deny_unknown_fields)]
pub(crate) struct Settings {
    /// Days a device issued without `--expires` is good for; absent means a
    /// device that never expires, which is what a home server wants for its
    /// own players.
    pub(crate) device_default_expiry_days: Option<u32>,
}

impl Settings {
    pub(crate) fn load(ctx: &Ctx) -> Fallible<Self> {
        let Some(config) = ctx.resource::<Config>() else {
            // No configuration installed at all is a valid host; the
            // defaults then apply.
            return Ok(Self::default());
        };
        let settings: Self = config.section("auth")?;
        if let Some(days) = settings.device_default_expiry_days
            && !(1..=MAX_EXPIRY_DAYS).contains(&days)
        {
            return Err(AuthFailure::InvalidConfig(days).boxed());
        }
        Ok(settings)
    }
}

pub(crate) fn shared_db(ctx: &Ctx) -> Fallible<Arc<SharedDatabase>> {
    SharedDatabase::require(ctx, AuthFailure::NotConfigured)
}
