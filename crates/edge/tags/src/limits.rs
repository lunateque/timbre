//! Caps applied while tags are read, so one hostile file cannot grow a row,
//! a database transaction or the scanner's memory without bound.

/// Longest text value kept for a tag field.
pub(crate) const MAX_TEXT_BYTES: usize = 64 * 1024;
/// Longest lyrics body kept, embedded or synchronized.
pub(crate) const MAX_LYRIC_BYTES: usize = 1024 * 1024;
/// Values kept for one multi-valued field.
pub(crate) const MAX_VALUES: usize = 256;
/// Lyrics entries kept for one file.
pub(crate) const MAX_LYRICS: usize = 32;
/// Synchronized lyric cues kept for one entry.
pub(crate) const MAX_CUES: usize = 20_000;
/// Pictures kept for one file.
pub(crate) const MAX_PICTURES: usize = 16;
/// Largest picture whose bytes are carried out of the reader.
pub(crate) const MAX_PICTURE_BYTES: usize = 32 * 1024 * 1024;

/// Cut `value` to at most `limit` bytes, on a character boundary.
pub(crate) fn text(mut value: String, limit: usize) -> String {
    if value.len() > limit {
        let cut = value
            .char_indices()
            .map(|(offset, _)| offset)
            .take_while(|offset| *offset <= limit)
            .last()
            .unwrap_or_default();
        value.truncate(cut);
    }
    value
}

#[cfg(test)]
mod tests {
    #[test]
    fn text_is_cut_on_a_character_boundary() {
        let value = "é".repeat(10);
        let cut = super::text(value, 5);
        assert_eq!(cut, "éé");
    }

    #[test]
    fn short_text_is_kept() {
        assert_eq!(super::text("abc".to_owned(), 5), "abc");
    }
}
