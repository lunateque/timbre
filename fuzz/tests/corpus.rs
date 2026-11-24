//! Stable replay of the fuzz corpus through the target functions.
//!
//! This is the `just ci` smoke run: it needs no nightly and no clang (the
//! fuzz package's `libfuzzer-sys` dependency links libFuzzer's C++ runtime
//! only when cargo-fuzz's flags ask for it), but it replays every
//! checked-in seed - including the regression fixtures from past crashes -
//! through the same assertion-bearing target functions a fuzzer drives. A
//! parser change that breaks one of the invariants on a known-interesting
//! input fails here, without waiting for the nightly fuzz job.

use std::path::Path;

/// Every file under `corpus/<target>`, sorted, each read whole; a corpus
/// directory can hold subdirectories, which `cargo fuzz` creates for its own
/// inputs, so they are all walked.
fn inputs(target: &str) -> Vec<(String, Vec<u8>)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("corpus")
        .join(target);
    let mut files = Vec::new();
    let mut pending = vec![root];
    while let Some(directory) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
                continue;
            }
            let name = path.display().to_string();
            let data =
                std::fs::read(&path).unwrap_or_else(|error| panic!("cannot read {name}: {error}"));
            files.push((name, data));
        }
    }
    files.sort_by(|left, right| left.0.cmp(&right.0));
    files
}

/// Replay every seed through `exercise`, naming the file that failed.
/// `catch_unwind` turns the target's own panic into a message carrying the
/// seed path, which is what makes a regression reproducible in CI output.
fn replay(target: &str, exercise: fn(&[u8])) {
    let files = inputs(target);
    assert!(!files.is_empty(), "`{target}` has a seed corpus");
    for (name, data) in files {
        let outcome = std::panic::catch_unwind(|| exercise(&data));
        if let Err(panic) = outcome {
            let reason = panic
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| panic.downcast_ref::<&str>().map(ToString::to_string))
                .unwrap_or_else(|| "a panic without a message".to_owned());
            panic!("seed `{name}` failed the `{target}` checks: {reason}");
        }
    }
}

#[test]
fn http_head_seeds_hold_their_invariants() {
    replay("http_head", timbre_fuzz::http_head);
}

#[test]
fn form_roundtrip_seeds_hold_their_invariants() {
    replay("form_roundtrip", timbre_fuzz::form_roundtrip);
}

#[test]
fn proxy_seeds_hold_their_invariants() {
    replay("proxy", timbre_fuzz::proxy);
}

#[test]
fn identity_seeds_hold_their_invariants() {
    replay("identity", timbre_fuzz::identity);
}

#[test]
fn config_seeds_hold_their_invariants() {
    replay("config", timbre_fuzz::config);
}

#[test]
fn log_filter_seeds_hold_their_invariants() {
    replay("log_filter", timbre_fuzz::log_filter);
}

#[test]
fn tags_read_seeds_hold_their_invariants() {
    replay("tags_read", timbre_fuzz::tags_read);
}

#[test]
fn image_thumbnail_seeds_hold_their_invariants() {
    replay("image_thumbnail", timbre_fuzz::image_thumbnail);
}

#[test]
fn lrc_parse_seeds_hold_their_invariants() {
    replay("lrc_parse", timbre_fuzz::lrc_parse);
}
