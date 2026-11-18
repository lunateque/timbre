//! `LogFilter::parse`: the `TIMBRE_LOG` filter specification.

use timbre_kernel::{Level, LogFilter};

/// The level a documented word names, spelled from the contract rather than
/// reused from the parser's own table: this is the differential the target
/// compares against.
fn expected_level(word: &str) -> Option<Level> {
    match word.to_ascii_lowercase().as_str() {
        "error" => Some(Level::Error),
        "warn" => Some(Level::Warn),
        "info" => Some(Level::Info),
        "debug" => Some(Level::Debug),
        "trace" => Some(Level::Trace),
        _ => None,
    }
}

/// Parse the whole input as a filter spec and hold it to its contract:
/// a bare word sets the default, `module=level` items override exactly that
/// module, every other module keeps the default, level words are matched
/// ASCII-case-insensitively, item order does not matter, and every refusal
/// carries `kernel.log_filter_invalid`.
///
/// # Panics
///
/// When a verdict contradicts the contract above, or the parser panics.
pub fn log_filter(data: &[u8]) {
    let spec = String::from_utf8_lossy(data);
    let filter = match LogFilter::parse(&spec) {
        Ok(filter) => filter,
        Err(failure) => {
            assert_eq!(
                failure.code().as_str(),
                "kernel.log_filter_invalid",
                "every refusal names the kernel's filter-code"
            );
            return;
        }
    };
    // Re-derivation of the contract's answer straight from the spec text,
    // independent of the filter's internals.
    let mut want_default = Level::Info;
    let mut want_modules: Vec<(String, Level)> = Vec::new();
    for item in spec.split(',') {
        let item = item.trim();
        if item.is_empty() {
            continue;
        }
        match item.split_once('=') {
            None => {
                // A spec that parsed has at most one bare word, and it is a
                // level word.
                want_default = expected_level(item).expect("a parsed spec has level words only");
            }
            Some((module, level)) => want_modules.push((
                module.trim().to_owned(),
                expected_level(level.trim()).expect("a parsed spec has level words only"),
            )),
        }
    }
    // `u-v` names no possible override: a module segment may not hold `-`,
    // so whatever the spec, this module keeps the default.
    assert_eq!(
        filter.level_for("u-v"),
        want_default,
        "the default level reaches every unmentioned module"
    );
    for (module, level) in &want_modules {
        assert_eq!(
            filter.level_for(module),
            *level,
            "`{module}={level:?}` must override exactly that module"
        );
    }
    // Two spec transformations that must leave every verdict the same: the
    // level words recased (ASCII-case-insensitive per the contract; module
    // names are not recased, they have no case to ignore) and the items
    // sorted. Compared through `level_for`, not struct equality: the
    // override list keeps insertion order, which is visible to `PartialEq`
    // but carries no meaning - that ordering is not a bug.
    let upper = spec
        .split(',')
        .map(|item| match item.split_once('=') {
            Some((module, level)) => format!("{module}={}", level.to_ascii_uppercase()),
            None => item.to_ascii_uppercase(),
        })
        .collect::<Vec<String>>()
        .join(",");
    let mut items: Vec<&str> = spec.split(',').collect();
    items.sort_unstable();
    let sorted = items.join(",");
    for reform in [&upper, &sorted] {
        let other = LogFilter::parse(reform)
            .unwrap_or_else(|_| panic!("`{reform}` must parse whenever `{spec}` does"));
        assert_eq!(
            other.level_for("u-v"),
            want_default,
            "the default survives `{reform}`"
        );
        for (module, level) in &want_modules {
            assert_eq!(
                other.level_for(module),
                *level,
                "override `{module}` survives `{reform}`"
            );
        }
    }
}
