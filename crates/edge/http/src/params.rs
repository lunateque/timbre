//! The parameter caps and the redacted, clipped shape of everything the log
//! may carry about a target.

/// The longest query or form key one request may carry; a longer name is a
/// field nobody can read, not a parameter.
pub(crate) const MAX_PARAM_KEY_BYTES: usize = 256;

/// The most one logged query or form value may occupy; the rest of a long
/// value tells the reader nothing the head did not.
pub(crate) const MAX_LOGGED_VALUE_BYTES: usize = 120;

/// The most the whole logged target may occupy, so one request cannot push
/// an unbounded string into every log reader.
const MAX_LOGGED_TARGET_BYTES: usize = 2048;

/// The marks that a value or target was cut.
const ELLIPSIS: char = '\u{2026}';

/// Query and form keys whose names contain one of these words (matched
/// case-insensitively on the lowercase name) carry credentials: the logger
/// never shows their values. `token`, `key`, `secret`, `pass`, `auth`,
/// `cookie`, `session` and `sid` cover the OAuth, API-key, password and
/// session families without enumerating every spelling (`access_token`,
/// `client_secret`, `apiKey`, `set-cookie`, `password`, `session_id`,
/// `sid`).
pub(crate) const SECRET_QUERY_WORDS: &[&str] = &[
    "token", "key", "secret", "pass", "auth", "cookie", "session", "sid",
];

/// Query and form keys that are secret by their exact name: Subsonic's
/// `p`/`t`/`s`, its `pwd`, and the short names verification flows use.
pub(crate) const SECRET_QUERY_NAMES: &[&str] = &["p", "t", "s", "code", "pwd"];

/// The header names whose values must never reach a log line: the edge logs
/// no header value at all, and every future logger honors this list.
pub const SECRET_HEADER_NAMES: &[&str] = &[
    "authorization",
    "cookie",
    "proxy-authorization",
    "x-api-key",
    "set-cookie",
];

/// Whether `name` (already lowercased) names a header whose value stays out of
/// the log.
#[must_use]
pub fn is_secret_header(name: &str) -> bool {
    SECRET_HEADER_NAMES.contains(&name)
}

/// Whether `key` names a secret query or form value.
#[must_use]
pub(crate) fn is_secret_key(key: &str) -> bool {
    let lower = key.to_ascii_lowercase();
    SECRET_QUERY_WORDS.iter().any(|word| lower.contains(word))
        || SECRET_QUERY_NAMES.contains(&lower.as_str())
}

/// Where refused parameter pairs came from: the URL or the request form.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ParamsSource {
    Url,
    Form,
}

impl ParamsSource {
    /// The name of the source in a log line.
    #[must_use]
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Url => "url",
            Self::Form => "form",
        }
    }
}

/// The `(status, reason)` a pair list deserves when it breaks one of the
/// caps, `None` when it fits. A key over `MAX_PARAM_KEY_BYTES` is a field
/// that does not fit (431 from the URL), a list over `limit` pairs is the
/// target being too long (414 from the URL); a form answers 413 either way,
/// the payload being the oversized part.
pub(crate) fn refusal(
    pairs: &[(String, String)],
    limit: usize,
    source: ParamsSource,
) -> Option<(u16, &'static str)> {
    if pairs.iter().any(|(key, _)| key.len() > MAX_PARAM_KEY_BYTES) {
        let status = match source {
            ParamsSource::Url => 431,
            ParamsSource::Form => 413,
        };
        return Some((status, "key_too_long"));
    }
    if pairs.len() > limit {
        let status = match source {
            ParamsSource::Url => 414,
            ParamsSource::Form => 413,
        };
        return Some((status, "too_many_pairs"));
    }
    None
}

/// `text` cut to at most `budget` bytes plus an ellipsis when bytes fell off;
/// never split in the middle of a character.
#[must_use]
pub(crate) fn clip(text: &str, budget: usize) -> String {
    if text.len() <= budget {
        return text.to_owned();
    }
    let mut end = budget;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let mut out = String::with_capacity(budget + ELLIPSIS.len_utf8());
    out.push_str(text.get(..end).unwrap_or(text));
    out.push(ELLIPSIS);
    out
}

/// The query as it may be logged: secret keys keep their name, their value
/// becomes `***`, every other value is clipped, order and the rest stay as
/// they arrived.
#[must_use]
pub(crate) fn redacted_query(pairs: &[(String, String)]) -> String {
    let mut text = String::new();
    for (key, value) in pairs {
        if !text.is_empty() {
            text.push('&');
        }
        text.push_str(key);
        text.push('=');
        if is_secret_key(key) {
            text.push_str("***");
        } else {
            text.push_str(&clip(value, MAX_LOGGED_VALUE_BYTES));
        }
    }
    text
}

/// The path and query as they may be logged, whole capped at 2 KiB.
#[must_use]
pub(crate) fn logged_target(path: &str, query: &[(String, String)]) -> String {
    let text = if query.is_empty() {
        path.to_owned()
    } else {
        format!("{path}?{}", redacted_query(query))
    };
    clip(&text, MAX_LOGGED_TARGET_BYTES)
}

/// The path and query of a raw request-target as they may be logged, for the
/// lines an answer produces before the target was parsed at all: the query
/// part, if any, is decoded and its secret values redacted.
#[must_use]
pub(crate) fn redacted_target(target: &str) -> String {
    let Some((path, query)) = target.split_once('?') else {
        return clip(target, MAX_LOGGED_TARGET_BYTES);
    };
    // The raw query of a target is already bounded by hyper's 64 KiB URI cap
    // and every logged value is clipped downstream, so the redaction pass may
    // parse it whole; the bound that must hold at parse time is the form
    // body's, in `merge_form_pairs`.
    logged_target(
        path,
        &crate::form::form_pairs(query.as_bytes(), usize::MAX).0,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::form::form_pairs;

    #[test]
    fn secret_keys_are_caught_by_word_and_by_name() {
        for key in [
            "access_token",
            "client_secret",
            "apiKey",
            "API_KEY",
            "password",
            "passwd",
            "authorization",
            "session_cookie",
            "session_id",
            "sid",
            "SID",
            "p",
            "t",
            "s",
            "code",
            "pwd",
        ] {
            assert!(is_secret_key(key), "{key} names a secret");
        }
        for key in ["id", "u", "format", "offset", "salt", "c", "q"] {
            assert!(!is_secret_key(key), "{key} names nothing secret");
        }
    }

    #[test]
    fn secret_values_redact_and_others_pass_through() {
        let pairs = form_pairs(b"u=anna&p=secret&apiKey=3f&t=deadbeef&s=cafe&sid=7&x=1", 10).0;
        assert_eq!(
            redacted_query(&pairs),
            "u=anna&p=***&apiKey=***&t=***&s=***&sid=***&x=1"
        );
    }

    #[test]
    fn logged_values_clip_at_120_bytes_and_never_split_a_character() {
        let value = "x".repeat(300);
        let pairs = vec![("note".to_owned(), value.clone())];
        let logged = redacted_query(&pairs);
        assert_eq!(logged.len(), "note=".len() + 120 + "…".len());
        assert!(logged.ends_with('…'));
        // The budget lands mid-character (a two-byte e-acute); the clip steps back to the
        // character before it instead of cutting one in half.
        let wide = "é".repeat(200);
        let pairs = vec![("v".to_owned(), wide)];
        let logged = redacted_query(&pairs);
        let kept: String = logged
            .trim_start_matches("v=")
            .trim_end_matches('…')
            .chars()
            .collect();
        assert_eq!(kept.chars().count(), 60, "60 two-byte characters fit");
        assert!(std::str::from_utf8(logged.as_bytes()).is_ok());
    }

    #[test]
    fn the_whole_logged_target_is_capped() {
        let query = form_pairs("a=1&".repeat(1000).as_bytes(), 1000).0;
        let logged = logged_target("/long", &query);
        assert!(logged.len() <= 2048 + ELLIPSIS.len_utf8());
        assert!(logged.ends_with(ELLIPSIS));
        // A path that needs no query at all still caps when it is huge.
        let path = "/".to_owned() + &"x".repeat(5000);
        let logged = redacted_target(&path);
        assert_eq!(logged.len(), 2048 + ELLIPSIS.len_utf8());
        assert!(logged.ends_with(ELLIPSIS));
    }

    #[test]
    fn pairs_over_the_cap_or_with_long_keys_are_refused() {
        let many = vec![("k".to_owned(), "v".to_owned()); 5];
        assert_eq!(refusal(&many, 5, ParamsSource::Url), None);
        assert_eq!(
            refusal(&many, 4, ParamsSource::Url),
            Some((414, "too_many_pairs"))
        );
        assert_eq!(
            refusal(&many, 4, ParamsSource::Form),
            Some((413, "too_many_pairs"))
        );
        let long_key = vec![("x".repeat(257), "v".to_owned())];
        assert_eq!(
            refusal(&long_key, 1024, ParamsSource::Url),
            Some((431, "key_too_long"))
        );
        assert_eq!(
            refusal(&long_key, 1024, ParamsSource::Form),
            Some((413, "key_too_long"))
        );
        let edge = vec![("x".repeat(256), "v".to_owned())];
        assert_eq!(refusal(&edge, 1024, ParamsSource::Url), None);
    }

    #[test]
    fn header_secrets_cover_the_credential_headers() {
        assert!(is_secret_header("authorization"));
        assert!(is_secret_header("cookie"));
        assert!(is_secret_header("proxy-authorization"));
        assert!(is_secret_header("x-api-key"));
        assert!(is_secret_header("set-cookie"));
        assert!(!is_secret_header("content-type"));
    }
}
