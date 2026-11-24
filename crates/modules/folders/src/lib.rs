//! The visible directory view of the scanned library.

mod browse;
pub mod failures;
pub mod log_events;

use std::sync::Arc;
use timbre_kernel::folders::Folders;
use timbre_kernel::{Fallible, Registry};

/// Register the module.
///
/// # Errors
///
/// Returns a failure if the `Folders` port already has a provider.
pub fn register(registry: &mut Registry) -> Fallible {
    registry.provide::<dyn Folders>(Arc::new(browse::Browse))
}
