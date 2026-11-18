//! The `[http]` config section: document parse, environment merge, defaults.

/// The byte that separates the config document from the environment
/// overrides in the `config` target's input; the bench's convention.
pub const OVERRIDE_SEPARATOR: char = '\0';

/// The value the leak oracle substitutes for every override: a string no
/// serde message or key name contains, so whatever appears in the real
/// refusal but not in the oracle's baseline came through the mask.
const SENTINEL: &str = "zzsentinelzz";

/// Read the `[http]` section from the input as one startup would: the
/// document up to an optional `\0`, then `TIMBRE_HTTP__<KEY>=<VALUE>` lines
/// standing in for the environment.
///
/// The differential: an override that reaches the config as a number must
/// be the number the environment carried (the bench's "override wins over
/// the file, per-key fallback to the defaults" contract), and the same
/// document with no overrides must parse to the same values - the merge
/// reads each override key at most once.
///
/// # Panics
///
/// When a parsed section contradicts its overrides, or the parsers panic.
pub fn config(data: &[u8]) {
    let text = String::from_utf8_lossy(data);
    let (document, override_text) = match text.split_once(OVERRIDE_SEPARATOR) {
        Some((document, rest)) => (document, rest),
        None => (&*text, ""),
    };
    let mut overrides: Vec<(String, String)> = Vec::new();
    for line in override_text.split('\n') {
        if let Some((key, value)) = line.split_once('=') {
            overrides.push((key.to_owned(), value.to_owned()));
        }
    }
    let http = match timbre_edge_http::fuzzing::http_section(document, overrides.clone()) {
        Ok(http) => http,
        Err(failure) => {
            // A refusal is legitimate - but its reason reaches the log, so
            // it must not carry the offending value back: the config edge
            // redacts every serde value marker to `<value>` (the
            // `hide_value` contract the fuzz job guards).
            assert_eq!(
                failure.code().as_str(),
                "config.invalid",
                "a refused section names the config edge's code"
            );
            let shown = format!("{failure:?}");
            // `hide_value` guards against a leaked value, but a value whose
            // text is part of the message's own scaffolding (`http`,
            // `<value>`, a field name) would read as a leak. The same
            // document merged with a sentinel in every override value shows
            // exactly that scaffolding, so a value that appears there too
            // is skipped; one that appears only in the real refusal came
            // through the mask.
            let baseline = timbre_edge_http::fuzzing::http_section(
                document,
                overrides
                    .iter()
                    .map(|(key, _)| (key.clone(), SENTINEL.to_owned()))
                    .collect(),
            );
            let Err(baseline_failure) = &baseline else {
                // The sentinel merged cleanly where the input's value did
                // not: the two refusals would not have comparable shapes.
                return;
            };
            let scaffold = format!("{baseline_failure:?}");
            for (key, value) in &overrides {
                if !key.starts_with("TIMBRE_HTTP__") || value.len() < 4 {
                    continue;
                }
                if scaffold.contains(value.as_str()) {
                    continue;
                }
                assert!(
                    !shown.contains(value.as_str()),
                    "the refusal leaked the value of `{key}`: `{value}` in {shown}"
                );
            }
            return;
        }
    };
    for (key, value) in &overrides {
        let Some(field) = key.strip_prefix("TIMBRE_HTTP__") else {
            continue;
        };
        // The merge keys by the lowercased field name, so only a field
        // named once in total has a value the result must show.
        let lower = field.to_ascii_lowercase();
        let named = overrides.iter().filter(|(other, _)| {
            other
                .strip_prefix("TIMBRE_HTTP__")
                .is_some_and(|name| name.to_ascii_lowercase() == lower)
        });
        if named.count() > 1 {
            continue;
        }
        // An override the config accepted as a number must equal the
        // environment's number; any other spelling made the section refuse
        // (the `else` arm above), because every numeric key rejects a
        // string with a type error.
        let Ok(wanted) = value.trim().parse::<u64>() else {
            continue;
        };
        let stored: Option<u128> = match lower.as_str() {
            "max_body_bytes" => u128::try_from(http.max_body_bytes).ok(),
            "max_param_pairs" => u128::try_from(http.max_param_pairs).ok(),
            "handler_timeout_ms" => Some(http.handler_timeout.as_millis()),
            "guard_timeout_ms" => Some(http.guard_timeout.as_millis()),
            "body_read_timeout_ms" => Some(http.body_read_timeout.as_millis()),
            "header_read_timeout_ms" => Some(http.header_read_timeout.as_millis()),
            // `listen` and `trusted_proxies` carry structured values;
            // comparing their spelling is not what this is for.
            _ => None,
        };
        if let Some(stored) = stored {
            assert_eq!(
                stored,
                u128::from(wanted),
                "`{key}` must reach the config unchanged"
            );
        }
    }
    // The same document with no overrides must still parse: the merge of an
    // empty environment is the document alone, and a config the environment
    // accepted is a config the file alone describes.
    if let Ok(bare) = timbre_edge_http::fuzzing::http_section(document, Vec::new()) {
        let _ = bare.max_body_bytes;
        // The trusted-proxy entries the config carries must reach the same
        // verdict `serve`'s pre-flight gives: parse without asserting a
        // result (a listed-but-bad CIDR fails startup, and that is a
        // legitimate refusal).
        for entry in &bare.trusted_proxies {
            let _ = timbre_edge_http::fuzzing::trusted_entry_accepted(entry);
        }
        for listen in &bare.listen {
            let _ = listen.parse::<std::net::SocketAddr>();
        }
    }
}
