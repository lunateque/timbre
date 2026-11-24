//! Filesystem watcher failures.

use timbre_kernel::{Failure, FailureCode};

/// A watch could not be established or the watch channel closed.
pub const WATCH: FailureCode = FailureCode::new("fswatch.watch_failed");

/// The backend's watch diagnostic.
#[derive(Debug)]
pub struct WatchFailure(pub String);

impl Failure for WatchFailure {
    fn code(&self) -> FailureCode {
        WATCH
    }

    fn status(&self) -> u16 {
        500
    }

    fn public_message(&self) -> &'static str {
        "Filesystem monitoring is unavailable."
    }
}
