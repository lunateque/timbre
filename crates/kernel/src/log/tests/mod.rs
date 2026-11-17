//! Logger tests: exact lines, filters, and the writer thread under stalled
//! and wedged sinks.

mod inline;

use super::*;
use crate::registry::Kernel;
use crate::testing::{Shared, fixed};
use crate::{Ctx, LogEvent, Registry, RequestId, Secret, log};
use std::cell::Cell;
use std::io::Write;
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant};

const GREETED: LogEvent = LogEvent::new("hello.greeted");

fn kernel(sink: Shared, format: Format, filter: &str) -> Arc<Kernel> {
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
    assert!(kernel.logger().flush());
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
    assert!(kernel.logger().flush());
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
    assert!(kernel.logger().flush());
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
    assert!(kernel.logger().flush());
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
    assert!(kernel.logger().flush());
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
    assert!(kernel.logger().flush());
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

/// A sink that holds every write until its gate opens, as a stalled pipe does.
struct Gated {
    gate: Arc<(Mutex<bool>, Condvar)>,
    out: Shared,
}

impl Write for Gated {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let (open, wake) = &*self.gate;
        let mut open = open.lock().unwrap_or_else(PoisonError::into_inner);
        while !*open {
            open = wake.wait(open).unwrap_or_else(PoisonError::into_inner);
        }
        drop(open);
        self.out.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn a_stalled_sink_drops_lines_and_counts_them() {
    const TOTAL: usize = 5_000;
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let sink = Shared::default();
    let kernel = Registry::new().finish(
        Logger::new(
            LogFilter::default(),
            Format::Json,
            Box::new(Gated {
                gate: Arc::clone(&gate),
                out: sink.clone(),
            }),
            fixed,
        ),
        Arc::new(crate::ThreadRuntime::new()),
    );
    let ctx = Ctx::root(&kernel);
    // The caller returns from every call although the sink has written nothing.
    for _ in 0..TOTAL {
        log!(ctx, info, GREETED);
    }
    let (open, wake) = &*gate;
    *open.lock().unwrap_or_else(PoisonError::into_inner) = true;
    wake.notify_all();
    assert!(kernel.logger().flush());
    let text = sink.text();
    let delivered = text
        .lines()
        .filter(|line| line.contains(r#""event":"hello.greeted""#))
        .count();
    let dropped: usize = text
        .lines()
        .filter(|line| line.contains(r#""event":"kernel.log_dropped""#))
        .map(|line| {
            crate::testing::json_field(line, "count")
                .expect("a count field")
                .parse::<usize>()
                .expect("a numeric count")
        })
        .sum();
    assert!(
        dropped > 0,
        "the queue must overflow: {delivered} lines written"
    );
    assert_eq!(
        delivered + dropped,
        TOTAL,
        "each line is written or counted"
    );
}

/// A sink that never returns from a write, as a wedged pipe does, until
/// `stop` says the test is done.
struct Wedged {
    stop: Arc<std::sync::atomic::AtomicBool>,
}

impl Write for Wedged {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        while !self.stop.load(std::sync::atomic::Ordering::SeqCst) {
            std::thread::park_timeout(std::time::Duration::from_millis(5));
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn a_wedged_sink_costs_the_bound_never_a_hang() {
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let kernel = Registry::new().finish(
        Logger::new(
            LogFilter::default(),
            Format::Json,
            Box::new(Wedged {
                stop: Arc::clone(&stop),
            }),
            fixed,
        ),
        Arc::new(crate::ThreadRuntime::new()),
    );
    let ctx = Ctx::root(&kernel);
    // The one queued line wedges the writer thread inside the sink mutex.
    log!(ctx, info, GREETED);
    let started = Instant::now();
    let completed = kernel.logger().flush();
    let flushed_in = started.elapsed();
    assert!(!completed, "a wedged sink never catches up");
    // The bound is 2 s; allow generous slack for a loaded machine.
    assert!(
        flushed_in < Duration::from_secs(10),
        "flush hung on the wedged sink: {flushed_in:?}"
    );
    // `Drop` joins the writer for at most the same bound, then detaches it.
    let started = Instant::now();
    drop(kernel);
    let dropped_in = started.elapsed();
    assert!(
        dropped_in < Duration::from_secs(10),
        "dropping the logger hung on the wedged writer: {dropped_in:?}"
    );
    // Release the detached thread so the test binary can exit cleanly.
    stop.store(true, std::sync::atomic::Ordering::SeqCst);
}

#[test]
fn warn_and_error_are_in_the_sink_without_a_flush() {
    let sink = Shared::default();
    let kernel = kernel(sink.clone(), Format::Json, "info");
    let ctx = Ctx::root(&kernel);
    log!(ctx, warn, GREETED);
    log!(ctx, error, GREETED);
    // Queued lines need a flush; inline urgent lines are already there.
    let text = sink.text();
    assert_eq!(
        text.lines().count(),
        2,
        "both urgent lines are written: {text}"
    );
    assert!(
        text.contains(r#""level":"warn""#) && text.contains(r#""level":"error""#),
        "{text}"
    );
}

#[test]
fn the_dropped_lines_report_bypasses_the_module_filter() {
    // `kernel` is filtered to `error`: a warn-level `kernel.log_dropped`
    // line must still reach the sink, because the report is not written
    // through the filter.
    const TOTAL: usize = 5_000;
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let sink = Shared::default();
    let filter = LogFilter::parse("error,hello=info").expect("test filter parses");
    let kernel = Registry::new().finish(
        Logger::new(
            filter,
            Format::Json,
            Box::new(Gated {
                gate: Arc::clone(&gate),
                out: sink.clone(),
            }),
            fixed,
        ),
        Arc::new(crate::ThreadRuntime::new()),
    );
    let ctx = Ctx::root(&kernel);
    for _ in 0..TOTAL {
        log!(ctx, info, GREETED);
    }
    let (open, wake) = &*gate;
    *open.lock().unwrap_or_else(PoisonError::into_inner) = true;
    wake.notify_all();
    assert!(kernel.logger().flush(), "the sink caught up once open");
    let reports = sink.lines_for(crate::log_events::LOG_DROPPED.as_str());
    assert_eq!(
        reports.len(),
        1,
        "one dropped-lines report: {}",
        sink.text()
    );
    let report = reports.first().expect("one report line");
    assert!(
        report.contains(r#""level":"warn""#),
        "the report is a warn line: {report}"
    );
    let count: usize = crate::testing::json_field(report, "count")
        .expect("a count field")
        .parse()
        .expect("a numeric count");
    assert!(count > 0, "the overflow dropped lines: {report}");
}
