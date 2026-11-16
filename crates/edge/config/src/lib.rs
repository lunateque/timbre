//! Configuration: one TOML file plus environment overrides, read once at startup.
//!
//! [`Config::load`] finds the file (the explicit argument, then `TIMBRE_CONFIG`,
//! then `./timbre.toml`) and keeps its value tree; [`Config::section`] hands a
//! module its own typed section with `TIMBRE_<SECTION>__<KEY>` overrides
//! applied over the file over the section's defaults.

#![warn(missing_docs)]

mod failures;

pub use failures::{ConfigFailure, INVALID};

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

/// The binary's `[http]` section; absent keys fall back to the edge's defaults.
#[derive(serde::Deserialize, Default)]
#[serde(crate = "crate::serde", default)]
pub struct HttpSection {
    /// Listen addresses.
    pub listen: Option<Vec<String>>,
    /// Trusted proxy entries.
    pub trusted_proxies: Option<Vec<String>>,
    /// The body size cap.
    pub max_body_bytes: Option<usize>,
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
                reason: error.to_string(),
            }) as Box<dyn timbre_kernel::Failure>
        })?;
        let document = toml::from_str::<toml::Table>(&text).map_err(|error| {
            Box::new(ConfigFailure::Invalid {
                file: path.display().to_string(),
                line: line_of(&text, error.span()),
                reason: error.to_string(),
            }) as Box<dyn timbre_kernel::Failure>
        })?;
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
            Box::new(ConfigFailure::Invalid {
                file: self.file_name(),
                line: None,
                reason: format!("the section `{name}` is malformed: {error}"),
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

/// The one-based line holding the error's span start, when the span is known.
fn line_of(text: &str, span: Option<std::ops::Range<usize>>) -> Option<usize> {
    let start = span?.start.min(text.len());
    Some(text[..start].matches('\n').count() + 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;
    use std::sync::atomic::{AtomicU32, Ordering};

    #[derive(Clone, Debug, Default, Deserialize, PartialEq)]
    #[serde(crate = "crate::serde", default)]
    struct Sample {
        name: String,
        port: u16,
        list: Vec<String>,
    }

    // A process-unique scratch directory; env and files must not collide
    // between parallel tests or earlier runs.
    struct Dir(PathBuf);

    impl Dir {
        fn new(tag: &str) -> Dir {
            static COUNTER: AtomicU32 = AtomicU32::new(0);
            let unique = format!(
                "timbre-config-test-{}-{tag}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::SeqCst)
            );
            let path = std::env::temp_dir().join(unique);
            std::fs::create_dir_all(&path).expect("the scratch directory exists");
            Dir(path)
        }

        fn file(&self, content: &str) -> PathBuf {
            let path = self.0.join("timbre.toml");
            std::fs::write(&path, content).expect("the config file is written");
            path
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_missing_file_is_defaults() {
        let dir = Dir::new("missing");
        let config = Config::load(Some(&dir.0.join("absent.toml"))).expect("absent is defaults");
        assert_eq!(config.path(), None);
        assert_eq!(
            config.section::<Sample>("absent").expect("defaults"),
            Sample::default()
        );
    }

    #[test]
    fn a_broken_file_names_it_and_the_line() {
        let dir = Dir::new("broken");
        let path = dir.file("name = \"x\"\n[bad = =\n");
        let error = Config::load(Some(&path))
            .err()
            .expect("a parse error fails the load");
        assert_eq!(error.code(), INVALID);
        let message = format!("{error:?}");
        assert!(message.contains("timbre.toml"), "{message}");
        assert!(
            message.contains("line: Some(2)"),
            "line 2 is reported: {message}"
        );
    }

    #[test]
    fn environment_wins_over_the_file_and_the_file_over_defaults() {
        let dir = Dir::new("precedence");
        let path = dir.file("[precedence]\nname = \"from-file\"\nport = 1\n");
        let config = Config::load(Some(&path)).expect("the file parses");
        let section: Sample = config
            .section_with(
                "precedence",
                [("TIMBRE_PRECEDENCE__PORT".to_owned(), "2".to_owned())],
            )
            .expect("the override deserializes");
        // env overrides the file; the file overrides the default.
        assert_eq!(
            section,
            Sample {
                name: "from-file".to_owned(),
                port: 2,
                list: Vec::new(),
            }
        );
    }

    #[test]
    fn a_non_parsing_override_value_becomes_a_string() {
        let dir = Dir::new("string");
        let config = Config::load(Some(&dir.file(""))).expect("an empty file parses");
        let section: Sample = config
            .section_with(
                "string",
                [("TIMBRE_STRING__NAME".to_owned(), "raw text".to_owned())],
            )
            .expect("the string value lands in the string field");
        assert_eq!(section.name, "raw text");
    }

    #[test]
    fn an_override_value_may_be_a_toml_array() {
        let dir = Dir::new("array");
        let config = Config::load(Some(&dir.file(""))).expect("an empty file parses");
        let section: Sample = config
            .section_with(
                "array",
                [("TIMBRE_ARRAY__LIST".to_owned(), r#"["a","b"]"#.to_owned())],
            )
            .expect("the array value lands in the list field");
        assert_eq!(section.list, ["a", "b"]);
    }

    #[test]
    fn a_malformed_section_is_invalid() {
        let dir = Dir::new("malformed");
        let config = Config::load(Some(&dir.file("[malformed]\nport = \"not-a-number\"\n")))
            .expect("the file parses");
        let error = config
            .section::<Sample>("malformed")
            .expect_err("a string is not a port number");
        assert_eq!(error.code(), INVALID);
    }

    #[test]
    fn a_section_that_is_not_a_table_is_invalid() {
        let dir = Dir::new("scalar");
        let config = Config::load(Some(&dir.file("scalar = 1"))).expect("the file parses");
        let error = config
            .section::<Sample>("scalar")
            .expect_err("a scalar is not a section");
        assert_eq!(error.code(), INVALID);
    }
}
