#[cfg(feature = "fuzzing")]
use std::fmt::Write as _;
use std::sync::Arc;
use std::time::Duration;
use timbre_kernel::catalog::{LyricLine, WordCue};

/// Timestamps accepted on one LRC line.
const MAX_MARKERS: usize = 64;
/// Lines produced from one lyric.
const MAX_LINES: usize = 10_000;
/// Text bytes produced from one lyric.
const MAX_BYTES: usize = 4 * 1024 * 1024;

fn timestamp(value: &str) -> Option<u64> {
    let (prefix, seconds) = value.rsplit_once(':')?;
    let (hours, minutes) = prefix.split_once(':').unwrap_or(("0", prefix));
    let (seconds, fraction) = seconds.split_once('.').unwrap_or((seconds, "0"));
    let integer = |part: &str| {
        (!part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
            .then(|| part.parse::<u64>().ok())
            .flatten()
    };
    let fraction = integer(fraction)?.checked_mul(match fraction.len() {
        1 => 100,
        2 => 10,
        3 => 1,
        _ => return None,
    })?;
    let seconds = integer(seconds)?;
    if seconds >= 60 {
        return None;
    }
    integer(hours)?
        .checked_mul(60)?
        .checked_add(integer(minutes)?)?
        .checked_mul(60)?
        .checked_add(seconds)?
        .checked_mul(1000)?
        .checked_add(fraction)
}

fn marker(text: &str, open: char, close: char) -> Option<(u64, &str)> {
    let text = text.strip_prefix(open)?;
    // A representable timestamp is shorter than this. Never scan a whole lyric for a bracket.
    let end = text
        .as_bytes()
        .iter()
        .take(32)
        .position(|&byte| char::from(byte) == close)?;
    Some((timestamp(text.get(..end)?)?, text.get(end + 1..)?))
}

fn header(text: &str) -> Option<(&str, &str)> {
    let (name, body) = text.strip_prefix('[')?.split_once(':')?;
    if !name.starts_with(|ch: char| ch.is_ascii_alphabetic())
        || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
    {
        return None;
    }
    Some((name, body.strip_suffix(']')?))
}

pub(crate) fn language(text: &str) -> Option<&str> {
    text.trim_start_matches('\u{feff}')
        .lines()
        .find_map(|line| {
            let (name, value) = header(line.trim())?;
            (name.eq_ignore_ascii_case("lang") && !value.trim().is_empty()).then_some(value.trim())
        })
}

struct Timed {
    starts: Vec<u64>,
    text: String,
}

fn lines_with_times(text: &str) -> (Vec<Timed>, i64) {
    let mut lines: Vec<Timed> = Vec::new();
    let mut offset = 0;
    for line in text.lines() {
        let mut body = line.trim();
        if let Some((name, value)) = header(body) {
            if name.eq_ignore_ascii_case("offset")
                && let Ok(value) = value.trim().parse()
            {
                offset = value;
            }
            continue;
        }
        let mut starts = Vec::new();
        while let Some((start, tail)) = marker(body, '[', ']') {
            if starts.len() < MAX_MARKERS {
                starts.push(start);
            }
            body = tail.trim_start();
        }
        if starts.is_empty() {
            if let Some(previous) = lines.last_mut() {
                previous.text.push('\n');
                previous.text.push_str(body);
            }
        } else {
            lines.push(Timed {
                starts,
                text: body.to_owned(),
            });
        }
    }
    if let Some(last) = lines.last_mut() {
        last.text.truncate(last.text.trim_end().len());
    }
    (lines, offset)
}

fn enhanced(text: &str) -> (&str, Vec<(u64, &str)>) {
    let mut prefix = text;
    let mut previous = None;
    let mut words = Vec::new();
    for (at, _) in text.match_indices('<') {
        let Some(tail) = text.get(at..) else { continue };
        let Some((start, after)) = marker(tail, '<', '>') else {
            continue;
        };
        if let Some((time, content)) = previous {
            let length: usize = content;
            if let Some(part) = text.get(length..at) {
                words.push((time, part));
            }
        } else if let Some(part) = text.get(..at) {
            prefix = part;
        }
        previous = Some((start, text.len() - after.len()));
    }
    if let Some((start, at)) = previous
        && let Some(part) = text.get(at..)
    {
        words.push((start, part));
    }
    (prefix, words)
}

/// Parse LRC or plain lyrics, applying the final offset to line and word times.
/// A line repeated at many timestamps shares one text allocation, and the
/// expansion is capped: `[0:00]` repeated 87 000 times in front of half a
/// megabyte of text would otherwise ask for tens of gibibytes.
pub(crate) fn parse_lrc(text: &str) -> Vec<LyricLine> {
    let text = text.trim_start_matches('\u{feff}');
    let (groups, offset) = lines_with_times(text);
    if groups.is_empty() {
        return text
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .take(MAX_LINES)
            .map(|line| LyricLine {
                start: None,
                text: Arc::from(line),
                words: Vec::new(),
            })
            .collect();
    }
    let mut lines = Vec::new();
    let mut bytes = 0usize;
    for Timed { starts, text } in groups {
        let Some(&base) = starts.first() else {
            continue;
        };
        let (prefix, words) = enhanced(text.trim_end());
        let mut value = prefix.to_owned();
        for &(_, word) in &words {
            value.push_str(word);
        }
        let value: Arc<str> = Arc::from(value);
        let cues: Vec<(u64, Arc<str>)> = words
            .iter()
            .map(|&(at, word)| (at, Arc::from(word)))
            .collect();
        for start in starts {
            if lines.len() >= MAX_LINES || bytes >= MAX_BYTES {
                break;
            }
            bytes = bytes.saturating_add(value.len());
            let move_time = |word: u64| {
                let shifted =
                    i128::from(word) + i128::from(start) - i128::from(base) + i128::from(offset);
                Duration::from_millis(
                    u64::try_from(shifted.clamp(0, i128::from(u64::MAX))).unwrap_or_default(),
                )
            };
            lines.push(LyricLine {
                start: Some(Duration::from_millis(start.saturating_add_signed(offset))),
                text: Arc::clone(&value),
                words: cues
                    .iter()
                    .map(|(at, word)| WordCue {
                        start: move_time(*at),
                        text: Arc::clone(word),
                    })
                    .collect(),
            });
        }
    }
    lines.sort_by_key(|line| line.start);
    lines
}

#[cfg(feature = "fuzzing")]
fn clock(time: Duration, open: char, close: char, output: &mut String) {
    let seconds = time.as_secs();
    let _ = write!(
        output,
        "{open}{:02}:{:02}.{:03}{close}",
        seconds / 60,
        seconds % 60,
        time.subsec_millis()
    );
}

/// Render lines produced by [`parse_lrc`] in offset-free canonical LRC.
#[cfg(feature = "fuzzing")]
pub fn canonical_lrc(lines: &[LyricLine]) -> String {
    let mut output = String::new();
    for line in lines {
        if let Some(start) = line.start {
            clock(start, '[', ']', &mut output);
        }
        let timed_bytes = line.words.iter().map(|cue| cue.text.len()).sum::<usize>();
        if let Some(prefix) = line.text.get(..line.text.len().saturating_sub(timed_bytes)) {
            output.push_str(prefix);
        }
        for word in &line.words {
            clock(word.start, '<', '>', &mut output);
            output.push_str(&word.text);
        }
        output.push('\n');
    }
    output
}
