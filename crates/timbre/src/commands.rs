//! The binary's built-in CLI commands, registered into the kernel's command
//! table before it is finished, so dispatch is uniform (`Kernel::find_command`
//! is the only lookup) and a module that registers e.g. `db vacuum` either
//! coexists (the longest registered name wins) or fails registration with
//! `kernel.command_taken` -- never silently shadowed by a hard-coded branch.

use crate::db;
use crate::failures::TimbreFailure;
use crate::log_events::{DATABASE_FAILED, STARTED};
use timbre_edge_sqlite::{Database, SharedDatabase};
use timbre_kernel::signals::Started;
use timbre_kernel::{BoxFuture, Ctx, Failure, Fallible, Kernel, Registry, log};

/// Register every built-in command on `registry`, after the modules: a
/// module that already took one of these names fails registration rather
/// than being shadowed.
///
/// # Errors
///
/// `kernel.command_taken` when a module holds one of these names,
/// `kernel.command_invalid` for a malformed name.
pub(crate) fn register(registry: &mut Registry) -> Fallible {
    registry.command("serve", "run the server until the stop signal", serve)?;
    registry.command("help", "show this list", help)?;
    registry.command(
        "db",
        "database maintenance: check, backup or prune",
        db_command,
    )?;
    registry.command("db check", "run SQLite's integrity check", db_check)?;
    registry.command(
        "db backup",
        "copy the database to <file>, replacing an existing one only with --force",
        db_backup,
    )?;
    registry.command(
        "db prune",
        "list (with --yes: drop) tables no registered module owns",
        db_prune,
    )?;
    Ok(())
}

/// What a dispatched command's outcome means for the process exit; `main`
/// turns these into codes, logging only the `Failed` case.
pub(crate) enum Outcome {
    /// The command succeeded: exit 0.
    Success,
    /// The command line was unusable; its `timbre.usage` line is logged: exit 2.
    Usage,
    /// The command failed and the code that failed it already logged the
    /// explanatory line: exit 1 without a second one.
    Reported,
    /// The command failed with a failure nobody logged yet.
    Failed(Box<dyn Failure>),
}

/// Classify a dispatched command's failure for the exit path.
pub(crate) fn classify(ctx: &Ctx, outcome: Fallible) -> Outcome {
    let Err(failure) = outcome else {
        return Outcome::Success;
    };
    match failure.code() {
        crate::failures::USAGE => Outcome::Usage,
        timbre_kernel::failures::USAGE => {
            log!(
                ctx,
                error,
                crate::log_events::USAGE,
                reason = failure.public_message()
            );
            Outcome::Usage
        }
        crate::failures::REPORTED => Outcome::Reported,
        _ => Outcome::Failed(failure),
    }
}

/// The dispatcher: the kernel's table is consulted and only it. No argument
/// matches: the help list prints and the run ends with `timbre.usage`
/// (exit 2). A command accesses the shared database only when it needs it.
///
/// # Errors
///
/// Propagates the handler's failure; an unknown command is `timbre.usage`.
pub(crate) async fn dispatch(ctx: &Ctx, kernel: &Kernel, args: &[String]) -> Fallible {
    let Some((handler, rest)) = kernel.find_command(args) else {
        // The usage failure is the answer here; a stdout that cannot take
        // the command list does not change what went wrong.
        let _ = help_list(ctx, kernel);
        return Err(Box::new(TimbreFailure::Usage) as Box<dyn Failure>);
    };
    handler(ctx.clone(), rest).await
}

fn with_database<T>(ctx: &Ctx, read: impl FnOnce(&Database) -> Fallible<T>) -> Fallible<T> {
    let shared = ctx.resource::<SharedDatabase>().ok_or_else(|| {
        Box::new(TimbreFailure::DataDir {
            reason: "no database was installed for this run".to_owned(),
        }) as Box<dyn Failure>
    })?;
    match shared.get(ctx) {
        Ok(database) => read(database),
        Err(failure) => {
            log!(
                ctx,
                error,
                DATABASE_FAILED,
                code = failure.code().as_str(),
                message = failure.public_message(),
                reason = format!("{failure:?}").as_str()
            );
            Err(Box::new(TimbreFailure::Reported))
        }
    }
}

/// The `serve` command as the table knows it. `main` runs it inside the
/// tokio path -- the kernel must be finished with the serving runtime before
/// anything is built on it, and that is also the only place `HttpConfig` is
/// installed -- so reaching this handler without it is a startup bug, not a
/// serve attempt on the CLI runtime.
fn serve(ctx: Ctx, _rest: Vec<String>) -> BoxFuture<Fallible> {
    Box::pin(async move {
        let http = ctx
            .resource::<timbre_edge_http::HttpConfig>()
            .ok_or_else(|| Box::new(TimbreFailure::Runtime) as Box<dyn Failure>)?;
        with_database(&ctx, |_| Ok(()))?;
        let listen = http.listen.join(",");
        log!(ctx, info, STARTED, listen = listen.as_str());
        ctx.publish(&Started);
        ctx.kernel().start_jobs(&ctx);
        timbre_edge_http::serve(ctx, (*http).clone()).await
    })
}

fn help(ctx: Ctx, _rest: Vec<String>) -> BoxFuture<Fallible> {
    Box::pin(async move {
        let kernel = ctx.kernel();
        help_list(&ctx, kernel)
    })
}

/// The bare `db` catch-all: the table's longest-prefix rule delivers every
/// `db <unknown>` here, where an unknown verb is a usage error.
fn db_command(ctx: Ctx, rest: Vec<String>) -> BoxFuture<Fallible> {
    Box::pin(async move { db::unknown_verb(&ctx, rest.first().map_or("", String::as_str)) })
}

fn db_check(ctx: Ctx, rest: Vec<String>) -> BoxFuture<Fallible> {
    Box::pin(async move { with_database(&ctx, |database| db::check(&ctx, database, &rest)) })
}

fn db_backup(ctx: Ctx, rest: Vec<String>) -> BoxFuture<Fallible> {
    Box::pin(async move { with_database(&ctx, |database| db::backup(&ctx, database, &rest)) })
}

fn db_prune(ctx: Ctx, rest: Vec<String>) -> BoxFuture<Fallible> {
    Box::pin(async move {
        with_database(&ctx, |database| {
            db::prune(&ctx, ctx.kernel(), database, &rest)
        })
    })
}

/// The usage line plus one line per registered command, on stdout: a
/// command's own output never depends on the log filter.
fn help_list(ctx: &Ctx, kernel: &Kernel) -> Fallible {
    ctx.say(format_args!(
        "usage: timbre [--config <path>] <command> [args]"
    ))?;
    for command in kernel.commands() {
        ctx.say(format_args!("  {:18} {}", command.name, command.help))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn noop(_: Ctx, _: Vec<String>) -> BoxFuture<Fallible> {
        Box::pin(async { Ok(()) })
    }

    #[test]
    fn a_module_claiming_a_builtin_name_fails_registration() {
        // The table is one namespace. A module's `db check` collides with
        // the built-in whichever registered first; `db vacuum` coexists and
        // the longest prefix still wins.
        let mut registry = Registry::new();
        register(&mut registry).expect("the built-ins register into an empty table");
        let error = registry
            .command("db check", "mine", noop)
            .expect_err("the name is taken by the built-in");
        assert_eq!(error.code(), timbre_kernel::failures::COMMAND_TAKEN);
        let mut registry = Registry::new();
        registry
            .command("db check", "mine", noop)
            .expect("the module registers first");
        let error = register(&mut registry).expect_err("the built-in collides");
        assert_eq!(error.code(), timbre_kernel::failures::COMMAND_TAKEN);
        let mut registry = Registry::new();
        registry
            .command("db vacuum", "mine", noop)
            .expect("a fresh name under `db` registers");
        register(&mut registry).expect("the built-ins fit around it");
    }

    #[test]
    fn every_builtin_is_dispatchable_through_the_table() {
        let mut registry = Registry::new();
        register(&mut registry).expect("the built-ins register");
        let kernel = registry.finish(
            timbre_kernel::Logger::stdout(timbre_kernel::LogFilter::default()),
            std::sync::Arc::new(timbre_kernel::ThreadRuntime::new()),
        );
        for name in ["serve", "help", "db", "db check", "db backup", "db prune"] {
            let args: Vec<String> = name.split(' ').map(str::to_owned).collect();
            assert!(
                kernel.find_command(&args).is_some(),
                "`{name}` is dispatchable"
            );
        }
        // An unregistered verb under `db` reaches the `db` catch-all, which
        // answers it as a usage error.
        let (_handler, rest) = kernel
            .find_command(&["db".to_owned(), "vacuum".to_owned()])
            .expect("the bare `db` name is the longest prefix");
        assert_eq!(rest, ["vacuum".to_owned()]);
    }
}
