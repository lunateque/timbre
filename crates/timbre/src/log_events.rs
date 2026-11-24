//! The binary's own log events.

use timbre_kernel::LogEvent;

/// The server finished starting; `listen` is the configured addresses.
pub(crate) const STARTED: LogEvent = LogEvent::new("timbre.started");
/// `serve` returned: the server stopped accepting and drained its
/// connections; the process is on its way out.
pub(crate) const STOPPED: LogEvent = LogEvent::new("timbre.stopped");
/// The `TIMBRE_LOG` (or config) filter could not be parsed; startup stops.
pub(crate) const LOG_FILTER_INVALID: LogEvent = LogEvent::new("timbre.log_filter_invalid");
/// A module refused to register; startup stops.
pub(crate) const REGISTRATION_FAILED: LogEvent = LogEvent::new("timbre.registration_failed");
/// The async executor could not start; no module ran.
pub(crate) const RUNTIME_FAILED: LogEvent = LogEvent::new("timbre.runtime_failed");
/// The configuration sections could not be read; startup stops.
pub(crate) const CONFIG_FAILED: LogEvent = LogEvent::new("timbre.config_failed");
/// The database could not be opened or migrated; startup stops.
pub(crate) const DATABASE_FAILED: LogEvent = LogEvent::new("timbre.database_failed");
/// A data directory that already existed is reachable beyond its owner;
/// `path` names it and `mode` is its Unix mode. The binary leaves a
/// directory it did not create alone, so this is a warning, not a failure.
#[cfg_attr(
    not(unix),
    expect(dead_code, reason = "only unix reports directory permissions")
)]
pub(crate) const DATA_DIR_PERMISSIVE: LogEvent = LogEvent::new("timbre.data_dir_permissive");
/// A CLI handler returned a failure; the code names it.
pub(crate) const COMMAND_FAILED: LogEvent = LogEvent::new("timbre.command_failed");
/// The command line itself is unusable: `reason` says why, exit is 2.
pub(crate) const USAGE: LogEvent = LogEvent::new("timbre.usage");
/// `timbre db check` found corruption or could not finish.
pub(crate) const DB_CHECK: LogEvent = LogEvent::new("timbre.db_check");
