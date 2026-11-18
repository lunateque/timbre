//! Sink refusals: a `Write` that returns an error (a full disk, a closed
//! pipe) must cost lines, never silently look delivered: every refused line
//! lands in the dropped bucket its watcher reads, and an audit refusal in
//! the audit one.

use super::*;
use crate::Level;
use crate::log_events::{AUDIT_DROPPED, AUTH_FAILED, LOG_DROPPED};

/// A sink that refuses its first `broken` writes, as a disk that is briefly
/// full and then clears.
#[derive(Clone)]
pub(super) struct Flaky {
    broken: Arc<std::sync::atomic::AtomicUsize>,
    out: Shared,
}

impl Flaky {
    fn new(broken: usize, out: Shared) -> Flaky {
        Flaky {
            broken: Arc::new(std::sync::atomic::AtomicUsize::new(broken)),
            out,
        }
    }
}

impl Write for Flaky {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        use std::sync::atomic::Ordering;
        // Only the write is the disk-full path; the flush always succeeds,
        // so a test failure cannot come from the half of a write pair.
        if self.broken.load(Ordering::Relaxed) == 0 {
            return self.out.write(buf);
        }
        self.broken.fetch_sub(1, Ordering::Relaxed);
        Err(std::io::Error::new(
            std::io::ErrorKind::StorageFull,
            "the disk is gone",
        ))
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Every `kernel.log_dropped` count written so far, summed: the writer may
/// report in several lines, and what must hold is the total.
fn dropped_total(sink: &Shared, event: &crate::LogEvent) -> usize {
    sink.lines_for(event.as_str())
        .iter()
        .map(|report| {
            crate::testing::json_field(report, "count")
                .expect("a count field")
                .parse::<usize>()
                .expect("a numeric count")
        })
        .sum()
}

#[test]
fn a_sink_that_refuses_queued_lines_counts_them_as_dropped() {
    let seen = Shared::default();
    let kernel = Registry::new().finish(
        Logger::new(
            LogFilter::default(),
            Format::Json,
            // The logger takes the sink by value; the test's clone shares one
            // refusal counter and one buffer with it.
            Box::new(Flaky::new(2, seen.clone())),
            fixed,
        ),
        Arc::new(crate::ThreadRuntime::new()),
    );
    let ctx = Ctx::root(&kernel);
    log!(ctx, info, GREETED);
    log!(ctx, info, GREETED);
    // The disk clears (two refusals spent): the report the writer emits
    // after them must reach the sink.
    assert!(kernel.logger().flush(), "the sink caught up");
    assert_eq!(
        dropped_total(&seen, &LOG_DROPPED),
        2,
        "both refused lines are counted: {}",
        seen.text()
    );
    assert!(
        !seen.text().contains(r#""event":"hello.greeted""#),
        "the refused lines are not in the log: {}",
        seen.text()
    );
}

#[test]
fn a_sink_that_refuses_an_inline_line_counts_it_as_dropped() {
    // warn/error are written by the calling thread, not the queue; a refused
    // line must count in the same report.
    let seen = Shared::default();
    let kernel = Registry::new().finish(
        Logger::new(
            LogFilter::default(),
            Format::Json,
            Box::new(Flaky::new(1, seen.clone())),
            fixed,
        ),
        Arc::new(crate::ThreadRuntime::new()),
    );
    let ctx = Ctx::root(&kernel);
    log!(ctx, warn, GREETED);
    assert!(kernel.logger().flush(), "the sink caught up");
    assert_eq!(
        dropped_total(&seen, &LOG_DROPPED),
        1,
        "the refused warn is counted: {}",
        seen.text()
    );
}

#[test]
fn a_sink_that_refuses_an_audit_line_counts_it_as_audit_dropped() {
    // A refused `write_always` is counted where an audit watcher reads it,
    // not in the generic bucket.
    let seen = Shared::default();
    let kernel = Registry::new().finish(
        Logger::new(
            // The filter hides `kernel` lines; the audit line and its report
            // bypass the filter, so both still reach the sink.
            LogFilter::parse("error,kernel=error").expect("test filter parses"),
            Format::Json,
            Box::new(Flaky::new(1, seen.clone())),
            fixed,
        ),
        Arc::new(crate::ThreadRuntime::new()),
    );
    kernel.logger().write_always(
        None,
        Level::Warn,
        &AUTH_FAILED,
        &[("reason", &"rejected" as &dyn crate::LogValue)],
    );
    assert!(kernel.logger().flush(), "the sink caught up");
    assert_eq!(
        dropped_total(&seen, &AUDIT_DROPPED),
        1,
        "the refused audit line is counted: {}",
        seen.text()
    );
    assert_eq!(
        dropped_total(&seen, &LOG_DROPPED),
        0,
        "the audit bucket stands apart: {}",
        seen.text()
    );
}
