//! Calendar time for people: the stamps commands print and the dates they
//! type. The logger already carries the calendar arithmetic for its own
//! stamps, and a second implementation in a module would be a second set of
//! leap-year bugs.

use crate::log::time;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Milliseconds in a day, for epoch-based expiry and retention.
pub const DAY_MILLIS: i64 = 24 * 60 * 60 * 1000;

/// The current system time in whole Unix milliseconds.
pub fn now_unix_millis() -> i64 {
    unix_millis(SystemTime::now())
}

/// Whole Unix milliseconds, clamped to `0..=i64::MAX`.
pub fn unix_millis(time: SystemTime) -> i64 {
    i64::try_from(
        time.duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
    .unwrap_or(i64::MAX)
}

/// Convert whole Unix milliseconds to a system time. Negative or
/// unrepresentable values are reported as the Unix epoch.
pub fn from_unix_millis(millis: i64) -> SystemTime {
    UNIX_EPOCH
        .checked_add(Duration::from_millis(
            u64::try_from(millis).unwrap_or_default(),
        ))
        .unwrap_or(UNIX_EPOCH)
}

/// `YYYY-MM-DDTHH:MM:SS.mmmZ` in UTC, the spelling the log stamps use.
#[must_use]
pub fn iso(time: SystemTime) -> String {
    time::iso(time)
}

/// Midnight UTC of a `YYYY-MM-DD` date, or `None` when the text is not such
/// a date (including a day that month does not have).
#[must_use]
pub fn parse_date(text: &str) -> Option<SystemTime> {
    let (year, rest) = text.split_once('-')?;
    let (month, day) = rest.split_once('-')?;
    if year.len() != 4 || month.len() != 2 || day.len() != 2 {
        return None;
    }
    let number = |part: &str| -> Option<i64> {
        part.bytes()
            .all(|byte| byte.is_ascii_digit())
            .then(|| part.parse().ok())
            .flatten()
    };
    let (year, month, day) = (number(year)?, number(month)?, number(day)?);
    let days = time::days_from_civil(year, month, day);
    // A date like 2026-02-30 converts to some day number; converting back is
    // what tells it apart from a real date.
    if time::civil_from_days(days) != (year, month, day) || days < 0 {
        return None;
    }
    let seconds = u64::try_from(days).ok()?.checked_mul(86_400)?;
    Some(UNIX_EPOCH + Duration::from_secs(seconds))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_date_is_midnight_utc_of_that_day() {
        let day = parse_date("2026-11-16").expect("a real date");
        assert_eq!(iso(day), "2026-11-16T00:00:00.000Z");
        assert_eq!(
            parse_date("1970-01-01").expect("the epoch"),
            SystemTime::UNIX_EPOCH
        );
        assert_eq!(
            iso(parse_date("2000-02-29").expect("a leap day")),
            "2000-02-29T00:00:00.000Z"
        );
    }

    #[test]
    fn nothing_but_a_real_date_parses() {
        for text in [
            "",
            "2026",
            "2026-11",
            "2026-11-16T00:00:00Z",
            "26-11-16",
            "2026-1-16",
            "2026-13-01",
            "2026-02-30",
            "2026-00-10",
            "2026-11-00",
            "1969-12-31",
            "20x6-11-16",
            " 2026-11-16",
        ] {
            assert!(parse_date(text).is_none(), "accepted {text:?}");
        }
    }
}
