//! Bounded summaries keep an already-refused flood off the audit hot path.

use crate::log_events::{AUTH_THROTTLED, AUTH_THROTTLED_UNTRACKED};
use crate::{BoxFuture, Ctx, Fallible};
use std::collections::HashMap;
use std::future::poll_fn;
use std::net::IpAddr;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Mutex, PoisonError};
use std::task::Poll;
use std::time::Duration;

const INTERVAL: Duration = Duration::from_secs(60);
const MAX_CLIENTS: usize = 4096;

#[derive(Default)]
struct Counts {
    clients: HashMap<IpAddr, u64>,
    untracked: u64,
}

#[derive(Default)]
pub(crate) struct Throttled(Mutex<Counts>);

impl Throttled {
    pub(crate) fn record(&self, client: IpAddr) {
        let mut counts = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let count = if counts.clients.len() < MAX_CLIENTS {
            counts.clients.entry(client).or_default()
        } else if let Some(count) = counts.clients.get_mut(&client) {
            count
        } else {
            &mut counts.untracked
        };
        *count = count.saturating_add(1);
    }

    fn report(&self, ctx: &Ctx) {
        let Counts { clients, untracked } = {
            let mut counts = self.0.lock().unwrap_or_else(PoisonError::into_inner);
            std::mem::take(&mut *counts)
        };
        for (client, count) in clients {
            ctx.log_security(
                &AUTH_THROTTLED,
                &[("client", &client.to_string()), ("count", &count)],
            );
        }
        if untracked > 0 {
            ctx.log_security(&AUTH_THROTTLED_UNTRACKED, &[("count", &untracked)]);
        }
    }
}

fn report(ctx: &Ctx) {
    ctx.kernel().throttled.report(ctx);
    if let Some(limiter) = ctx.kernel().throttle() {
        limiter.report(ctx);
    }
}

pub(crate) fn run(ctx: Ctx) -> BoxFuture<Fallible> {
    Box::pin(async move {
        let (cancel, tick) = mpsc::channel::<()>();
        let mut cancel = Some(cancel);
        let worker = ctx.clone();
        let mut blocking = ctx.blocking(move || {
            while tick.recv_timeout(INTERVAL) == Err(RecvTimeoutError::Timeout) {
                report(&worker);
            }
            report(&worker);
        });
        let mut shutdown = ctx.shutdown();
        poll_fn(|task| {
            if cancel.is_some() && shutdown.as_mut().poll(task).is_ready() {
                drop(cancel.take());
            }
            match blocking.as_mut().poll(task) {
                Poll::Ready(result) => Poll::Ready(result),
                Poll::Pending => Poll::Pending,
            }
        })
        .await
    })
}
