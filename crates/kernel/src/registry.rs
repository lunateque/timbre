//! Registration: modules declare their handlers and ports on a `Registry`,
//! which the binary finishes into the shared `Kernel`.

use crate::bus::{Bus, Handler, Signal};
use crate::failure::Fallible;
use crate::log::Logger;
use crate::port::{Port, Ports};
use std::sync::Arc;

/// The finished, read-only service core every `Ctx` points at.
pub struct Kernel {
    logger: Logger,
    bus: Bus,
    ports: Ports,
}

impl Kernel {
    pub(crate) fn logger(&self) -> &Logger {
        &self.logger
    }

    pub(crate) fn bus(&self) -> &Bus {
        &self.bus
    }

    pub(crate) fn ports(&self) -> &Ports {
        &self.ports
    }
}

/// What a module may declare while registering. The binary threads one registry
/// through every module in build order and finishes it once at the end.
#[derive(Default)]
pub struct Registry {
    bus: Bus,
    ports: Ports,
}

impl Registry {
    /// An empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Run one module's registration step. A failing step fails startup.
    ///
    /// # Errors
    ///
    /// Propagates whatever the module returns.
    pub fn module(mut self, register: fn(&mut Registry) -> Fallible) -> Fallible<Self> {
        register(&mut self)?;
        Ok(self)
    }

    /// Subscribe `handler` to signals of type `S`; handlers run in registration order.
    pub fn on<S: Signal>(&mut self, handler: Handler<S>) {
        self.bus.add(handler);
    }

    /// Declare the single implementation of port `P`.
    ///
    /// # Errors
    ///
    /// Returns `kernel.port_taken` when `P` already has a provider.
    pub fn provide<P: ?Sized + Port>(&mut self, implementation: Arc<P>) -> Fallible {
        self.ports.insert(implementation)
    }

    /// Freeze the registrations into the kernel, paired with its logger.
    pub fn finish(self, logger: Logger) -> Arc<Kernel> {
        Arc::new(Kernel {
            logger,
            bus: self.bus,
            ports: self.ports,
        })
    }
}
