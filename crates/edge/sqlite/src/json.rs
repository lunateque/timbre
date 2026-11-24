//! Typed JSON columns for structured database values that are not queried individually.

use crate::failures::SqliteFailure;
use serde::Serialize;
use serde::de::DeserializeOwned;
use timbre_kernel::{Failure, Fallible};

/// Encode a value for a JSON column.
///
/// # Errors
/// A `sqlite.json_failed` failure when the value cannot be encoded.
pub fn encode<T: Serialize + ?Sized>(value: &T) -> Fallible<String> {
    serde_json::to_string(value).map_err(|error| failure(&error))
}

/// Decode a value read from a JSON column.
///
/// # Errors
/// A `sqlite.json_failed` failure when the stored value has the wrong shape.
pub fn decode<T: DeserializeOwned>(value: &str) -> Fallible<T> {
    serde_json::from_str(value).map_err(|error| failure(&error))
}

fn failure(error: &serde_json::Error) -> Box<dyn Failure> {
    Box::new(SqliteFailure::Json {
        reason: error.to_string(),
    })
}
