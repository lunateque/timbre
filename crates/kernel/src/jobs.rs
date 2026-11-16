//! Background jobs: long-lived tasks the server starts after `Started`.

use crate::ctx::Ctx;
use crate::failure::Fallible;
use crate::failures::KernelFailure;
use crate::future::BoxFuture;
use crate::log::LogEvent;
use crate::log_events::{JOB_FAILED, JOB_FINISHED, JOB_STARTED};

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

    /// Start every job on the runtime: each logs its start, its end and, when
    /// the handler fails, the failure code. A job that ends normally is over;
    /// nothing restarts it.
    pub(crate) fn start(&self, ctx: &Ctx) {
        for entry in &self.entries {
            let name = entry.name;
            let handler = entry.handler;
            let job_ctx = ctx.clone();
            job_ctx.clone().spawn(Box::pin(async move {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::failures::KernelFailure;
    use crate::log::log_tests::{Shared, fixed};
    use crate::log_events::{JOB_FAILED, JOB_FINISHED, JOB_STARTED};
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
}
