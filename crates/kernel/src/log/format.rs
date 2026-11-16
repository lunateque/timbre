//! Render one log line as JSON or as a human-readable line.

use crate::ctx::RequestId;
use crate::log::{Level, LogEvent, LogValue, Value, time};
use std::time::SystemTime;

/// Field names that would collide with the envelope keys of the JSON format.
const RESERVED: [&str; 4] = ["ts", "level", "event", "request"];

fn push_key(out: &mut String, key: &str) {
    if RESERVED.contains(&key) {
        out.push_str("f_");
    }
    out.push_str(key);
}

/// Append `text` wrapped in quotes, with the escapes both formats share.
fn push_escaped(out: &mut String, text: &str) {
    out.push('"');
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            control @ '\0'..='\u{1f}' => {
                // Fixed \u00XX with lowercase hex digits, spelled out to keep
                // the line building with `push` only.
                let code = u32::from(control);
                let hi = char::from_digit(code / 16, 16).unwrap_or('0');
                let lo = char::from_digit(code % 16, 16).unwrap_or('0');
                out.push_str("\\u00");
                out.push(hi);
                out.push(lo);
            }
            other => out.push(other),
        }
    }
    out.push('"');
}

fn json_value(out: &mut String, value: Value<'_>) {
    match value {
        Value::Bool(b) => out.push_str(if b { "true" } else { "false" }),
        Value::Int(i) => out.push_str(&i.to_string()),
        Value::Float(f) if f.is_finite() => out.push_str(&f.to_string()),
        // JSON has no infinity or NaN; null keeps every line parseable.
        Value::Float(_) => out.push_str("null"),
        Value::Str(s) => push_escaped(out, s),
        Value::Redacted => out.push_str("\"***\""),
    }
}

fn pretty_value(out: &mut String, value: Value<'_>) {
    match value {
        Value::Bool(b) => out.push_str(if b { "true" } else { "false" }),
        Value::Int(i) => out.push_str(&i.to_string()),
        Value::Float(f) => out.push_str(&f.to_string()),
        Value::Str(s) => push_escaped(out, s),
        Value::Redacted => out.push_str("***"),
    }
}

pub(crate) fn json(
    now: SystemTime,
    level: Level,
    event: &LogEvent,
    request: Option<RequestId>,
    fields: &[(&'static str, &dyn LogValue)],
) -> String {
    let mut out = String::with_capacity(128);
    out.push_str("{\"ts\":");
    push_escaped(&mut out, &time::iso(now));
    out.push_str(",\"level\":");
    push_escaped(&mut out, level.as_str());
    out.push_str(",\"event\":");
    push_escaped(&mut out, event.as_str());
    if let Some(request) = request {
        out.push_str(",\"request\":");
        push_escaped(&mut out, &request.to_string());
    }
    for (key, value) in fields {
        out.push_str(",\"");
        push_key(&mut out, key);
        out.push_str("\":");
        json_value(&mut out, value.log_value());
    }
    out.push_str("}\n");
    out
}

pub(crate) fn pretty(
    now: SystemTime,
    level: Level,
    event: &LogEvent,
    request: Option<RequestId>,
    fields: &[(&'static str, &dyn LogValue)],
) -> String {
    let mut out = String::with_capacity(128);
    out.push_str(&time::clock(now));
    out.push(' ');
    // Padded to five columns so event names line up across levels.
    out.push_str(match level {
        Level::Error => "ERROR",
        Level::Warn => "WARN ",
        Level::Info => "INFO ",
        Level::Debug => "DEBUG",
        Level::Trace => "TRACE",
    });
    out.push(' ');
    out.push_str(event.as_str());
    if let Some(request) = request {
        out.push_str(" request=");
        out.push_str(&request.to_string());
    }
    for (key, value) in fields {
        out.push(' ');
        push_key(&mut out, key);
        out.push('=');
        pretty_value(&mut out, value.log_value());
    }
    out.push('\n');
    out
}
