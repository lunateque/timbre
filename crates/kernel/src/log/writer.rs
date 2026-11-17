//! The writer thread: callers queue rendered lines and this thread writes them.
//!
//! While the queue runs, only the writer touches the sink, so lines never
//! interleave. `warn` and `error` are urgent enough to pay for: they are written
//! inline, under the sink mutex, waiting for it at most the logger's 50 ms
//! bound. Only info/debug/trace go through the bounded queue; a line that finds
//! the queue full is dropped and counted, and so is an inline line whose sink
//! mutex stayed locked past the bound. The count is reported at `warn`,
//! bypassing the module filter, on every flush and at shutdown. A stalled sink
//! therefore costs lines, never an unbounded wait: flush and `Drop` hold on to
//! it for at most [`SINK_WAIT`].

use crate::ctx::RequestId;
use crate::log::format;
use crate::log::logger::{Clock, Format};
use crate::log::{Level, LogEvent, LogValue};
use crate::log_events::LOG_DROPPED;
use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant, SystemTime};

/// Lines the writer may fall behind by before new lines are dropped.
const QUEUE_CAPACITY: usize = 4096;

/// How long the writer waits for a line before it counts as caught up.
const CAUGHT_UP_AFTER: Duration = Duration::from_millis(100);

/// The most any caller ever waits for the sink: flush, and `Drop` of the writer.
const SINK_WAIT: Duration = Duration::from_secs(2);

/// The output every line goes to.
pub(super) type Sink = Mutex<Box<dyn Write + Send>>;

/// What the writer thread receives.
enum Message {
    /// One rendered line, newline included.
    Line(String),
    /// Acknowledged once every line queued before it has been written.
    Flush(SyncSender<()>),
}

/// The pieces one dropped-lines report needs; shared by the writer thread and
/// by any thread that flushes, so a wedged writer cannot postpone the report.
#[derive(Clone)]
struct Reporter {
    sink: Arc<Sink>,
    dropped: Arc<AtomicU64>,
    format: Format,
    clock: Clock,
}

impl Reporter {
    /// Report when the sink mutex is free. A sink held by a wedged write
    /// cannot accept anything anyway, so the count stays owed to a later
    /// report instead of hanging the caller past its bound.
    fn report(&self) {
        let mut sink = match self.sink.try_lock() {
            Ok(sink) => sink,
            Err(std::sync::TryLockError::WouldBlock) => return,
            Err(std::sync::TryLockError::Poisoned(error)) => error.into_inner(),
        };
        self.write_report(&mut sink);
    }

    /// Report waiting for the sink mutex: the last chance at shutdown.
    fn report_blocking(&self) {
        let mut sink = lock_sink(&self.sink);
        self.write_report(&mut sink);
    }

    /// One `kernel.log_dropped` line at `warn` for the lines dropped since the
    /// last report, written with the sink already held.
    fn write_report(&self, sink: &mut Box<dyn Write + Send>) {
        let count = self.dropped.swap(0, Ordering::Relaxed);
        if count == 0 {
            return;
        }
        let fields: [(&'static str, &dyn LogValue); 1] = [("count", &count)];
        let line = render(
            self.format,
            (self.clock)(),
            Level::Warn,
            &LOG_DROPPED,
            None,
            &fields,
        );
        let _ = sink.write_all(line.as_bytes()).and_then(|()| sink.flush());
    }
}

/// The queue in front of the sink and the thread that drains it.
pub(super) struct Queue {
    /// Taken on drop, which closes the queue so that the thread can finish.
    sender: Option<SyncSender<Message>>,
    reporter: Reporter,
    /// Set when the writer thread has exited; the bounded stand-in for a timed
    /// join, which std does not offer, and `Sync` so the kernel stays `Sync`.
    finished: Arc<(Mutex<bool>, Condvar)>,
}

impl Queue {
    /// Start the writer thread. `None` when the OS refuses one: the caller then writes
    /// each line itself, so none is lost. `dropped` is the logger's shared count:
    /// overflowed inline and queued lines land in one report.
    pub(super) fn start(
        sink: Arc<Sink>,
        format: Format,
        clock: Clock,
        dropped: Arc<AtomicU64>,
    ) -> Option<Self> {
        let (sender, inbox) = mpsc::sync_channel(QUEUE_CAPACITY);
        let reporter = Reporter {
            sink,
            dropped,
            format,
            clock,
        };
        let writer = Writer {
            inbox,
            reporter: reporter.clone(),
        };
        let finished = Arc::new((Mutex::new(false), Condvar::new()));
        let exiting = Arc::clone(&finished);
        std::thread::Builder::new()
            .name("timbre-log".to_owned())
            .spawn(move || {
                writer.run();
                let (done, wake) = &*exiting;
                *done.lock().unwrap_or_else(PoisonError::into_inner) = true;
                wake.notify_all();
            })
            .ok()?;
        Some(Self {
            sender: Some(sender),
            reporter,
            finished,
        })
    }

    /// Queue one rendered line. A full queue, or a writer that has died, drops
    /// the line and counts it: the caller never waits on the sink.
    pub(super) fn push(&self, line: String) {
        let sent = self
            .sender
            .as_ref()
            .is_some_and(|sender| sender.try_send(Message::Line(line)).is_ok());
        if !sent {
            self.reporter.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Report lines dropped since the last call, then block until every line
    /// queued so far has reached the sink. Returns whether the sink caught up
    /// within [`SINK_WAIT`]; a wedged sink costs this caller the bound, not a
    /// hang, and its remaining lines stay queued for a later flush.
    pub(super) fn flush(&self) -> bool {
        self.reporter.report();
        let Some(sender) = &self.sender else {
            return true;
        };
        let deadline = Instant::now() + SINK_WAIT;
        let (ack, done) = mpsc::sync_channel(1);
        // A writer wedged on the sink cannot read the queue: sending is bounded
        // too, so a full queue never turns the flush into an unbounded wait.
        loop {
            match sender.try_send(Message::Flush(ack.clone())) {
                Ok(()) => break,
                Err(mpsc::TrySendError::Full(retry)) => {
                    drop(retry);
                    if Instant::now() >= deadline {
                        return false;
                    }
                    std::thread::sleep(Duration::from_millis(1));
                }
                // A writer that has stopped has nothing left to wait for.
                Err(mpsc::TrySendError::Disconnected(_)) => return true,
            }
        }
        // The sent clone is the channel's only other sender: dropping this one
        // means a writer that dies holding it disconnects the wait below.
        drop(ack);
        done.recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .is_ok()
    }
}

impl Drop for Queue {
    fn drop(&mut self) {
        // Closing the queue lets the writer drain what is left, report drops and
        // exit. Waiting keeps the last lines from being lost when the process
        // ends right after; a writer wedged on the sink past the bound is left
        // detached (the thread finishes on its own once the sink returns)
        // instead of hanging the thread that drops the logger.
        drop(self.sender.take());
        let (done, wake) = &*self.finished;
        let mut done = done.lock().unwrap_or_else(PoisonError::into_inner);
        while !*done {
            let (state, wait) = wake
                .wait_timeout(done, SINK_WAIT)
                .unwrap_or_else(PoisonError::into_inner);
            done = state;
            if wait.timed_out() {
                break;
            }
        }
    }
}

/// The writer thread's state.
struct Writer {
    inbox: Receiver<Message>,
    reporter: Reporter,
}

impl Writer {
    fn run(&self) {
        loop {
            match self.inbox.recv_timeout(CAUGHT_UP_AFTER) {
                Ok(Message::Line(line)) => write_line(&self.reporter.sink, &line),
                Ok(Message::Flush(ack)) => {
                    self.reporter.report();
                    // The flusher may have stopped waiting, which leaves nothing owed.
                    let _ = ack.send(());
                }
                Err(RecvTimeoutError::Timeout) => self.reporter.report(),
                Err(RecvTimeoutError::Disconnected) => {
                    // Drain what is already queued before the final report, so a
                    // shutdown flush's lines count as delivered, not dropped.
                    while let Ok(Message::Line(line)) = self.inbox.try_recv() {
                        write_line(&self.reporter.sink, &line);
                    }
                    self.reporter.report_blocking();
                    return;
                }
            }
        }
    }
}

/// Render one line in the given format.
pub(super) fn render(
    format: Format,
    now: SystemTime,
    level: Level,
    event: &LogEvent,
    request: Option<RequestId>,
    fields: &[(&'static str, &dyn LogValue)],
) -> String {
    match format {
        Format::Json => format::json(now, level, event, request, fields),
        Format::Pretty => format::pretty(now, level, event, request, fields),
    }
}

/// Lock the sink for writing. A poisoned mutex means an earlier writer panicked
/// while holding it; its `Box<dyn Write>` is intact, so the guard is taken anyway.
fn lock_sink(sink: &Sink) -> MutexGuard<'_, Box<dyn Write + Send>> {
    sink.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Write one line. Logging never fails the caller, so sink errors are dropped.
pub(super) fn write_line(sink: &Sink, line: &str) {
    let mut sink = lock_sink(sink);
    let _ = sink.write_all(line.as_bytes()).and_then(|()| sink.flush());
}
