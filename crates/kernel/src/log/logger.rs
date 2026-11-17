//! The logger: filter, format and clock, in front of a queue that feeds the sink.

use crate::ctx::RequestId;
use crate::log::writer::{Queue, Sink, render, write_line};
use crate::log::{Level, LogEvent, LogFilter, LogValue};
use std::io::{IsTerminal, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

/// A source of timestamps; tests pass a fixed one.
pub type Clock = fn() -> SystemTime;

/// How long an inline `warn`/`error` waits for the sink mutex before its line
/// counts as dropped: a wedged sink never blocks the caller past this.
const INLINE_SINK_WAIT: Duration = Duration::from_millis(50);

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
    sink: Arc<Sink>,
    /// Lines counted away: a full queue, or an inline line whose sink mutex
    /// stayed locked past the 50 ms inline bound. Shared with the writer
    /// thread so both feed one `kernel.log_dropped` report.
    dropped: Arc<AtomicU64>,
    /// `None` only when the OS refused the writer thread; lines are then written inline.
    queue: Option<Queue>,
}

impl Logger {
    /// Build a logger over an arbitrary sink. Queued lines reach the sink from a
    /// thread this logger starts, so call [`Logger::flush`] before reading what it
    /// has written; `warn`/`error` lines are written by the calling thread and,
    /// when the sink answers within the 50 ms inline bound, are in it when
    /// `write` returns.
    pub fn new(
        filter: LogFilter,
        format: Format,
        sink: Box<dyn Write + Send>,
        clock: Clock,
    ) -> Self {
        let sink: Arc<Sink> = Arc::new(Mutex::new(sink));
        let dropped = Arc::new(AtomicU64::new(0));
        let queue = Queue::start(Arc::clone(&sink), format, clock, Arc::clone(&dropped));
        Self {
            filter,
            format,
            clock,
            sink,
            dropped,
            queue,
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

    /// Render one line, when its level passes the module's threshold.
    ///
    /// `warn` and `error` lines are written inline, under the sink mutex, so
    /// they reach the sink ahead of everything queued; the wait for the mutex
    /// is bounded at 50 ms and a line that finds the sink still locked is
    /// dropped and counted like a queued overflow. Everything else goes
    /// through the bounded queue: a full queue drops the line and counts it,
    /// so the caller never waits on the sink.
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
        let line = render(self.format, (self.clock)(), level, event, request, fields);
        if level <= Level::Warn {
            if !write_line_bounded(&self.sink, &line, INLINE_SINK_WAIT) {
                self.dropped.fetch_add(1, Ordering::Relaxed);
            }
            return;
        }
        match &self.queue {
            Some(queue) => queue.push(line),
            None => write_line(&self.sink, &line),
        }
    }

    /// Report any lines dropped since the last call (bypassing the module
    /// filter, at `warn`, even when `kernel` is filtered above that), then block
    /// until every queued line has reached the sink. Returns whether the sink
    /// caught up within the wait bound; a wedged sink costs a caller the bound,
    /// never a hang.
    pub fn flush(&self) -> bool {
        match &self.queue {
            Some(queue) => queue.flush(),
            None => true,
        }
    }
}

/// Write one inline line, waiting at most `bound` for the sink mutex with a
/// growing sleep between attempts. `false` when the mutex stayed locked past
/// the bound: the line was not written and the caller counts it dropped.
/// Logging never fails the caller, so sink errors are dropped like everywhere
/// else. A poisoned mutex is taken anyway, as in `write_line`.
fn write_line_bounded(sink: &Sink, line: &str, bound: Duration) -> bool {
    let deadline = Instant::now() + bound;
    let mut delay = Duration::from_micros(50);
    let mut sink = loop {
        match sink.try_lock() {
            Ok(guard) => break guard,
            Err(std::sync::TryLockError::Poisoned(error)) => break error.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) => {
                let now = Instant::now();
                if now >= deadline {
                    return false;
                }
                let remaining = deadline - now;
                std::thread::sleep(delay.min(remaining).min(Duration::from_millis(5)));
                delay = delay.saturating_mul(2);
            }
        }
    };
    let _ = sink.write_all(line.as_bytes()).and_then(|()| sink.flush());
    true
}
