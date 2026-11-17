//! The SQLite edge's own log events.

use timbre_kernel::LogEvent;

/// A migration step was applied; `module` and `version` name it.
pub const MIGRATION_APPLIED: LogEvent = LogEvent::new("sqlite.migration_applied");
