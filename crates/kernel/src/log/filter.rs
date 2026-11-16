//! Parse and answer the `TIMBRE_LOG` level filter.

use crate::failure::Fallible;
use crate::failures::KernelFailure;
use crate::log::Level;
use crate::names;

/// A default log level plus per-module overrides.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogFilter {
    default: Level,
    modules: Vec<(String, Level)>,
}

impl Default for LogFilter {
    fn default() -> Self {
        Self {
            default: Level::Info,
            modules: Vec::new(),
        }
    }
}

fn parse_level(word: &str) -> Option<Level> {
    if word.eq_ignore_ascii_case("error") {
        Some(Level::Error)
    } else if word.eq_ignore_ascii_case("warn") {
        Some(Level::Warn)
    } else if word.eq_ignore_ascii_case("info") {
        Some(Level::Info)
    } else if word.eq_ignore_ascii_case("debug") {
        Some(Level::Debug)
    } else if word.eq_ignore_ascii_case("trace") {
        Some(Level::Trace)
    } else {
        None
    }
}

impl LogFilter {
    /// Parse a comma-separated spec: one bare level word sets the default,
    /// `module=level` items add overrides. Whitespace around items is ignored,
    /// empty items are skipped, an empty spec yields the default filter.
    ///
    /// # Errors
    ///
    /// Returns `kernel.log_filter_invalid` for an unknown level word, a second
    /// default item, a repeated module, an invalid module name, or an empty
    /// side of `=`.
    pub fn parse(spec: &str) -> Fallible<LogFilter> {
        let mut filter = LogFilter::default();
        let mut default_set = false;
        for item in spec.split(',') {
            let item = item.trim();
            if item.is_empty() {
                continue;
            }
            match item.split_once('=') {
                None => {
                    // A second bare level contradicts the first: which one meant what?
                    if default_set {
                        return Err(Box::new(KernelFailure::LogFilterInvalid));
                    }
                    let Some(level) = parse_level(item) else {
                        return Err(Box::new(KernelFailure::LogFilterInvalid));
                    };
                    filter.default = level;
                    default_set = true;
                }
                Some((module, level)) => {
                    let module = module.trim();
                    let level = level.trim();
                    if module.is_empty() || level.is_empty() || !names::is_valid_segment(module) {
                        return Err(Box::new(KernelFailure::LogFilterInvalid));
                    }
                    let Some(level) = parse_level(level) else {
                        return Err(Box::new(KernelFailure::LogFilterInvalid));
                    };
                    if filter
                        .modules
                        .iter()
                        .any(|(existing, _)| existing == module)
                    {
                        return Err(Box::new(KernelFailure::LogFilterInvalid));
                    }
                    filter.modules.push((module.to_owned(), level));
                }
            }
        }
        Ok(filter)
    }

    /// The threshold for `module`: its override if present, else the default.
    pub fn level_for(&self, module: &str) -> Level {
        self.modules
            .iter()
            .find(|(name, _)| name == module)
            .map_or(self.default, |(_, level)| *level)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_spec_is_default() {
        for spec in ["", "  "] {
            let filter =
                LogFilter::parse(spec).unwrap_or_else(|_| panic!("spec {spec:?} must parse"));
            assert_eq!(filter, LogFilter::default());
            assert_eq!(filter.level_for("hello"), Level::Info);
        }
    }

    #[test]
    fn bare_word_sets_default() {
        let filter = LogFilter::parse("debug").expect("debug parses");
        assert_eq!(filter.level_for("anything"), Level::Debug);
    }

    #[test]
    fn overrides_are_spaced_and_scoped() {
        let filter = LogFilter::parse("info, stream = debug").expect("spec parses");
        assert_eq!(filter.level_for("stream"), Level::Debug);
        assert_eq!(filter.level_for("hello"), Level::Info);
    }

    #[test]
    fn level_words_are_case_insensitive() {
        let filter = LogFilter::parse("INFO").expect("INFO parses");
        assert_eq!(filter.level_for("anything"), Level::Info);
    }

    #[test]
    fn invalid_specs_fail_with_the_kernel_code() {
        for spec in [
            "loud",
            "info,warn",
            "hello=trace,hello=warn",
            "Hello=debug",
            "=debug",
            "hello=",
        ] {
            let error = LogFilter::parse(spec)
                .err()
                .unwrap_or_else(|| panic!("spec {spec:?} must fail"));
            assert_eq!(
                error.code(),
                crate::failures::LOG_FILTER_INVALID,
                "spec {spec:?} must report kernel.log_filter_invalid"
            );
        }
    }
}
