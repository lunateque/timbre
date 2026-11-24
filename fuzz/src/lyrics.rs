use timbre_library::fuzzing::{canonical_lrc, parse_lrc};

/// Canonical LRC must preserve every parsed line and word cue.
///
/// # Panics
/// Panics when writing canonical LRC changes the parsed lyrics.
pub fn lrc_parse(data: &[u8]) {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let parsed = parse_lrc(text);
    assert_eq!(parse_lrc(&canonical_lrc(&parsed)), parsed);
}
