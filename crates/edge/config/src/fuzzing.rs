//! Public seams over the private merge machinery, for `fuzz/`.
//!
//! `Config::section` reads the live process environment, which a fuzzer
//! cannot feed per input; these wrappers hand the same merge the document
//! and the overrides as plain arguments. The module exists only under the
//! `fuzzing` cargo feature, which nothing in the shipped build enables.

use crate::Config;
use serde::de::DeserializeOwned;
use timbre_kernel::Fallible;

/// The whole read a section needs, from text instead of a file: `Config::load`'s
/// TOML parse, then `Config::section`'s merge with the environment handed in as
/// `overrides` (`TIMBRE_<SECTION>__<KEY>` shaped pairs), so no process state is
/// read and no file is touched.
///
/// # Errors
///
/// `config.invalid` exactly as `Config::load` plus `Config::section` report it.
pub fn section<T: DeserializeOwned + Default>(
    document: &str,
    name: &str,
    overrides: Vec<(String, String)>,
) -> Fallible<T> {
    let parsed = crate::parse_document("<fuzz document>", document)?;
    let config = Config {
        path: None,
        document: parsed,
    };
    config.section_with(name, overrides)
}
