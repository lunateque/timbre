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
/// One line of `timbre help`: `name` and `help`.
pub(crate) const HELP: LogEvent = LogEvent::new("timbre.help");
/// A CLI handler returned a failure; the code names it.
pub(crate) const COMMAND_FAILED: LogEvent = LogEvent::new("timbre.command_failed");
/// The command line itself is unusable: `reason` says why, exit is 2.
pub(crate) const USAGE: LogEvent = LogEvent::new("timbre.usage");
/// `timbre db check` finished: `ok` says whether the integrity check passed.
pub(crate) const DB_CHECK: LogEvent = LogEvent::new("timbre.db_check");
/// `timbre db backup` finished: `to` names the destination.
pub(crate) const DB_BACKUP: LogEvent = LogEvent::new("timbre.db_backup");
/// `timbre db prune`: `tables` lists unowned table names, `dropped` says
/// whether they were removed (only with `--yes`).
pub(crate) const DB_PRUNE: LogEvent = LogEvent::new("timbre.db_prune");
