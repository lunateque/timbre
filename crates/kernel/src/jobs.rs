//! Background jobs: long-lived tasks the server starts after `Started`.

use crate::ctx::Ctx;
use crate::failure::Fallible;
use crate::failures::KernelFailure;
use crate::future::BoxFuture;
use crate::log::LogEvent;
use crate::log_events::{JOB_FAILED, JOB_FINISHED, JOB_STARTED};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant};

/// The handler shape of a job: runs until the future it returns completes.
pub type JobHandler = fn(Ctx) -> BoxFuture<Fallible>;

struct Entry {
    name: LogEvent,
    handler: JobHandler,
}

/// The job table; owned by the kernel, filled by `Registry::job`.
#[derive(Default)]
pub(crate) struct JobTable {
    entries: Vec<Entry>,
}

impl JobTable {
    pub(crate) fn add(&mut self, name: LogEvent, handler: JobHandler) -> Fallible {
        if self.entries.iter().any(|entry| entry.name == name) {
            return Err(Box::new(KernelFailure::JobTaken {
                name: name.as_str(),
            }));
        }
        self.entries.push(Entry { name, handler });
        Ok(())
    }

    /// Start every job on the runtime, counting it in `tracker` until its
    /// future is over: each logs its start, its end and, when the handler
    /// fails, the failure code. A job that ends normally is over; nothing
    /// restarts it.
    pub(crate) fn start(&self, ctx: &Ctx, tracker: &JobTracker) {
        for entry in &self.entries {
            let name = entry.name;
            let handler = entry.handler;
            let job_ctx = ctx.clone();
            let running = tracker.begin();
            job_ctx.clone().spawn(Box::pin(async move {
                // Held until the job's future is over, even on unwind: the
                // guard's drop is what tells `JobTracker::wait` this job ended.
                let _running = running;
                let ctx = job_ctx;
                crate::log!(ctx, info, JOB_STARTED, job = name.as_str());
                match (handler)(ctx.clone()).await {
                    Ok(()) => crate::log!(ctx, info, JOB_FINISHED, job = name.as_str()),
                    Err(failure) => crate::log!(
                        ctx,
                        error,
                        JOB_FAILED,
                        job = name.as_str(),
                        code = failure.code().as_str()
                    ),
                }
            }));
        }
    }
}

/// How many started jobs are still running, plus the waiters on that count.
/// A never-polled future drops its guard like any other ending, so the count
/// only counts jobs that have not reached an end yet.
#[derive(Default)]
pub(crate) struct JobTracker {
    inner: Arc<TrackerInner>,
}

#[derive(Default)]
struct TrackerInner {
    running: AtomicUsize,
    idle: Mutex<()>,
    wake: Condvar,
}

/// One running job; releases the count when dropped.
pub(crate) struct RunningJob(Arc<TrackerInner>);

impl Drop for RunningJob {
    fn drop(&mut self) {
        if self.0.running.fetch_sub(1, Ordering::AcqRel) > 1 {
            return;
        }
        // The last job ended: wake every `wait` caller. Locking first makes a
        // lost wake impossible: a waiter either sees the zero count before it
        // parks or parks and is woken here.
        let _guard = self.0.idle.lock().unwrap_or_else(PoisonError::into_inner);
        self.0.wake.notify_all();
    }
}

impl JobTracker {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    fn begin(&self) -> RunningJob {
        self.inner.running.fetch_add(1, Ordering::AcqRel);
        RunningJob(Arc::clone(&self.inner))
    }

    /// Whether every started job finished within `timeout`.
    pub(crate) fn wait(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut guard = self
            .inner
            .idle
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        while self.inner.running.load(Ordering::Acquire) > 0 {
            let now = Instant::now();
            if now >= deadline {
                return false;
            }
            let (next, wait) = self
                .inner
                .wake
                .wait_timeout(guard, deadline - now)
                .unwrap_or_else(PoisonError::into_inner);
            guard = next;
            if wait.timed_out() && self.inner.running.load(Ordering::Acquire) > 0 {
                return false;
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::failures::KernelFailure;
    use crate::log_events::{JOB_FAILED, JOB_FINISHED, JOB_STARTED};
    use crate::testing::{Shared, fixed};
    use crate::{Ctx, Failure, Format, LogFilter, Logger, Registry, ThreadRuntime};
    use std::sync::Arc;

    const SLOW: LogEvent = LogEvent::new("kernel.slow");
    const BROKEN: LogEvent = LogEvent::new("kernel.broken");

    // The job logs its own line through the test sink: reading work and
    // assertions share the one per-test sink, so parallel tests cannot see
    // each other's records.
    fn slow(ctx: Ctx) -> BoxFuture<Fallible> {
        Box::pin(async move {
            std::thread::sleep(std::time::Duration::from_millis(30));
            crate::log!(ctx, info, SLOW);
            Ok(())
        })
    }

    fn broken(_: Ctx) -> BoxFuture<Fallible> {
        Box::pin(async move { Err(Box::new(KernelFailure::LogFilterInvalid) as Box<dyn Failure>) })
    }

    // `start_jobs` runs each job on the runtime's threads; tests wait on the
    // sink lines they must produce instead of sleeping blindly.
    fn wait_for(sink: &Shared, needle: &str) -> String {
        for _ in 0..400 {
            let text = sink.text();
            if text.contains(needle) {
                return text;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("the log never received {needle:?}: {}", sink.text());
    }

    #[test]
    fn a_failing_job_logs_kernel_job_failed_with_its_code() {
        let sink = Shared::default();
        let mut registry = Registry::new();
        registry.job(BROKEN, broken).expect("one job");
        let kernel = registry.finish(
            Logger::new(
                LogFilter::default(),
                Format::Json,
                Box::new(sink.clone()),
                fixed,
            ),
            Arc::new(ThreadRuntime::new()),
        );
        let ctx = Ctx::root(&kernel);
        kernel.start_jobs(&ctx);
        let text = wait_for(&sink, JOB_FAILED.as_str());
        // The start line and the failure line exist; the failure carries its code.
        let started: Vec<&str> = text
            .lines()
            .filter(|line| line.contains(&format!("\"event\":\"{}\"", JOB_STARTED.as_str())))
            .collect();
        assert_eq!(started.len(), 1, "one start line: {text}");
        let failed = text
            .lines()
            .find(|line| line.contains(&format!("\"event\":\"{}\"", JOB_FAILED.as_str())))
            .expect("a failure line");
        assert!(
            failed.contains(r#""code":"kernel.log_filter_invalid""#)
                && failed.contains(r#""level":"error""#),
            "the failure line carries its code: {failed}"
        );
    }

    #[test]
    fn a_finished_job_logs_its_end_after_the_work_is_done() {
        let sink = Shared::default();
        let mut registry = Registry::new();
        registry.job(SLOW, slow).expect("one job");
        let kernel = registry.finish(
            Logger::new(
                LogFilter::default(),
                Format::Json,
                Box::new(sink.clone()),
                fixed,
            ),
            Arc::new(ThreadRuntime::new()),
        );
        kernel.start_jobs(&Ctx::root(&kernel));
        let text = wait_for(&sink, JOB_FINISHED.as_str());
        let lines: Vec<&str> = text.lines().collect();
        let position_of = |event: LogEvent| {
            let needle = format!("\"event\":\"{}\"", event.as_str());
            lines.iter().position(|line| line.contains(&needle))
        };
        // The work is on the record before its end line can be seen.
        let work = position_of(SLOW).expect("the job logged its own line");
        let end = position_of(JOB_FINISHED).expect("the end line");
        assert!(
            work < end,
            "the finished line appeared before the work was done: {text}"
        );
        // The line names the job by its event.
        assert!(
            text.contains(&format!("\"job\":\"{}\"", SLOW.as_str())),
            "the job is named in its own line: {text}"
        );
    }

    #[test]
    fn a_duplicate_job_name_is_taken() {
        let mut registry = Registry::new();
        registry.job(SLOW, slow).expect("first");
        let error = registry.job(SLOW, slow).expect_err("the same event twice");
        assert_eq!(error.code(), crate::failures::JOB_TAKEN);
    }

    #[test]
    fn wait_jobs_holds_the_caller_until_the_jobs_end() {
        let sink = Shared::default();
        let mut registry = Registry::new();
        registry.job(SLOW, slow).expect("a slow job");
        registry.job(BROKEN, broken).expect("a failing job");
        let kernel = registry.finish(
            Logger::new(
                LogFilter::default(),
                Format::Json,
                Box::new(sink.clone()),
                fixed,
            ),
            Arc::new(ThreadRuntime::new()),
        );
        kernel.start_jobs(&Ctx::root(&kernel));
        // The count is taken before the spawn returns, so a job that is still
        // running is visible with zero patience.
        assert!(
            !kernel.wait_jobs(std::time::Duration::ZERO),
            "a running job must not look finished"
        );
        // The slow job sleeps 30 ms; a generous bound must see both jobs end
        // (a failed job counts as ended, not as still running).
        assert!(
            kernel.wait_jobs(std::time::Duration::from_secs(5)),
            "every job must finish within the bound"
        );
        assert!(
            kernel.wait_jobs(std::time::Duration::ZERO),
            "finished jobs stay finished"
        );
        kernel.logger().flush();
        let text = sink.text();
        assert!(
            text.contains(JOB_FINISHED.as_str()) && text.contains(JOB_FAILED.as_str()),
            "both jobs logged their end before the wait returned: {text}"
        );
    }
}
