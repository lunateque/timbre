use std::time::Duration;
use timbre_library::fuzzing::{canonical_lrc, parse_lrc};

#[test]
fn offset_applies_to_every_timestamp_including_repeated_lines_and_words() {
    let text = "[ar:Artist]\n[00:01.25]<00:01.35>Hello <00:02.123>world\n[00:04][00:08]Again\n[offset:-300]\n";
    let lyrics = parse_lrc(text);
    let ms = |n| Some(Duration::from_millis(n));
    assert_eq!(lyrics.len(), 3);
    assert_eq!(
        (lyrics[0].start, &*lyrics[0].text),
        (ms(950), "Hello world")
    );
    assert_eq!(
        (lyrics[0].words[0].start, &*lyrics[0].words[0].text),
        (Duration::from_millis(1050), "Hello ")
    );
    assert_eq!(
        (lyrics[0].words[1].start, &*lyrics[0].words[1].text),
        (Duration::from_millis(1823), "world")
    );
    assert_eq!((lyrics[1].start, &*lyrics[1].text), (ms(3700), "Again"));
    assert_eq!((lyrics[2].start, &*lyrics[2].text), (ms(7700), "Again"));
    assert_eq!(parse_lrc(&canonical_lrc(&lyrics)), lyrics);
}

#[test]
fn a_line_repeated_at_many_timestamps_cannot_expand_without_bound() {
    let mut text = "[00:00.00]".repeat(87_000);
    text.push_str(&"x".repeat(500_000));
    let started = std::time::Instant::now();
    let lyrics = parse_lrc(&text);
    assert!(lyrics.len() <= 64, "{} lines were produced", lyrics.len());
    let bytes: usize = lyrics.iter().map(|line| line.text.len()).sum();
    assert!(bytes <= 5 * 1024 * 1024, "{bytes} bytes were produced");
    assert!(
        started.elapsed().as_secs() < 10,
        "parsing a repeated line took too long"
    );
}
