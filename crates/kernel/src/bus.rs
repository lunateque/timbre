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
/// Not part of the module-facing contract: modules reach signals through
/// `Registry::on` and `Ctx::publish`.
#[derive(Default)]
pub(crate) struct Bus {
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
    /// logged and does not stop the others nor fail the publisher; a handler
    /// that panics is caught the same way, because a rejection path that
    /// unwinds through its publisher (say, every failed login) would turn one
    /// module's bug into a lost connection for the client.
    pub(crate) fn publish<S: Signal>(&self, ctx: &Ctx, signal: &S) {
        let Some(entry) = self.handlers.get(&TypeId::of::<S>()) else {
            return;
        };
        let Some(handlers) = entry.downcast_ref::<Vec<Handler<S>>>() else {
            return;
        };
        for handler in handlers {
            // `AssertUnwindSafe`: the bus only hands out shared borrows, and a
            // handler that panicked mid-way left nothing mutable behind here.
            let outcome =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| handler(ctx, signal)));
            match outcome {
                Ok(Ok(())) => {}
                Ok(Err(failure)) => crate::log!(
                    ctx,
                    warn,
                    HANDLER_FAILED,
                    signal = std::any::type_name::<S>(),
                    code = failure.code().as_str(),
                    panicked = false
                ),
                Err(_) => crate::log!(
                    ctx,
                    warn,
                    HANDLER_FAILED,
                    signal = std::any::type_name::<S>(),
                    // The panic payload may be anything a module threw; the
                    // bus names the handler that ran, which is what is stable.
                    panicked = true
                ),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::failures::KernelFailure;
    use crate::port::tests::{Recorder, RecorderImpl};
    use crate::testing::{Shared, fixed};
    use crate::{Ctx, Format, LogFilter, Logger, Registry, ThreadRuntime};
    use std::sync::Arc;

    struct Ping;
    impl Signal for Ping {}

    struct Panics;
    impl Signal for Panics {}

    #[expect(
        clippy::unnecessary_wraps,
        reason = "handlers must have the `Handler<Panics>` signature"
    )]
    fn panics(_: &Ctx, _: &Panics) -> Fallible {
        // A caller-visible bug: an unchecked slice index past the end, the
        // way a handler with a hidden assumption breaks at runtime.
        let empty: Vec<u8> = Vec::new();
        let _ = empty[0];
        Ok(())
    }

    #[expect(
        clippy::unnecessary_wraps,
        reason = "handlers must have the `Handler<Panics>` signature"
    )]
    fn remembers(ctx: &Ctx, _: &Panics) -> Fallible {
        if let Some(recorder) = ctx.ports().get::<dyn Recorder>() {
            recorder.push("second");
        }
        Ok(())
    }

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
        kernel.logger().flush();
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
    fn a_panicking_handler_costs_only_itself() {
        let sink = Shared::default();
        let recorder = Arc::new(RecorderImpl::default());
        let mut registry = Registry::new();
        registry
            .provide::<dyn Recorder>(Arc::clone(&recorder) as Arc<dyn Recorder>)
            .expect("first provider");
        registry.on::<Panics>(panics);
        registry.on::<Panics>(remembers);
        let kernel = registry.finish(
            Logger::new(
                LogFilter::default(),
                Format::Json,
                Box::new(sink.clone()),
                fixed,
            ),
            Arc::new(ThreadRuntime::new()),
        );
        // The caller survives: `publish` is synchronous and unwound nothing.
        Ctx::root(&kernel).publish(&Panics);
        kernel.logger().flush();
        assert_eq!(
            RecorderImpl::entries(&recorder),
            ["second"],
            "the handler after the panic still ran"
        );
        let reports = sink.lines_for(HANDLER_FAILED.as_str());
        assert_eq!(reports.len(), 1, "one line names the failed handler");
        let line = reports.first().expect("one line");
        assert!(
            line.contains(r#""panicked":true"#),
            "the line says the handler panicked: {line}"
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
        kernel.logger().flush();
        assert_eq!(sink.text(), "");
    }
}
