//! The logger: filter, format, clock and a mutex-guarded sink.

use crate::ctx::RequestId;
use crate::log::{Level, LogEvent, LogFilter, LogValue, format};
use std::io::IsTerminal;
use std::sync::Mutex;
use std::sync::PoisonError;
use std::time::SystemTime;

/// A source of timestamps; tests pass a fixed one.
pub type Clock = fn() -> SystemTime;

/// The shape of a rendered line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    /// One JSON object per line, for pipes and log collectors.
    Json,
    /// Aligned text, for a terminal.
    Pretty,
}

/// Structured logger shared by the whole process behind one `Arc<Kernel>`.
pub struct Logger {
    filter: LogFilter,
    format: Format,
    clock: Clock,
    sink: Mutex<Box<dyn std::io::Write + Send>>,
}

impl Logger {
    /// Build a logger over an arbitrary sink.
    pub fn new(
        filter: LogFilter,
        format: Format,
        sink: Box<dyn std::io::Write + Send>,
        clock: Clock,
    ) -> Self {
        Self {
            filter,
            format,
            clock,
            sink: Mutex::new(sink),
        }
    }

    /// A logger on stdout: pretty when attached to a terminal, JSON otherwise.
    pub fn stdout(filter: LogFilter) -> Self {
        let format = if std::io::stdout().is_terminal() {
            Format::Pretty
        } else {
            Format::Json
        };
        Self::new(filter, format, Box::new(std::io::stdout()), SystemTime::now)
    }

    /// Whether a line of `level` from `module` would be written.
    pub fn enabled(&self, level: Level, module: &str) -> bool {
        level <= self.filter.level_for(module)
    }

    /// Render and write one line, when its level passes the module's threshold.
    /// Writing never fails the caller: a broken sink must not stop the server.
    pub fn write(
        &self,
        request: Option<RequestId>,
        level: Level,
        event: &LogEvent,
        fields: &[(&'static str, &dyn LogValue)],
    ) {
        if !self.enabled(level, event.module()) {
            return;
        }
        let now = (self.clock)();
        let line = match self.format {
            Format::Json => format::json(now, level, event, request, fields),
            Format::Pretty => format::pretty(now, level, event, request, fields),
        };
        let mut sink = self.sink.lock().unwrap_or_else(PoisonError::into_inner);
        // Logging never fails the caller, so sink errors are dropped.
        let _ = sink.write_all(line.as_bytes()).and_then(|()| sink.flush());
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::registry::Kernel;
    use crate::{Ctx, LogEvent, Registry, RequestId, Secret, log};
    use std::cell::Cell;
    use std::io::Write;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    pub(crate) const GREETED: LogEvent = LogEvent::new("hello.greeted");

    pub(crate) fn fixed() -> SystemTime {
        UNIX_EPOCH + Duration::from_millis(1_794_832_496_789)
    }

    #[derive(Clone, Default)]
    pub(crate) struct Shared(Arc<Mutex<Vec<u8>>>);

    impl Write for Shared {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            let mut guard = self.0.lock().unwrap_or_else(PoisonError::into_inner);
            guard.extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl Shared {
        pub(crate) fn text(&self) -> String {
            let guard = self.0.lock().unwrap_or_else(PoisonError::into_inner);
            String::from_utf8(guard.clone()).expect("logs are utf-8")
        }
    }

    pub(crate) fn kernel(sink: Shared, format: Format, filter: &str) -> Arc<Kernel> {
        let filter = LogFilter::parse(filter).expect("test filter parses");
        Registry::new().finish(
            Logger::new(filter, format, Box::new(sink), fixed),
            Arc::new(crate::ThreadRuntime::new()),
        )
    }

    fn logged_call(ctx: &Ctx) {
        log!(
            ctx,
            info,
            GREETED,
            greeting = "hi \"x\"\n",
            n = -3i32,
            ok = true,
            ms = Duration::from_millis(1500),
            token = Secret::new("abc"),
            level = "x"
        );
    }

    #[test]
    fn json_line_is_exact() {
        let sink = Shared::default();
        let kernel = kernel(sink.clone(), Format::Json, "info");
        let ctx = Ctx::root(&kernel).with_request(RequestId::new(255));
        logged_call(&ctx);
        assert_eq!(
            sink.text(),
            "{\"ts\":\"2026-11-16T12:34:56.789Z\",\"level\":\"info\",\"event\":\"hello.greeted\",\"request\":\"00000000000000ff\",\"greeting\":\"hi \\\"x\\\"\\n\",\"n\":-3,\"ok\":true,\"ms\":1500,\"token\":\"***\",\"f_level\":\"x\"}\n"
        );
    }

    #[test]
    fn pretty_line_is_exact() {
        let sink = Shared::default();
        let kernel = kernel(sink.clone(), Format::Pretty, "info");
        let ctx = Ctx::root(&kernel).with_request(RequestId::new(255));
        logged_call(&ctx);
        assert_eq!(
            sink.text(),
            "12:34:56.789 INFO  hello.greeted request=00000000000000ff greeting=\"hi \\\"x\\\"\\n\" n=-3 ok=true ms=1500 token=*** f_level=\"x\"\n"
        );
    }

    #[test]
    fn non_finite_float_is_json_null() {
        let sink = Shared::default();
        let kernel = kernel(sink.clone(), Format::Json, "info");
        let ctx = Ctx::root(&kernel);
        log!(ctx, info, GREETED, x = f64::NAN);
        assert_eq!(
            sink.text(),
            "{\"ts\":\"2026-11-16T12:34:56.789Z\",\"level\":\"info\",\"event\":\"hello.greeted\",\"x\":null}\n"
        );
    }

    #[test]
    fn control_characters_escape_as_unicode() {
        let sink = Shared::default();
        let kernel = kernel(sink.clone(), Format::Json, "info");
        let ctx = Ctx::root(&kernel);
        let one = char::from_u32(1).expect("U+0001 is a scalar value");
        let input = one.to_string();
        log!(ctx, info, GREETED, s = input.as_str());
        // Raw string: a literal backslash, u, 0, 0, 0, 1 in the output.
        assert!(
            sink.text().contains(r#""s":"\u0001""#),
            "U+0001 must escape, got {}",
            sink.text()
        );
    }

    #[test]
    fn disabled_lines_skip_writing_and_fields() {
        let sink = Shared::default();
        let kernel = kernel(sink.clone(), Format::Json, "warn");
        let ctx = Ctx::root(&kernel);
        let counter = Cell::new(0u32);
        log!(
            ctx,
            info,
            GREETED,
            n = {
                counter.set(counter.get() + 1);
                1
            }
        );
        assert_eq!(sink.text(), "");
        assert_eq!(counter.get(), 0);
    }

    #[test]
    fn module_override_sets_its_own_threshold() {
        let sink = Shared::default();
        let kernel = kernel(sink.clone(), Format::Json, "warn,hello=debug");
        let ctx = Ctx::root(&kernel);
        log!(ctx, debug, GREETED);
        log!(ctx, trace, GREETED);
        let text = sink.text();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 1, "only debug must be written: {text}");
        assert!(
            lines
                .first()
                .expect("one line")
                .contains(r#""level":"debug""#)
        );
    }
}
