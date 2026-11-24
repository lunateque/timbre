//! The visible directory view of the scanned library.

mod browse;
pub mod failures;
pub mod log_events;

use std::sync::Arc;
use timbre_edge_config::{Config, serde};
use timbre_kernel::folders::Folders;
use timbre_kernel::{Ctx, Fallible, Registry};

#[derive(Default, serde::Deserialize)]
#[serde(crate = "timbre_edge_config::serde", default, deny_unknown_fields)]
struct Settings {}

fn configure(ctx: &Ctx) -> Fallible {
    if let Some(config) = ctx.resource::<Config>() {
        config.section::<Settings>("folders")?;
    }
    Ok(())
}

/// Register the module.
///
/// # Errors
///
/// Returns a failure if the `Folders` port already has a provider.
pub fn register(registry: &mut Registry) -> Fallible {
    registry.config_check(configure);
    registry.provide::<dyn Folders>(Arc::new(browse::Browse))
}
