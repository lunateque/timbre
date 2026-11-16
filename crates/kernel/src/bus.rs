//! The signal bus: registration-ordered synchronous delivery, failures isolated.

use crate::ctx::Ctx;
use crate::failure::Fallible;
use crate::log_events::HANDLER_FAILED;
use std::any::{Any, TypeId};
use std::collections::HashMap;

/// A fact something happened, defined in the kernel and published to anyone listening.
pub trait Signal: Send + Sync + 'static {}

/// The shape of a signal handler: a plain function pointer that reaches any
/// state it needs through `Ctx`, so registration cannot hide shared state.
pub type Handler<S> = fn(&Ctx, &S) -> Fallible;

/// Delivers signals to handlers in registration order; owned by the kernel.
#[derive(Default)]
pub struct Bus {
    // Each entry is a `Vec<Handler<S>>` for the `S` matching the key; the type
    // id guarantees the downcast always succeeds.
    handlers: HashMap<TypeId, Box<dyn Any + Send + Sync>>,
}

impl Bus {
    pub(crate) fn add<S: Signal>(&mut self, handler: Handler<S>) {
        let entry = self
            .handlers
            .entry(TypeId::of::<S>())
            .or_insert_with(|| Box::new(Vec::<Handler<S>>::new()));
        if let Some(handlers) = entry.downcast_mut::<Vec<Handler<S>>>() {
            handlers.push(handler);
        }
    }

    /// Call every handler of `S` in registration order. A failing handler is
    /// logged and does not stop the others nor fail the publisher.
    pub(crate) fn publish<S: Signal>(&self, ctx: &Ctx, signal: &S) {
        let Some(entry) = self.handlers.get(&TypeId::of::<S>()) else {
            return;
        };
        let Some(handlers) = entry.downcast_ref::<Vec<Handler<S>>>() else {
            return;
        };
        for handler in handlers {
            if let Err(failure) = handler(ctx, signal) {
                crate::log!(
                    ctx,
                    warn,
                    HANDLER_FAILED,
                    signal = std::any::type_name::<S>(),
                    code = failure.code().as_str()
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::failures::KernelFailure;
    use crate::log::log_tests::{Shared, fixed};
    use crate::port::tests::{Recorder, RecorderImpl};
    use crate::{Ctx, Format, LogFilter, Logger, Registry, ThreadRuntime};
    use std::sync::Arc;

    struct Ping;
    impl Signal for Ping {}

    fn first(ctx: &Ctx, _: &Ping) -> Fallible {
        if let Some(recorder) = ctx.ports().get::<dyn Recorder>() {
            recorder.push("first");
        }
        Err(Box::new(KernelFailure::LogFilterInvalid))
    }

    #[expect(
        clippy::unnecessary_wraps,
        reason = "handlers must have the `Handler<Ping>` signature"
    )]
    fn second(ctx: &Ctx, _: &Ping) -> Fallible {
        if let Some(recorder) = ctx.ports().get::<dyn Recorder>() {
            recorder.push("second");
        }
        Ok(())
    }

    #[test]
    fn handlers_run_in_order_and_a_failure_only_costs_its_own_line() {
        let sink = Shared::default();
        let recorder = Arc::new(RecorderImpl::default());
        let mut registry = Registry::new();
        registry
            .provide::<dyn Recorder>(Arc::clone(&recorder) as Arc<dyn Recorder>)
            .expect("first provider");
        registry.on::<Ping>(first);
        registry.on::<Ping>(second);
        let kernel = registry.finish(
            Logger::new(
                LogFilter::default(),
                Format::Json,
                Box::new(sink.clone()),
                fixed,
            ),
            Arc::new(ThreadRuntime::new()),
        );
        Ctx::root(&kernel).publish(&Ping);
        assert_eq!(RecorderImpl::entries(&recorder), ["first", "second"]);
        let text = sink.text();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 1, "one warning for the failed handler: {text}");
        let line = lines.first().expect("one line");
        assert!(
            line.contains(r#""event":"kernel.handler_failed""#),
            "got {line}"
        );
        assert!(
            line.contains(r#""code":"kernel.log_filter_invalid""#),
            "got {line}"
        );
    }

    #[test]
    fn a_signal_without_handlers_does_nothing() {
        struct Quiet;
        impl Signal for Quiet {}
        let sink = Shared::default();
        let kernel = Registry::new().finish(
            Logger::new(
                LogFilter::default(),
                Format::Json,
                Box::new(sink.clone()),
                fixed,
            ),
            Arc::new(ThreadRuntime::new()),
        );
        Ctx::root(&kernel).publish(&Quiet);
        assert_eq!(sink.text(), "");
    }
}
