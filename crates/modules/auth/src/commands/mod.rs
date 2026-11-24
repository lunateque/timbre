//! The `user` and `device` commands.

mod device;
mod user;

use crate::{failures, log_events::LOOKUP_FAILED};
use timbre_kernel::{Ctx, Fallible, Registry, failures::KernelFailure, log};

pub(crate) fn argument<T>(usage: &'static str) -> Fallible<T> {
    Err(Box::new(KernelFailure::Usage(usage)))
}

fn lookup<T>(ctx: &Ctx, value: &str, result: Fallible<T>) -> Fallible<T> {
    result.inspect_err(|failure| {
        let code = failure.code();
        if matches!(
            code,
            failures::NOT_FOUND | failures::EXISTS | failures::UNKNOWN_PERMISSION
        ) {
            log!(
                ctx,
                warn,
                LOOKUP_FAILED,
                code = code.as_str(),
                value = value
            );
        }
    })
}

pub(crate) fn register(registry: &mut Registry) -> Fallible {
    registry.command(
        "user add",
        "Add a user: user add <name> [--admin].",
        user::add,
    )?;
    registry.command("user list", "List users and what they may do.", user::list)?;
    registry.command(
        "user remove",
        "Remove a user and every device of theirs: user remove <name>.",
        user::remove,
    )?;
    registry.command(
        "user grant",
        "Grant a declared permission: user grant <name> <permission>.",
        user::grant,
    )?;
    registry.command(
        "user revoke",
        "Take a permission back: user revoke <name> <permission>.",
        user::revoke,
    )?;
    registry.command(
        "device issue",
        "Issue a device secret, shown once: device issue <user> <label> [--expires <days>].",
        device::issue,
    )?;
    registry.command(
        "device reissue",
        "Replace the secret of a device, e.g. one whose print was lost: device reissue <id>.",
        device::reissue,
    )?;
    registry.command(
        "device list",
        "List devices: device list [<user>].",
        device::list,
    )?;
    registry.command(
        "device revoke",
        "Stop a device from authenticating: device revoke <id>.",
        device::revoke,
    )
}
