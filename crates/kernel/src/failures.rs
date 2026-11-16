//! The kernel's own failure codes, mirroring the per-module `failures.rs` convention.

use crate::failure::{Failure, FailureCode};

/// A port was provided twice.
pub const PORT_TAKEN: FailureCode = FailureCode::new("kernel.port_taken");
/// The log filter string could not be parsed.
pub const LOG_FILTER_INVALID: FailureCode = FailureCode::new("kernel.log_filter_invalid");
/// A route pattern is malformed.
pub const ROUTE_INVALID: FailureCode = FailureCode::new("kernel.route_invalid");
/// A method and pattern pair is registered twice.
pub const ROUTE_TAKEN: FailureCode = FailureCode::new("kernel.route_taken");
/// A job name is registered twice.
pub const JOB_TAKEN: FailureCode = FailureCode::new("kernel.job_taken");
/// A permission is declared twice.
pub const PERMISSION_TAKEN: FailureCode = FailureCode::new("kernel.permission_taken");
/// A credential kind has a second owner.
pub const CREDENTIAL_KIND_TAKEN: FailureCode = FailureCode::new("kernel.credential_kind_taken");
/// A module registers the same migration version twice.
pub const MIGRATION_TAKEN: FailureCode = FailureCode::new("kernel.migration_taken");
/// A command name is registered twice.
pub const COMMAND_TAKEN: FailureCode = FailureCode::new("kernel.command_taken");
/// A command name is malformed.
pub const COMMAND_INVALID: FailureCode = FailureCode::new("kernel.command_invalid");
/// A blocking closure panicked, so it produced no value.
pub const BLOCKING_PANICKED: FailureCode = FailureCode::new("kernel.blocking_panicked");

/// Failures raised by the kernel itself.
#[derive(Debug)]
pub enum KernelFailure {
    /// `port` is the `type_name` of the port with two providers.
    PortTaken {
        /// The type name of the port that was provided twice.
        port: &'static str,
    },
    /// The `TIMBRE_LOG` (or config) filter string is malformed.
    LogFilterInvalid,
    /// `pattern` does not parse as a route pattern.
    RouteInvalid {
        /// The rejected pattern.
        pattern: &'static str,
    },
    /// `method` and `pattern` already have a route.
    RouteTaken {
        /// The method of the route registered twice.
        method: &'static str,
        /// The pattern of the route registered twice.
        pattern: &'static str,
    },
    /// `name` is the log event of a job registered twice.
    JobTaken {
        /// The event name of the job registered twice.
        name: &'static str,
    },
    /// `permission` was declared by two registrations.
    PermissionTaken {
        /// The permission declared twice.
        permission: &'static str,
    },
    /// `kind` already has a credential provider.
    CredentialKindTaken {
        /// The kind with two owners.
        kind: &'static str,
    },
    /// The module registered this migration version twice.
    MigrationTaken {
        /// The owning module id.
        module: &'static str,
        /// The duplicated version.
        version: u32,
    },
    /// `name` is the command name registered twice.
    CommandTaken {
        /// The command name registered twice.
        name: String,
    },
    /// `name` is not lowercase words separated by single spaces.
    CommandInvalid {
        /// The malformed command name.
        name: String,
    },
    /// A blocking closure panicked, so it produced no value.
    BlockingPanicked,
}

impl Failure for KernelFailure {
    fn code(&self) -> FailureCode {
        match self {
            Self::PortTaken { .. } => PORT_TAKEN,
            Self::LogFilterInvalid => LOG_FILTER_INVALID,
            Self::RouteInvalid { .. } => ROUTE_INVALID,
            Self::RouteTaken { .. } => ROUTE_TAKEN,
            Self::JobTaken { .. } => JOB_TAKEN,
            Self::PermissionTaken { .. } => PERMISSION_TAKEN,
            Self::CredentialKindTaken { .. } => CREDENTIAL_KIND_TAKEN,
            Self::MigrationTaken { .. } => MIGRATION_TAKEN,
            Self::CommandTaken { .. } => COMMAND_TAKEN,
            Self::CommandInvalid { .. } => COMMAND_INVALID,
            Self::BlockingPanicked => BLOCKING_PANICKED,
        }
    }

    fn status(&self) -> u16 {
        // Startup failures precede any request; a blocking panic can happen while serving.
        // Both are server faults, so every variant maps to 500.
        500
    }

    fn public_message(&self) -> &'static str {
        match self {
            Self::BlockingPanicked => "The server failed to finish a background task.",
            Self::PortTaken { .. }
            | Self::LogFilterInvalid
            | Self::RouteInvalid { .. }
            | Self::RouteTaken { .. }
            | Self::JobTaken { .. }
            | Self::PermissionTaken { .. }
            | Self::CredentialKindTaken { .. }
            | Self::MigrationTaken { .. }
            | Self::CommandTaken { .. }
            | Self::CommandInvalid { .. } => "The server failed to start.",
        }
    }
}
