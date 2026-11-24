//! The SQLite edge: the only crate in the tree that speaks SQL.
//!
//! [`Database`] owns one writer connection behind a mutex and a pool of
//! reader connections; [`Database::write`] and [`Database::read`] are blocking
//! calls designed for `Ctx::blocking`. [`Database::migrate`] applies the
//! registered migrations under an exclusive transaction, and the check,
//! backup and prune operations back the binary's `db` commands. Every wait
//! for a connection -- the reader lease and the writer gate -- is bounded at
//! five seconds and fails with `sqlite.busy`.

#![warn(missing_docs)]

mod database;
pub mod failures;
mod files;
mod gate;
pub mod json;
pub mod log_events;
mod migrate;
mod pool;
mod shared;
mod tables;

pub use database::Database;
pub use rusqlite;
pub use shared::SharedDatabase;
