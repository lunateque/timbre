use timbre_edge_text::{contains_cjk, fold, order_name};

#[test]
fn folds_diacritics_case_punctuation_and_spaces() {
    for spelling in ["Ёлка", "ёлка", "Елка"] {
        assert_eq!(fold(spelling), "елка");
    }
    assert_eq!(fold(" Beyoncé\u{a0}R.E.M. \n"), "beyoncerem");
    assert_eq!(fold("Straße—Σς İ"), "strasseσσi");
}

#[test]
fn strips_only_whole_leading_articles_before_folding() {
    assert_eq!(order_name("  THE\u{a0}Cure "), "cure");
    assert_eq!(order_name("An Honest Mistake"), "honestmistake");
    assert_eq!(order_name("A Tribe Called Quest"), "tribecalledquest");
    assert_eq!(order_name("Therapy?"), "therapy");
    assert_eq!(order_name("The"), "the");
}

#[test]
fn detects_scripts_used_by_trigram_search() {
    for text in ["x東京", "かな", "カナ", "한글", "𠀀"] {
        assert!(contains_cjk(&fold(text)), "{text}");
    }
    assert!(!contains_cjk("REM ёлка 123"));
}
