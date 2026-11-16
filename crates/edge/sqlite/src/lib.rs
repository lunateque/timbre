//! The SQLite edge: the only crate in the tree that speaks SQL.
//!
//! [`Database`] owns one writer connection behind a mutex and a pool of
//! reader connections; [`Database::write`] and [`Database::read`] are blocking
//! calls designed for `Ctx::blocking`. [`Database::migrate`] applies the
//! registered migrations under an exclusive transaction, and the check,
//! backup and prune operations back the binary's `db` commands.

#![warn(missing_docs)]

mod database;
mod failures;
mod migrate;

pub use database::Database;
pub use failures::{MIGRATION_FAILED, OPEN_FAILED, QUERY_FAILED, SqliteFailure};
