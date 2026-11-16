//! UTC calendar time for log stamps, on std only.

use std::time::{SystemTime, UNIX_EPOCH};

/// Days since 1970-01-01 to a proleptic Gregorian date, after Howard Hinnant's
/// `civil_from_days`. All divisions are exact floor divisions because the
/// input days are never negative: pre-epoch times clamp to the epoch above.
const fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + if month <= 2 { 1 } else { 0 };
    (year, month, day)
}

/// Split a time into `(days, second_of_day, millis_of_second)` after clamping
/// instants before the Unix epoch onto it.
fn split(t: SystemTime) -> (i64, i64, u32) {
    // Clocks in the past (a wrong system clock) must not produce nonsense stamps.
    let since = t.duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = i64::try_from(since.as_secs()).unwrap_or(i64::MAX);
    (
        secs.div_euclid(86_400),
        secs.rem_euclid(86_400),
        since.subsec_millis(),
    )
}

/// Append `value` zero-padded to `width` digits. `value` must be non-negative.
fn push_padded(out: &mut String, value: i64, width: usize) {
    let digits = value.to_string();
    for _ in digits.len()..width {
        out.push('0');
    }
    out.push_str(&digits);
}

fn push_clock(out: &mut String, second_of_day: i64, millis: u32) {
    push_padded(out, second_of_day / 3_600, 2);
    out.push(':');
    push_padded(out, second_of_day / 60 % 60, 2);
    out.push(':');
    push_padded(out, second_of_day % 60, 2);
    out.push('.');
    push_padded(out, i64::from(millis), 3);
}

/// `YYYY-MM-DDTHH:MM:SS.mmmZ`, the JSON stamp.
pub(crate) fn iso(t: SystemTime) -> String {
    let (days, second_of_day, millis) = split(t);
    let (year, month, day) = civil_from_days(days);
    let mut out = String::with_capacity(24);
    push_padded(&mut out, year, 4);
    out.push('-');
    push_padded(&mut out, month, 2);
    out.push('-');
    push_padded(&mut out, day, 2);
    out.push('T');
    push_clock(&mut out, second_of_day, millis);
    out.push('Z');
    out
}

/// `HH:MM:SS.mmm`, the pretty stamp.
pub(crate) fn clock(t: SystemTime) -> String {
    let (_days, second_of_day, millis) = split(t);
    let mut out = String::with_capacity(12);
    push_clock(&mut out, second_of_day, millis);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn fixed() -> SystemTime {
        UNIX_EPOCH + Duration::from_millis(1_794_832_496_789)
    }

    #[test]
    fn epoch_formats_as_1970() {
        assert_eq!(iso(UNIX_EPOCH), "1970-01-01T00:00:00.000Z");
    }

    #[test]
    fn leap_day_of_2000() {
        let t = UNIX_EPOCH + Duration::from_secs(951_868_799);
        assert_eq!(iso(t), "2000-02-29T23:59:59.000Z");
    }

    #[test]
    fn fixed_instant() {
        assert_eq!(iso(fixed()), "2026-11-16T12:34:56.789Z");
        assert_eq!(clock(fixed()), "12:34:56.789");
    }

    #[test]
    fn before_epoch_clamps_to_epoch() {
        let t = UNIX_EPOCH - Duration::from_secs(1);
        assert_eq!(iso(t), "1970-01-01T00:00:00.000Z");
    }
}
