//! Unicode search and browse normalization.
#![warn(missing_docs)]

use icu_casemap::CaseMapper;
use icu_normalizer::DecomposingNormalizer;
use icu_properties::CodePointMapData;
use icu_properties::props::{GeneralCategory, GeneralCategoryGroup, Script};
use icu_properties::script::ScriptWithExtensionsBorrowed;
use std::borrow::Cow;

/// Canonically decompose, remove nonspacing marks, and apply Unicode case folding.
/// Punctuation and whitespace do not participate in matching.
pub fn fold(text: &str) -> String {
    let categories = CodePointMapData::<GeneralCategory>::new();
    let normalized = DecomposingNormalizer::new_nfd().normalize(text);
    let mut filtered = String::with_capacity(normalized.len());
    filtered.extend(normalized.chars().filter(|&ch| {
        let category = categories.get(ch);
        category != GeneralCategory::NonspacingMark
            && !GeneralCategoryGroup::Punctuation.contains(category)
            && !ch.is_whitespace()
    }));
    // NFD already separates the two dots from the Cyrillic letter.
    match CaseMapper::new().fold_string(&filtered) {
        Cow::Borrowed(_) => filtered,
        Cow::Owned(folded) => folded,
    }
}

/// Fold a browse name after removing a leading English article (`a`, `an`, `the`).
/// The caller supplies a tagged sort name in preference to the display name.
pub fn order_name(name: &str) -> String {
    let name = name.trim();
    if let Some((first, rest)) = name.split_once(char::is_whitespace)
        && ["a", "an", "the"]
            .iter()
            .any(|article| first.eq_ignore_ascii_case(article))
    {
        return fold(rest);
    }
    fold(name)
}

/// Whether the text contains Han, Hiragana, Katakana or Hangul.
/// The caller also checks the folded character count before choosing trigram search.
pub fn contains_cjk(text: &str) -> bool {
    let scripts = ScriptWithExtensionsBorrowed::new();
    text.chars().any(|ch| {
        [
            Script::Han,
            Script::Hiragana,
            Script::Katakana,
            Script::Hangul,
        ]
        .into_iter()
        .any(|script| scripts.has_script(ch, script))
    })
}

/// The shipped parser entry points for fuzz harnesses.
#[cfg(feature = "fuzzing")]
pub mod fuzzing {
    pub use crate::{contains_cjk, fold, order_name};
}
