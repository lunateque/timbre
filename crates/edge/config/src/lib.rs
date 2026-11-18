//! Configuration: one TOML file plus environment overrides, read once at startup.
//!
//! [`Config::load`] finds the file (the explicit argument, then `TIMBRE_CONFIG`,
//! then `./timbre.toml`) and keeps its value tree; [`Config::section`] hands a
//! module its own typed section with `TIMBRE_<SECTION>__<KEY>` overrides
//! applied over the file over the section's defaults.

#![warn(missing_docs)]

pub mod failures;
mod redact;

#[cfg(feature = "fuzzing")]
#[doc(hidden)]
pub mod fuzzing;

use crate::failures::ConfigFailure;
use redact::hide_value;

use serde::de::DeserializeOwned;
use std::path::{Path, PathBuf};
use timbre_kernel::Fallible;

/// Re-export of the `serde` crate: modules derive `Deserialize` for their
/// section structs through `timbre_edge_config::serde`, so the external
/// crate stays an edge-only dependency and the derive's paths agree with
/// the version this crate deserializes with (`#[serde(crate = "...")]`).
pub use ::serde;

/// The environment variable naming the config file when no path was given.
pub const CONFIG_ENV: &str = "TIMBRE_CONFIG";
/// The file used when neither an argument nor `CONFIG_ENV` names one.
pub const DEFAULT_FILE: &str = "timbre.toml";

/// A parsed config document, plus where it came from.
pub struct Config {
    path: Option<PathBuf>,
    document: toml::Table,
}

/// The binary's `[server]` section.
#[derive(serde::Deserialize)]
#[serde(crate = "crate::serde", default)]
pub struct ServerSection {
    /// Where the database and caches live; default `data`.
    pub data_dir: String,
}

impl Default for ServerSection {
    fn default() -> Self {
        ServerSection {
            data_dir: "data".to_owned(),
        }
    }
}

/// The binary's `[log]` section; `TIMBRE_LOG` still wins over `filter`.
#[derive(serde::Deserialize, Default)]
#[serde(crate = "crate::serde", default)]
pub struct LogSection {
    /// The filter specification, `warn,hello=debug` style.
    pub filter: Option<String>,
}

impl Config {
    /// Read the config. `explicit` wins over `TIMBRE_CONFIG`, which wins over
    /// `./timbre.toml`; none of them existing means an empty document, so every
    /// section falls back to its defaults.
    ///
    /// # Errors
    ///
    /// `config.invalid` when the chosen file exists but cannot be read or
    /// parsed; the failure names the file and, for a parse error, the line.
    pub fn load(explicit: Option<&Path>) -> Fallible<Config> {
        let path = explicit
            .map(Path::to_path_buf)
            .or_else(|| std::env::var_os(CONFIG_ENV).map(PathBuf::from))
            .unwrap_or_else(|| PathBuf::from(DEFAULT_FILE));
        if !path.exists() {
            // A file named by an argument that was deleted between the check
            // and the read falls through to the `Err` arm below.
            return Ok(Config {
                path: None,
                document: toml::Table::new(),
            });
        }
        let text = std::fs::read_to_string(&path).map_err(|error| {
            Box::new(ConfigFailure::Invalid {
                file: path.display().to_string(),
                line: None,
                reason: hide_value(&error.to_string()),
            }) as Box<dyn timbre_kernel::Failure>
        })?;
        let document = parse_document(&path.display().to_string(), &text)?;
        Ok(Config {
            path: Some(path),
            document,
        })
    }

    /// The file the document was read from, `None` when there was none.
    #[must_use]
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// The `name` section as `T`. Precedence is environment, then file, then
    /// the section's defaults; `T` should derive `Default` and carry
    /// `#[serde(default)]`, so keys absent from both sources fall back.
    ///
    /// # Errors
    ///
    /// `config.invalid` when the section is not a table, an override variable
    /// has an empty key, or the merged table does not deserialize into `T`.
    pub fn section<T: DeserializeOwned + Default>(&self, name: &str) -> Fallible<T> {
        self.section_with(name, std::env::vars_os().filter_map(unicode_vars))
    }

    /// `section` with the environment handed in, so the merge is testable
    /// without mutating the process environment.
    fn section_with<T: DeserializeOwned + Default>(
        &self,
        name: &str,
        overrides: impl IntoIterator<Item = (String, String)>,
    ) -> Fallible<T> {
        let mut table = match self.document.get(name) {
            Some(toml::Value::Table(table)) => table.clone(),
            Some(
                toml::Value::String(_)
                | toml::Value::Integer(_)
                | toml::Value::Float(_)
                | toml::Value::Boolean(_)
                | toml::Value::Datetime(_)
                | toml::Value::Array(_),
            ) => {
                return Err(Box::new(ConfigFailure::Invalid {
                    file: self.file_name(),
                    line: None,
                    reason: format!("the section `{name}` is not a table"),
                }));
            }
            None => toml::Table::new(),
        };
        let prefix = format!("TIMBRE_{}__", name.to_ascii_uppercase());
        for (key, value) in overrides {
            let Some(tail) = key.strip_prefix(prefix.as_str()) else {
                continue;
            };
            let field = tail.to_ascii_lowercase();
            if field.is_empty() {
                return Err(Box::new(ConfigFailure::Invalid {
                    file: self.file_name(),
                    line: None,
                    reason: format!("`{key}` names no key in the `{name}` section"),
                }));
            }
            // A value that parses as a TOML value expression is structured
            // (arrays, tables and numbers override cleanly); anything else is
            // taken as a string.
            let parsed = value
                .parse::<toml::Value>()
                .unwrap_or(toml::Value::String(value));
            table.insert(field, parsed);
        }
        toml::Value::Table(table).try_into().map_err(|error| {
            // `{error}` is the parser's message plus the key chain (`in
            // \`data_dir\``); the key stays, the offending literal goes.
            let reason = format!("the section `{name}` is malformed: {error}");
            Box::new(ConfigFailure::Invalid {
                file: self.file_name(),
                line: None,
                reason: hide_value(&reason),
            }) as Box<dyn timbre_kernel::Failure>
        })
    }

    fn file_name(&self) -> String {
        self.path.as_ref().map_or_else(
            || "<no config file>".to_owned(),
            |path| path.display().to_string(),
        )
    }
}

/// A `(key, value)` pair whose Os strings are both Unicode; non-Unicode
/// environment entries are not ours to interpret.
fn unicode_vars(pair: (std::ffi::OsString, std::ffi::OsString)) -> Option<(String, String)> {
    let (key, value) = pair;
    Some((key.into_string().ok()?, value.into_string().ok()?))
}

/// The one TOML parse both `load` and the `fuzzing` seam use: a table or a
/// `config.invalid` that names the file and the line.
pub(crate) fn parse_document(file: &str, text: &str) -> Fallible<toml::Table> {
    toml::from_str::<toml::Table>(text).map_err(|error| {
        Box::new(ConfigFailure::Invalid {
            file: file.to_owned(),
            line: line_of(text, error.span()),
            // `Display` quotes the offending source line back; a config
            // value can be a secret, so only the parser's own words stay.
            reason: hide_value(error.message()),
        }) as Box<dyn timbre_kernel::Failure>
    })
}

/// The one-based line holding the error's span start, when the span is known.
fn line_of(text: &str, span: Option<std::ops::Range<usize>>) -> Option<usize> {
    let start = span?.start.min(text.len());
    Some(text[..start].matches('\n').count() + 1)
}

#[cfg(test)]
mod tests;
