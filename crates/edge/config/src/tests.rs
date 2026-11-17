//! Configuration reading, exercised without touching the real environment.

use super::*;
use crate::failures::INVALID;
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
fn a_parse_error_does_not_echo_the_offending_line() {
    // `Display` quotes the source line back; a secret line must not reach
    // the log verbatim.
    let dir = Dir::new("echo");
    let path = dir.file("token = \"s3cr3t-t0ken\" x\n");
    let error = Config::load(Some(&path))
        .err()
        .expect("a parse error fails the load");
    let message = format!("{error:?}");
    assert!(!message.contains("s3cr3t"), "{message}");
    assert!(message.contains("line: Some(1)"), "line stays: {message}");
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
fn a_bad_value_is_redacted_but_its_section_and_key_stay() {
    // The offending value can be a secret; the reason must name where
    // the problem is without repeating what it is.
    let dir = Dir::new("redact");
    let config = Config::load(Some(&dir.file("[redact]\nname = 2024\n"))).expect("the file parses");
    let error = config
        .section::<Sample>("redact")
        .expect_err("an integer is not a name");
    let reason = format!("{error:?}");
    assert_eq!(error.code(), INVALID);
    assert!(!reason.contains("2024"), "the value is replaced: {reason}");
    assert!(reason.contains("<value>"), "the marker stays: {reason}");
    assert!(reason.contains("redact"), "the section is named: {reason}");
    assert!(reason.contains("name"), "the key is named: {reason}");
}

#[test]
fn a_hidden_secret_string_never_reaches_the_reason() {
    let dir = Dir::new("secret");
    let config = Config::load(Some(&dir.file(""))).expect("an empty file parses");
    let error = config
        .section_with::<Sample>(
            "secret",
            [("TIMBRE_SECRET__PORT".to_owned(), r#""hunter2""#.to_owned())],
        )
        .expect_err("a string is not a port number");
    let reason = format!("{error:?}");
    assert!(!reason.contains("hunter2"), "the secret is gone: {reason}");
}

#[derive(Debug, Default, Deserialize)]
#[serde(crate = "crate::serde", rename_all = "lowercase")]
enum Mode {
    Dev,
    #[default]
    Prod,
}

#[derive(Debug, Default, Deserialize)]
#[serde(crate = "crate::serde", default)]
struct EnumSample {
    mode: Mode,
}

#[test]
fn an_unknown_enum_variant_hides_its_value_and_names_its_key() {
    // The rejected spelling of an enum-typed key is a value too: a user
    // may have put a secret where a variant name belongs.
    let dir = Dir::new("variant");
    let config = Config::load(Some(&dir.file("[variant]\nmode = \"s3cr3t-choice\"\n")))
        .expect("the file parses");
    let error = config
        .section::<EnumSample>("variant")
        .expect_err("not a known variant");
    let reason = format!("{error:?}");
    assert_eq!(error.code(), INVALID);
    assert!(
        !reason.contains("s3cr3t-choice"),
        "the rejected variant is replaced: {reason}"
    );
    assert!(reason.contains("<value>"), "the marker stays: {reason}");
    assert!(reason.contains("mode"), "the key is named: {reason}");
    // The expected variants are not secrets: they stay for the operator.
    assert!(reason.contains("dev"), "the expected list stays: {reason}");
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
