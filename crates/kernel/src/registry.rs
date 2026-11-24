//! Registration: modules declare their handlers and ports on a `Registry`,
//! which the binary finishes into the shared `Kernel`.

use crate::Resources;
use crate::bus::{Bus, Handler, Signal};
use crate::command::{Command, CommandHandler, CommandTable};
use crate::credential::{
    CredentialKind, CredentialProvider, CredentialTable, Throttle, owner_label,
};
use crate::failure::Fallible;
use crate::failures::KernelFailure;
use crate::guard::Guard;
use crate::identity::{Permission, PermissionEntry};
use crate::jobs::{JobHandler, JobTable, JobTracker};
use crate::log::Logger;
use crate::migration::Migration;
use crate::port::{Port, Ports};
use crate::route::{RouteHandler, RouteMatch, RouteTable};
use crate::runtime::Runtime;
use std::sync::Arc;
use std::time::Duration;

/// The finished, read-only service core every `Ctx` points at.
pub struct Kernel {
    logger: Logger,
    runtime: Arc<dyn Runtime>,
    bus: Bus,
    pub(crate) resources: Resources,
    ports: Ports,
    routes: RouteTable,
    jobs: JobTable,
    running_jobs: JobTracker,
    permissions: Vec<PermissionEntry>,
    credentials: CredentialTable,
    throttle: Option<Arc<dyn Throttle>>,
    migrations: Vec<Migration>,
    commands: CommandTable,
}

impl Kernel {
    pub(crate) fn logger(&self) -> &Logger {
        &self.logger
    }

    /// Report any dropped log lines, then block until every queued line is
    /// written; for tests and shutdown. Returns whether the sink caught up
    /// within the wait bound (2 s): `false` means a wedged sink still holds
    /// lines, not that the caller hung. Callers that do not read lines back
    /// can ignore the answer.
    pub fn flush_log(&self) -> bool {
        self.logger.flush()
    }

    pub(crate) fn bus(&self) -> &Bus {
        &self.bus
    }

    pub(crate) fn runtime(&self) -> &Arc<dyn Runtime> {
        &self.runtime
    }

    pub(crate) fn credential_provider(
        &self,
        kind: CredentialKind,
    ) -> Option<&Arc<dyn CredentialProvider>> {
        self.credentials.get(kind)
    }

    /// The registered brute-force limiter, if a module provided one.
    pub(crate) fn throttle(&self) -> Option<&Arc<dyn Throttle>> {
        self.throttle.as_ref()
    }

    /// Start every registered job on the runtime, logging each job's start,
    /// end and failure. The binary calls this after `Started` is published.
    pub fn start_jobs(&self, ctx: &crate::Ctx) {
        self.jobs.start(ctx, &self.running_jobs);
    }

    /// Whether every job started by [`Kernel::start_jobs`] has finished within
    /// `timeout`. The binary calls it at shutdown, before flushing the log, so
    /// a job's last lines are not lost behind a still-running future.
    #[must_use]
    pub fn wait_jobs(&self, timeout: Duration) -> bool {
        self.running_jobs.wait(timeout)
    }

    pub(crate) fn ports(&self) -> &Ports {
        &self.ports
    }

    /// What the route table knows about `method` and `path`.
    #[must_use]
    pub fn find_route(&self, method: crate::http::Method, path: &str) -> RouteMatch {
        self.routes.find(method, path)
    }

    /// Every declared permission, in registration order.
    #[must_use]
    pub fn permissions(&self) -> &[PermissionEntry] {
        &self.permissions
    }

    /// Every migration of the registered modules, ordered by registration and
    /// then version within each module; applying them is the database edge's job.
    #[must_use]
    pub fn migrations(&self) -> &[Migration] {
        &self.migrations
    }

    /// Every registered CLI command, in registration order.
    #[must_use]
    pub fn commands(&self) -> Vec<Command<'_>> {
        self.commands.list()
    }

    /// The command whose name is the longest prefix of `args`, with the words
    /// after the name returned for the handler.
    #[must_use]
    pub fn find_command(&self, args: &[String]) -> Option<(CommandHandler, Vec<String>)> {
        self.commands.find(args)
    }
}

/// What a module may declare while registering. The binary threads one registry
/// through every module in build order and finishes it once at the end.
#[derive(Default)]
pub struct Registry {
    bus: Bus,
    ports: Ports,
    routes: RouteTable,
    jobs: JobTable,
    permissions: Vec<PermissionEntry>,
    credentials: CredentialTable,
    throttle: Option<Arc<dyn Throttle>>,
    throttle_owner: Option<&'static str>,
    migrations: Vec<Migration>,
    commands: CommandTable,
    /// The module whose registration step is running right now, named by
    /// `Registry::module`; owners of credential kinds and of the throttle are
    /// attributed to it so a clash can name both modules.
    registering: Option<&'static str>,
}

impl Registry {
    /// An empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Run one module's registration step under the module id `id`: one
    /// lowercase name segment, given explicitly because the type of the
    /// function behind `register` says nothing dependable about who wrote
    /// it. `kernel`, `timbre` and `xtask` are reserved: they name the kernel
    /// itself, the binary and the tooling, so no module may claim their
    /// prefixes. A failing step fails startup. Owners installed during the
    /// step (credential providers, the throttle) are attributed to `id` by
    /// name when a later registration clashes with them.
    ///
    /// # Errors
    ///
    /// `kernel.module_invalid` for a malformed or reserved `id`; otherwise
    /// propagates whatever the module returns.
    pub fn module(
        self,
        id: &'static str,
        register: impl FnOnce(&mut Self) -> Fallible,
    ) -> Fallible<Self> {
        if !crate::names::is_valid_segment(id) || matches!(id, "kernel" | "timbre" | "xtask") {
            return Err(Box::new(KernelFailure::ModuleInvalid { id }));
        }
        self.step(Some(id), register)
    }

    fn step(
        mut self,
        id: Option<&'static str>,
        register: impl FnOnce(&mut Self) -> Fallible,
    ) -> Fallible<Self> {
        let outer = self.registering;
        self.registering = id;
        let outcome = register(&mut self);
        self.registering = outer;
        outcome?;
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

    /// Bind `handler` to `method` on `pattern`: literal segments and one
    /// optional trailing `{name}` capture. `guard` is mandatory: the kernel
    /// authenticates and checks it before the handler runs, so a route that
    /// declares no access rule cannot exist.
    ///
    /// # Errors
    ///
    /// `kernel.route_invalid` for a malformed pattern, `kernel.route_taken`
    /// when the method and pattern pair is registered twice.
    pub fn route(
        &mut self,
        method: crate::http::Method,
        pattern: &'static str,
        guard: Guard,
        handler: RouteHandler,
    ) -> Fallible {
        self.routes.add(method, pattern, guard, handler)
    }

    /// Declare a background job, started after `Started` is published.
    ///
    /// # Errors
    ///
    /// `kernel.job_taken` when the name is registered twice.
    pub fn job(&mut self, name: crate::log::LogEvent, handler: JobHandler) -> Fallible {
        self.jobs.add(name, handler)
    }

    /// Declare a permission this module checks, with the description shown to
    /// whoever grants and revokes it.
    ///
    /// # Errors
    ///
    /// `kernel.permission_taken` when the permission is declared twice.
    pub fn permission(&mut self, permission: Permission, description: &'static str) -> Fallible {
        if self
            .permissions
            .iter()
            .any(|entry| entry.permission == permission)
        {
            return Err(Box::new(KernelFailure::PermissionTaken {
                permission: permission.as_str(),
            }));
        }
        self.permissions.push(PermissionEntry {
            permission,
            description,
        });
        Ok(())
    }

    /// Declare the single owner of credential kind `kind`. Kinds and scheme
    /// names share one table: `CredentialKind::Scheme` is keyed by its name,
    /// so a second owner of the same scheme clashes the same way a second
    /// owner of `ApiKey` does, and the refusal names both registering
    /// modules.
    ///
    /// # Errors
    ///
    /// `kernel.credential_kind_taken` when the kind already has a provider.
    pub fn credential_provider(
        &mut self,
        kind: CredentialKind,
        provider: Arc<dyn CredentialProvider>,
    ) -> Fallible {
        self.credentials.add(kind, provider, self.registering)
    }

    /// Declare the single brute-force limiter consulted before every
    /// credential check. It is a kernel-owned service, not a port: exactly
    /// one may exist and only the kernel calls it.
    ///
    /// # Errors
    ///
    /// `kernel.throttle_taken` when a throttle is already registered.
    pub fn throttle(&mut self, throttle: Arc<dyn Throttle>) -> Fallible {
        if self.throttle.is_some() {
            return Err(Box::new(KernelFailure::ThrottleTaken {
                first: owner_label(self.throttle_owner),
                second: owner_label(self.registering),
            }));
        }
        self.throttle_owner = self.registering;
        self.throttle = Some(throttle);
        Ok(())
    }

    /// Ship one migration step of `module`; versions apply ascending, all of a
    /// module's pending steps at once.
    ///
    /// # Errors
    ///
    /// `kernel.migration_taken` for a repeated module+version pair or a module
    /// id that is not one name segment.
    pub fn migration(&mut self, module: &'static str, version: u32, sql: &'static str) -> Fallible {
        if !crate::migration::accepted(module, version, &self.migrations) {
            return Err(Box::new(KernelFailure::MigrationTaken { module, version }));
        }
        // Keep each module's steps contiguous and ascending in version, so
        // `Kernel::migrations` is already in application order: registration
        // order between modules, version order within one.
        let start = self
            .migrations
            .iter()
            .take_while(|entry| entry.module() != module)
            .count();
        let end = self
            .migrations
            .iter()
            .skip(start)
            .take_while(|entry| entry.module() == module)
            .count()
            + start;
        let at = self
            .migrations
            .iter()
            .skip(start)
            .take(end - start)
            .filter(|entry| entry.version() < version)
            .count()
            + start;
        self.migrations
            .insert(at, Migration::new(module, version, sql));
        Ok(())
    }

    /// Contribute a CLI command: `name` is one or more lowercase words
    /// separated by single spaces, `help` one line for `timbre help`.
    ///
    /// # Errors
    ///
    /// `kernel.command_invalid` for a malformed name, `kernel.command_taken`
    /// when the name is registered twice.
    pub fn command(&mut self, name: &str, help: &'static str, handler: CommandHandler) -> Fallible {
        self.commands.add(name, help, handler)
    }

    /// Freeze the registrations into the kernel. The runtime is wired here,
    /// not through a port: it is server infrastructure, and every consumer
    /// needs it, so removability does not apply.
    pub fn finish(self, logger: Logger, runtime: Arc<dyn Runtime>) -> Arc<Kernel> {
        self.finish_with_resources(logger, runtime, Resources::new())
    }

    /// Freeze the registry with read-only, host-provided runtime resources.
    pub fn finish_with_resources(
        self,
        logger: Logger,
        runtime: Arc<dyn Runtime>,
        resources: Resources,
    ) -> Arc<Kernel> {
        Arc::new(Kernel {
            logger,
            runtime,
            bus: self.bus,
            ports: self.ports,
            resources,
            routes: self.routes,
            jobs: self.jobs,
            running_jobs: JobTracker::new(),
            permissions: self.permissions,
            credentials: self.credentials,
            throttle: self.throttle,
            migrations: self.migrations,
            commands: self.commands,
        })
    }
}
