//! Ports: narrow traits one module implements and others may consume.

use crate::failure::Fallible;
use crate::failures::KernelFailure;
use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::sync::Arc;

/// An interface one module provides and any number of modules may consume.
pub trait Port: Send + Sync + 'static {}

/// The table of provided port implementations; owned by the kernel.
#[derive(Default)]
pub struct Ports {
    // Each entry is an `Arc<P>` for the `P` matching the key.
    entries: HashMap<TypeId, Box<dyn Any + Send + Sync>>,
}

impl Ports {
    /// The implementation of `P`, or `None` when its module is not built in.
    /// Consumers must tolerate `None`; that is what keeps modules removable.
    pub fn get<P: ?Sized + Port>(&self) -> Option<Arc<P>> {
        self.entries
            .get(&TypeId::of::<P>())
            .and_then(|boxed| boxed.downcast_ref::<Arc<P>>())
            .cloned()
    }

    pub(crate) fn insert<P: ?Sized + Port>(&mut self, implementation: Arc<P>) -> Fallible {
        match self.entries.entry(TypeId::of::<P>()) {
            // `Arc<P>` behind `Box<dyn Any>`: an existing entry means a second provider.
            Entry::Occupied(_) => Err(Box::new(KernelFailure::PortTaken {
                port: std::any::type_name::<P>(),
            })),
            Entry::Vacant(slot) => {
                slot.insert(Box::new(implementation));
                Ok(())
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::log::log_tests::{Shared, fixed};
    use crate::{Ctx, Format, LogFilter, Logger, Registry};
    use std::sync::{Arc, Mutex};

    pub(crate) trait Recorder: Port {
        fn push(&self, entry: &'static str);
    }

    #[derive(Default)]
    pub(crate) struct RecorderImpl(Mutex<Vec<&'static str>>);

    impl RecorderImpl {
        pub(crate) fn entries(&self) -> Vec<&'static str> {
            self.0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        }
    }

    impl Recorder for RecorderImpl {
        fn push(&self, entry: &'static str) {
            let mut entries = self
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            entries.push(entry);
        }
    }

    impl Port for RecorderImpl {}

    fn test_logger(sink: Shared) -> Logger {
        Logger::new(LogFilter::default(), Format::Json, Box::new(sink), fixed)
    }

    pub(crate) fn test_runtime() -> Arc<crate::ThreadRuntime> {
        Arc::new(crate::ThreadRuntime::new())
    }

    #[test]
    fn an_unprovided_port_is_absent() {
        let kernel = Registry::new().finish(test_logger(Shared::default()), test_runtime());
        let ctx = Ctx::root(&kernel);
        assert!(ctx.ports().get::<dyn Recorder>().is_none());
    }

    #[test]
    fn a_provided_port_comes_back_as_the_same_arc() {
        let recorder: Arc<dyn Recorder> = Arc::new(RecorderImpl::default());
        let mut registry = Registry::new();
        registry
            .provide::<dyn Recorder>(Arc::clone(&recorder))
            .expect("first provider");
        let kernel = registry.finish(test_logger(Shared::default()), test_runtime());
        let ctx = Ctx::root(&kernel);
        let got = ctx.ports().get::<dyn Recorder>().expect("provided port");
        assert!(Arc::ptr_eq(&got, &recorder));
    }

    #[test]
    fn a_second_provider_of_the_same_port_fails_with_port_taken() {
        let recorder: Arc<dyn Recorder> = Arc::new(RecorderImpl::default());
        let mut registry = Registry::new();
        registry
            .provide::<dyn Recorder>(Arc::clone(&recorder))
            .expect("first provider");
        let error = registry
            .provide::<dyn Recorder>(recorder)
            .expect_err("a second provider must fail");
        assert_eq!(error.code(), crate::failures::PORT_TAKEN);
    }
}
