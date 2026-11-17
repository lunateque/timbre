//! The binary's built-in CLI commands, registered into the kernel's command
//! table before it is finished, so dispatch is uniform (`Kernel::find_command`
//! is the only lookup) and a module that registers e.g. `db vacuum` either
//! coexists (the longest registered name wins) or fails registration with
//! `kernel.command_taken` -- never silently shadowed by a hard-coded branch.

use crate::db;
use crate::failures::TimbreFailure;
use crate::log_events::{DATABASE_FAILED, HELP, STARTED, STOPPED};
use crate::open_database;
use std::sync::OnceLock;
use timbre_kernel::signals::Started;
use timbre_kernel::{BoxFuture, Ctx, Failure, Fallible, Kernel, Registry, log};

/// What the handlers need beyond their `Ctx`: the loaded config (both paths
/// read `[server]`), the HTTP settings the serve path built before it
/// started serving, and the database, opened and migrated once on first use
/// and shared by every later command of the run. The config is runtime
/// input, which is why this is a `OnceLock` set from `main` rather than a
/// `LazyLock`.
struct App {
    config: timbre_edge_config::Config,
    http: OnceLock<timbre_edge_http::HttpConfig>,
    database: OnceLock<Result<timbre_edge_sqlite::Database, FailedDatabase>>,
}

/// A remembered open failure: its code is already on the log under
/// `timbre.database_failed`; the command exits on it without a second line.
struct FailedDatabase;

static APP: OnceLock<App> = OnceLock::new();

/// Install the configuration the built-in handlers read; called once from
/// `main` before any dispatch, on both the serve and the command path.
pub(crate) fn install(config: timbre_edge_config::Config) {
    let _ = APP.set(App {
        config,
        http: OnceLock::new(),
        database: OnceLock::new(),
    });
}

/// Hand the serving settings to the table's `serve` handler; only the serve
/// path sets them, and it sets them before dispatching.
pub(crate) fn set_http(http: timbre_edge_http::HttpConfig) {
    if let Some(app) = APP.get() {
        let _ = app.http.set(http);
    }
}

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
pub(crate) fn classify(outcome: Fallible) -> Outcome {
    let Err(failure) = outcome else {
        return Outcome::Success;
    };
    match failure.code() {
        crate::failures::USAGE => Outcome::Usage,
        crate::failures::REPORTED => Outcome::Reported,
        _ => Outcome::Failed(failure),
    }
}

/// The dispatcher: the kernel's table is consulted and only it. No argument
/// matches: the help list prints and the run ends with `timbre.usage`
/// (exit 2). Every command but `help` starts from an open, migrated
/// database, exactly as `serve` does.
///
/// # Errors
///
/// Propagates the handler's failure; an unknown command is `timbre.usage`,
/// a failed database open is `timbre.reported` after its own line.
pub(crate) async fn dispatch(ctx: &Ctx, kernel: &Kernel, args: &[String]) -> Fallible {
    let Some((handler, rest)) = kernel.find_command(args) else {
        help_list(ctx, kernel);
        return Err(Box::new(TimbreFailure::Usage) as Box<dyn Failure>);
    };
    // `help` answers from the table alone; everything else needs the data
    // directory and the database migrated underneath it.
    if args.first().is_none_or(|first| first != "help") {
        opened_database(ctx)?;
    }
    handler(ctx.clone(), rest).await
}

/// The database, opened and migrated once per process; the first failure
/// logs one `timbre.database_failed` line carrying the code of the original
/// failure (never a relabeled one), and every later caller exits on the
/// remembered fact without a second line.
fn opened_database(ctx: &Ctx) -> Fallible<&'static timbre_edge_sqlite::Database> {
    let Some(app) = APP.get() else {
        // Only reachable if `main` dispatched before installing the config:
        // a programming error, not the operator's.
        return Err(Box::new(TimbreFailure::DataDir {
            reason: "no configuration was installed for this run".to_owned(),
        }) as Box<dyn Failure>);
    };
    let outcome =
        app.database
            .get_or_init(|| match open_database(ctx.kernel(), &app.config, ctx) {
                Ok(database) => Ok(database),
                Err(failure) => {
                    log!(
                        ctx,
                        error,
                        DATABASE_FAILED,
                        code = failure.code().as_str(),
                        message = failure.public_message(),
                        reason = format!("{failure:?}").as_str()
                    );
                    Err(FailedDatabase)
                }
            });
    match outcome {
        Ok(database) => Ok(database),
        Err(FailedDatabase) => Err(Box::new(TimbreFailure::Reported) as Box<dyn Failure>),
    }
}

/// The `serve` command as the table knows it. `main` runs it inside the
/// tokio path -- the kernel must be finished with the serving runtime before
/// anything is built on it, and that is also the only place `HttpConfig` is
/// installed -- so reaching this handler without it is a startup bug, not a
/// serve attempt on the CLI runtime.
fn serve(ctx: Ctx, _rest: Vec<String>) -> BoxFuture<Fallible> {
    Box::pin(async move {
        let http = APP
            .get()
            .and_then(|app| app.http.get())
            .ok_or_else(|| Box::new(TimbreFailure::Runtime) as Box<dyn Failure>)?;
        let listen = http.listen.join(",");
        log!(ctx, info, STARTED, listen = listen.as_str());
        ctx.publish(&Started);
        ctx.kernel().start_jobs(&ctx);
        let outcome = timbre_edge_http::serve(ctx.clone(), http.clone()).await;
        // The server stopped accepting and drained. A drain that expired
        // with responses still in flight is noticed only inside `edge/http`
        // -- `serve` reports it neither in its result nor through a port --
        // so `timbre.stopped` is the last word the binary can honestly log.
        log!(ctx, info, STOPPED);
        outcome
    })
}

fn help(ctx: Ctx, _rest: Vec<String>) -> BoxFuture<Fallible> {
    Box::pin(async move {
        help_list(&ctx, ctx.kernel());
        Ok(())
    })
}

/// The bare `db` catch-all: the table's longest-prefix rule delivers every
/// `db <unknown>` here, where an unknown verb is a usage error.
fn db_command(ctx: Ctx, rest: Vec<String>) -> BoxFuture<Fallible> {
    Box::pin(async move { db::unknown_verb(&ctx, rest.first().map_or("", String::as_str)) })
}

fn db_check(ctx: Ctx, rest: Vec<String>) -> BoxFuture<Fallible> {
    Box::pin(async move { db::check(&ctx, opened_database(&ctx)?, &rest) })
}

fn db_backup(ctx: Ctx, rest: Vec<String>) -> BoxFuture<Fallible> {
    Box::pin(async move { db::backup(&ctx, opened_database(&ctx)?, &rest) })
}

fn db_prune(ctx: Ctx, rest: Vec<String>) -> BoxFuture<Fallible> {
    Box::pin(async move { db::prune(&ctx, ctx.kernel(), opened_database(&ctx)?, &rest) })
}

/// The usage line plus one `timbre.help` line per registered command.
fn help_list(ctx: &Ctx, kernel: &Kernel) {
    log!(
        ctx,
        info,
        HELP,
        text = "usage: timbre [--config <path>] <command> [args]"
    );
    for command in kernel.commands() {
        log!(
            ctx,
            info,
            HELP,
            text = format!("  {:18} {}", command.name, command.help)
        );
    }
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
