//! `ApiKey::parse`: the one canonical spelling of a device secret.

/// `ApiKey::parse` against its contract: a key is accepted only in the exact
/// spelling `render` writes, so a parse that succeeds must round-trip back
/// to the very same text, and no other text may name the same key. That
/// uniqueness is what lets the prefix be a lookup handle: two spellings of
/// one key would mean a row that can be found two ways and compared once.
///
/// # Panics
///
/// When an accepted key does not render back to its input, when the
/// rendering is not accepted again, or when the parser panics.
pub fn api_key(data: &[u8]) {
    let text = String::from_utf8_lossy(data);
    let Some(key) = timbre_auth::ApiKey::parse(&text) else {
        return;
    };
    let rendered = key.render();
    assert!(
        rendered == text.as_ref(),
        "an accepted key must be spelled exactly as it renders"
    );
    assert!(
        timbre_auth::ApiKey::parse(&rendered).is_some_and(|again| again.render() == rendered),
        "parsing is the inverse of rendering"
    );
    // The two halves are fixed-length, so the rendered form is too; a parse
    // that accepted anything else would have changed the alphabet.
    assert_eq!(rendered.len(), 13 + 1 + 52, "a key has one shape");
    assert_eq!(
        rendered.matches('_').count(),
        1,
        "exactly one separator, or the halves would be ambiguous"
    );
}
