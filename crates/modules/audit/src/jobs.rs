//! The two background jobs: writing what has queued up, and dropping what
//! is older than the retention window.

use crate::config::shared_db;
use crate::log_events::{DRAIN_FAILED, DROPPED, PRUNED, RETENTION_FAILED, WRITTEN};
use crate::store;
use std::time::Duration;
use timbre_kernel::clock::{DAY_MILLIS, now_unix_millis};
use timbre_kernel::{BoxFuture, Ctx, Fallible, log, run_periodic};

/// How often rows past the retention window are looked for. Daily is what
/// the window is measured in; a restart runs one pass at once.
const PRUNE_EVERY: Duration = Duration::from_hours(24);

/// Write everything queued, and report what did not fit while it queued.
pub(crate) fn drain(ctx: &Ctx) -> Fallible {
    let audit = crate::state(ctx)?;
    let queue = audit.queue();
    let capacity = crate::settings(audit.as_ref())?.queue_capacity;
    let _writer = queue.lock_writer();
    let (batch, dropped) = queue.take();
    if dropped > 0 {
        ctx.log_security(&DROPPED, &[("events", &dropped)]);
    }
    if batch.rows().is_empty() {
        return Ok(());
    }
    let written = (|| {
        let shared = shared_db(ctx)?;
        shared
            .get(ctx)?
            .write(|sql| store::append(sql, batch.rows()))
    })();
    if let Err(failure) = written {
        queue.restore(batch, capacity);
        return Err(failure);
    }
    log!(ctx, debug, WRITTEN, events = batch.rows().len());
    Ok(())
}

/// Delete journal rows older than the retention window.
pub(crate) fn prune(ctx: &Ctx) -> Fallible {
    let audit = crate::state(ctx)?;
    let settings = crate::settings(audit.as_ref())?;
    let cutoff = now_unix_millis().saturating_sub(i64::from(settings.keep_days) * DAY_MILLIS);
    let shared = shared_db(ctx)?;
    let database = shared.get(ctx)?;
    let deleted = database.write(|sql| store::prune(sql, cutoff))?;
    if deleted > 0 {
        log!(
            ctx,
            info,
            PRUNED,
            events = deleted,
            keep_days = settings.keep_days
        );
    }
    Ok(())
}

pub(crate) fn draining(ctx: Ctx) -> BoxFuture<Fallible> {
    Box::pin(async move {
        let audit = crate::state(&ctx)?;
        let every = crate::settings(audit.as_ref())?.drain_every();
        run_periodic(&ctx, Duration::ZERO, every, DRAIN_FAILED, drain).await
    })
}

pub(crate) fn pruning(ctx: Ctx) -> BoxFuture<Fallible> {
    Box::pin(async move {
        run_periodic(&ctx, Duration::ZERO, PRUNE_EVERY, RETENTION_FAILED, prune).await
    })
}
