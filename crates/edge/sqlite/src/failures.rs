//! The SQLite edge's failures.

use timbre_kernel::{Failure, FailureCode};

/// The database could not be opened or configured.
pub const OPEN_FAILED: FailureCode = FailureCode::new("sqlite.open_failed");
/// A statement or transaction failed.
pub const QUERY_FAILED: FailureCode = FailureCode::new("sqlite.query_failed");
/// A migration step failed; its module's earlier state was left untouched.
pub const MIGRATION_FAILED: FailureCode = FailureCode::new("sqlite.migration_failed");
/// No reader connection became available within the lease timeout.
pub const BUSY: FailureCode = FailureCode::new("sqlite.busy");
/// A single statement ran past the statement timeout and was interrupted.
pub const QUERY_TIMEOUT: FailureCode = FailureCode::new("sqlite.query_timeout");

/// Failures raised while storing data.
#[derive(Debug)]
pub enum SqliteFailure {
    /// `reason` is SQLite's own description, which may name the path.
    Open {
        /// The message of the failed open or configuration step.
        reason: String,
    },
    /// `reason` is SQLite's own description.
    Query {
        /// The message of the failed statement.
        reason: String,
    },
    /// No reader connection was free for the whole lease timeout.
    Busy,
    /// A single statement ran past the statement timeout and was interrupted.
    QueryTimeout,
    /// `module` and `version` name the step that failed.
    Migration {
        /// The owning module of the failed step.
        module: &'static str,
        /// The failed step within the module.
        version: u32,
        /// The message SQLite reported.
        reason: String,
    },
}

impl Failure for SqliteFailure {
    fn code(&self) -> FailureCode {
        match self {
            Self::Open { .. } => OPEN_FAILED,
            Self::Query { .. } => QUERY_FAILED,
            Self::Migration { .. } => MIGRATION_FAILED,
            Self::Busy => BUSY,
            Self::QueryTimeout => QUERY_TIMEOUT,
        }
    }

    fn status(&self) -> u16 {
        // `busy` is temporary pressure another request can retry through,
        // and `query_timeout` is the watchdog's own verdict; everything
        // else is a server fault the operator reads in the log, not in
        // this message.
        match self {
            Self::Busy => 503,
            Self::QueryTimeout => 504,
            Self::Open { .. } | Self::Query { .. } | Self::Migration { .. } => 500,
        }
    }

    fn public_message(&self) -> &str {
        match self {
            Self::Open { .. } | Self::Migration { .. } => "The server failed to start.",
            Self::Query { .. } | Self::Busy | Self::QueryTimeout => {
                "The server could not complete the request."
            }
        }
    }
}

/// Wrap a `rusqlite` error as a query failure.
// By value: `map_err` hands the error over; a reference would force a closure
// at every call site.
#[expect(
    clippy::needless_pass_by_value,
    reason = "used as a map_err callback, which passes the error by value"
)]
pub(crate) fn query_error(error: rusqlite::Error) -> Box<dyn timbre_kernel::Failure> {
    // The only source of `SQLITE_INTERRUPT` on these connections is the
    // statement watchdog installed by `Database::open_with`, so an
    // interrupted statement is a timeout, not someone else's cancel.
    if error.sqlite_error_code() == Some(rusqlite::ErrorCode::OperationInterrupted) {
        return Box::new(SqliteFailure::QueryTimeout);
    }
    Box::new(SqliteFailure::Query {
        reason: error.to_string(),
    })
}

/// Wrap a `rusqlite` error as an open failure.
#[expect(
    clippy::needless_pass_by_value,
    reason = "used as a map_err callback, which passes the error by value"
)]
pub(crate) fn open_error(error: rusqlite::Error) -> Box<dyn timbre_kernel::Failure> {
    Box::new(SqliteFailure::Open {
        reason: error.to_string(),
    })
}
