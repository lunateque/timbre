//! The scanned catalog, artwork, and filesystem tree.
#![warn(missing_docs)]

mod albums;
mod art;
mod catalog;
mod commands;
mod config;
mod credits;
mod digest;
pub mod failures;
mod group;
mod jobs;
pub mod log_events;
mod lrc;
mod media;
mod scan;
mod search;
mod store;
mod tree;
mod views;

/// The production parser and canonical renderer used by the fuzz harness.
#[cfg(feature = "fuzzing")]
pub mod fuzzing {
    pub use crate::lrc::canonical_lrc;
    use timbre_kernel::catalog::LyricLine;

    /// Parse the production LRC representation.
    pub fn parse_lrc(text: &str) -> Vec<LyricLine> {
        crate::lrc::parse_lrc(text)
    }
}

use std::sync::{Arc, Mutex};
use timbre_kernel::artwork::Artwork;
use timbre_kernel::catalog::{Catalog, ScanStatus};
use timbre_kernel::library_tree::LibraryTree;
use timbre_kernel::{Ctx, Fallible, Permission, Port, Registry};

const SCAN_PERMISSION: Permission = Permission::new("library.scan");

#[derive(Default)]
struct Library {
    scan: Mutex<ScanStatus>,
    art: art::ArtCache,
}

impl Port for Library {}

/// The scan status of this module's one instance. The jobs and commands
/// are plain function pointers that cannot capture it, so the state
/// travels through the port registry like every other port; registering
/// the trait rather than the state keeps `Library` itself out of it.
trait LibraryState: Port {
    fn scan(&self) -> &Mutex<ScanStatus>;
}

impl LibraryState for Library {
    fn scan(&self) -> &Mutex<ScanStatus> {
        &self.scan
    }
}

fn state(ctx: &Ctx) -> Fallible<Arc<dyn LibraryState>> {
    ctx.ports()
        .get::<dyn LibraryState>()
        .ok_or_else(|| failures::LibraryFailure::NotConfigured.boxed())
}

/// Register the catalog, tree, artwork, migration, jobs, and commands.
///
/// # Errors
/// Returns a registration failure when a declaration conflicts.
pub fn register(registry: &mut Registry) -> Fallible {
    let library = Arc::new(Library::default());
    registry.provide::<dyn Catalog>(library.clone())?;
    registry.provide::<dyn Artwork>(library.clone())?;
    registry.provide::<dyn LibraryTree>(library.clone())?;
    registry.provide::<dyn LibraryState>(library)?;
    registry.migration("library", 1, include_str!("schema.sql"))?;
    registry.permission(SCAN_PERMISSION, "Run a library scan")?;
    registry.job(log_events::SCAN, jobs::scan)?;
    registry.job(log_events::WATCH, jobs::watch)?;
    registry.command(
        "scan",
        "Scan music roots (use --full to reread all tags).",
        commands::scan,
    )?;
    registry.command(
        "library explain",
        "Explain how one scanned path was indexed.",
        commands::explain,
    )
}
