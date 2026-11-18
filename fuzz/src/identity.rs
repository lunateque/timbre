//! `UserName::normalize`: the one canonical spelling of an account name.

/// Whether every accepted name obeys the documented alphabet.
fn in_alphabet(name: &str) -> bool {
    name.chars()
        .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || matches!(ch, '.' | '_' | '-'))
}

/// `UserName::normalize` against its contract: the accepted name is exactly
/// the trimmed, lowercased input, and a refusal carries that same candidate
/// back for reporting; the verdict must follow the documented alphabet and
/// length bounds.
///
/// # Panics
///
/// When the verdict contradicts the candidate or the rules, or the parser
/// panics.
pub fn identity(data: &[u8]) {
    let text = String::from_utf8_lossy(data);
    let expected = text
        .trim_matches(|ch: char| ch.is_ascii_whitespace())
        .to_ascii_lowercase();
    match timbre_kernel::identity::UserName::normalize(&text) {
        Ok(name) => {
            assert_eq!(
                name.as_str(),
                expected,
                "an accepted name must be the trimmed, lowercased input"
            );
            assert!(
                !name.as_str().is_empty()
                    && name.as_str().len() <= timbre_kernel::identity::UserName::MAX_LENGTH
                    && in_alphabet(name.as_str()),
                "an accepted name obeys the documented rules: `{name}`"
            );
            // Normalizing a canonical name is the identity.
            let again = timbre_kernel::identity::UserName::normalize(name.as_str())
                .unwrap_or_else(|rejected| panic!("`{name}` is canonical, got `{rejected}`"));
            assert_eq!(again, name, "normalization is idempotent");
        }
        Err(candidate) => {
            assert_eq!(
                candidate, expected,
                "a refusal must carry the trimmed, lowercased candidate"
            );
            let broken = candidate.is_empty()
                || candidate.len() > timbre_kernel::identity::UserName::MAX_LENGTH
                || !in_alphabet(&candidate);
            assert!(
                broken,
                "a refused candidate must break a documented rule: `{candidate}`"
            );
        }
    }
}
