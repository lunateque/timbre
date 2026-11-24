//! The server binary: configuration, data dir, database, CLI dispatch,
//! runtimes.
//!
//! Every command shares one startup: config (argument, `TIMBRE_CONFIG`,
//! `./timbre.toml`), log filter (`TIMBRE_LOG` wins over `[log].filter`), the
//! registry with every module and the binary's own built-in commands
//! (`commands.rs`: `serve`, `help`, `db ...`), and the kernel. `serve` runs
//! on tokio; CLI commands on the kernel's `ThreadRuntime`. Commands that
//! need the database share lazy open and migration; help never opens it.

mod commands;
mod db;
mod failures;
mod log_events;
mod modules;

use log_events::{CONFIG_FAILED, LOG_FILTER_INVALID, REGISTRATION_FAILED};
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use timbre_edge_config::{Config, LogSection, ServerSection};
use timbre_edge_sqlite::SharedDatabase;
use timbre_kernel::{
    Ctx, Failure, FailureCode, Fallible, LogEvent, LogFilter, Logger, Registry, Resources,
    ThreadRuntime, log,
};

fn main() -> ExitCode {
    let mut config_path: Option<PathBuf> = None;
    let mut args = Vec::new();
    let mut next_is_config = false;
    for argument in std::env::args_os().skip(1) {
        if next_is_config {
            config_path = Some(PathBuf::from(argument));
            next_is_config = false;
            continue;
        }
        // Only the command words must be Unicode; the config path stays an
        // `OsString` so non-Unicode file names still work.
        let Some(text) = argument.to_str() else {
            return fatal_usage("an argument is not valid unicode");
        };
        if text == "--config" {
            next_is_config = true;
        } else {
            args.push(text.to_owned());
        }
    }
    if next_is_config {
        return fatal_usage("`--config` needs a path");
    }
    let config = match Config::load(config_path.as_deref()) {
        Ok(config) => config,
        Err(failure) => return fatal(&CONFIG_FAILED, &*failure),
    };
    let filter = match filter_from(&config) {
        Ok(filter) => filter,
        Err((raw, code)) => {
            // Nothing trustworthy exists yet: log with the default filter.
            return fatal_with(&LOG_FILTER_INVALID, &raw, code);
        }
    };
    let mut registry = match modules::registry() {
        Ok(registry) => registry,
        Err(failure) => return fatal(&REGISTRATION_FAILED, &*failure),
    };
    // The binary's own commands join the table before it is finished, after
    // the modules: a module that took `db check` (or `help`, or `serve`)
    // fails the run here with `kernel.command_taken` rather than having its
    // handler shadowed by a hard-coded branch.
    if let Err(failure) = commands::register(&mut registry) {
        return fatal(&REGISTRATION_FAILED, &*failure);
    }
    let resources = match runtime_resources(config) {
        Ok(resources) => resources,
        Err(failure) => return fatal(&CONFIG_FAILED, &*failure),
    };
    let logger = Logger::new(
        filter,
        if std::io::stdout().is_terminal() {
            timbre_kernel::Format::Pretty
        } else {
            timbre_kernel::Format::Json
        },
        Box::new(std::io::stdout()),
        std::time::SystemTime::now,
    );
    let serving = args.first().is_some_and(|first| first == "serve");
    let outcome: Fallible<ExitCode> = if serving {
        match timbre_edge_runtime::TokioRuntime::block_on(serve_with(
            registry, logger, &args, resources,
        )) {
            Ok(outcome) => outcome,
            // The executor itself could not start: no future ran, so the
            // failure is reported without a request context.
            Err(cause) => return fatal_runtime(&cause),
        }
    } else {
        let kernel =
            registry.finish_with_resources(logger, Arc::new(ThreadRuntime::new()), resources);
        let ctx = Ctx::root(&kernel);
        // Command handlers are futures driven on this thread: that is what
        // the kernel's `block_on` is for, and why the command path finishes
        // the kernel with a `ThreadRuntime`.
        let dispatched = timbre_kernel::block_on(commands::dispatch(&ctx, &kernel, &args));
        // The command's last lines (help text, `db` results) sit in the
        // async queue: drain it before the exit code leaves `main`.
        kernel.flush_log();
        match commands::classify(dispatched) {
            commands::Outcome::Success => Ok(ExitCode::SUCCESS),
            commands::Outcome::Usage => Ok(ExitCode::from(2)),
            commands::Outcome::Reported => Ok(ExitCode::FAILURE),
            commands::Outcome::Failed(failure) => Err(failure),
        }
    };
    match outcome {
        Ok(code) => code,
        Err(failure) => fatal(&log_events::COMMAND_FAILED, &*failure),
    }
}

/// The serving path, already inside the tokio runtime so the kernel is
/// finished with the real runtime. `serve` itself is the table's own
/// command handler, reached through the one dispatcher.
async fn serve_with(
    registry: Registry,
    logger: Logger,
    args: &[String],
    resources: Resources,
) -> Fallible<ExitCode> {
    let runtime = timbre_edge_runtime::TokioRuntime::current();
    let kernel = registry.finish_with_resources(
        logger,
        Arc::clone(&runtime) as Arc<dyn timbre_kernel::Runtime>,
        resources,
    );
    let ctx = Ctx::root(&kernel);
    let dispatched = commands::dispatch(&ctx, &kernel, args).await;
    // Jobs get the spec's shutdown grace: up to ten seconds to finish their
    // current work before the process leaves. A job still running after it
    // is dropped with the runtime; every database write is one transaction,
    // so nothing is left half-done. The answer says only whether they all
    // made it, which changes no part of an already-stopped server's exit.
    let _ = kernel.wait_jobs(std::time::Duration::from_secs(10));
    // The last request lines live in the async queue: drain it before the
    // process moves on to (or returns) its exit code.
    kernel.flush_log();
    match commands::classify(dispatched) {
        commands::Outcome::Success => Ok(ExitCode::SUCCESS),
        commands::Outcome::Usage => Ok(ExitCode::from(2)),
        commands::Outcome::Reported => Ok(ExitCode::FAILURE),
        // A serve that fails after `Started` is a startup failure too: the
        // exit is 1, whatever the binding or serving error was, and its
        // code reaches the log through the caller's one `command_failed`
        // line.
        commands::Outcome::Failed(failure) => Err(failure),
    }
}

/// Create the directory the database lives in. A directory this process
/// created is made private; one that was already there belongs to the
/// operator, so a mode that lets other users in is reported and left alone
/// rather than changed under them.
fn prepare_data_dir(ctx: &Ctx, path: &Path) -> Fallible {
    let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    else {
        return Ok(());
    };
    let fault = |error: &std::io::Error| {
        Box::new(failures::TimbreFailure::DataDir {
            reason: format!("{}: {error}", parent.display()),
        }) as Box<dyn Failure>
    };
    let existed = parent.try_exists().map_err(|error| fault(&error))?;
    std::fs::create_dir_all(parent).map_err(|error| fault(&error))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if existed {
            let mode = std::fs::metadata(parent)
                .map_err(|error| fault(&error))?
                .permissions()
                .mode();
            if mode & 0o077 != 0 {
                log!(
                    ctx,
                    warn,
                    log_events::DATA_DIR_PERMISSIVE,
                    path = parent.display().to_string(),
                    mode = format!("{:04o}", mode & 0o7777),
                );
            }
        } else {
            // A data dir this process just created and cannot keep private
            // must stop startup: an ignored failure here silently exposes
            // the database to every local user.
            std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))
                .map_err(|error| fault(&error))?;
        }
    }
    #[cfg(not(unix))]
    let _ = (ctx, existed);
    Ok(())
}

fn runtime_resources(config: Config) -> Fallible<Resources> {
    let section: ServerSection = config.section("server")?;
    let path = Path::new(&section.data_dir).join("timbre.db");
    let http = timbre_edge_http::HttpConfig::from_config(&config)?;
    Resources::new()
        .with(Arc::new(config))?
        .with(Arc::new(http))?
        .with(Arc::new(SharedDatabase::new(path, prepare_data_dir)))
}

/// The filter named by `TIMBRE_LOG`, else `[log].filter`, else the default;
/// on `Err` the raw value and the code that explain the rejection.
fn filter_from(config: &Config) -> Result<LogFilter, (String, FailureCode)> {
    let raw = match std::env::var("TIMBRE_LOG") {
        Err(std::env::VarError::NotPresent) => {
            let section: LogSection = config
                .section("log")
                .map_err(|failure| ("<log>".to_owned(), failure.code()))?;
            match section.filter {
                Some(filter) => filter,
                None => return Ok(LogFilter::default()),
            }
        }
        Err(std::env::VarError::NotUnicode(_)) => {
            return Err((
                "<not unicode>".to_owned(),
                timbre_kernel::failures::LOG_FILTER_INVALID,
            ));
        }
        Ok(raw) => raw,
    };
    LogFilter::parse(&raw).map_err(|failure| (raw, failure.code()))
}

/// Report a startup or command failure with the default logger and exit 1.
pub(crate) fn fatal(event: &LogEvent, failure: &dyn Failure) -> ExitCode {
    let kernel = Registry::new().finish(
        Logger::stdout(LogFilter::default()),
        Arc::new(ThreadRuntime::new()),
    );
    log!(
        Ctx::root(&kernel),
        error,
        *event,
        code = failure.code().as_str(),
        message = failure.public_message()
    );
    kernel.flush_log();
    ExitCode::FAILURE
}

/// The command line itself is unusable: report it like the usage block,
/// exit 2, with the queue drained before the process leaves.
fn fatal_usage(reason: &str) -> ExitCode {
    let kernel = Registry::new().finish(
        Logger::stdout(LogFilter::default()),
        Arc::new(ThreadRuntime::new()),
    );
    log!(
        Ctx::root(&kernel),
        error,
        log_events::USAGE,
        reason = reason
    );
    kernel.flush_log();
    ExitCode::from(2)
}

fn fatal_with(event: &LogEvent, raw: &str, code: FailureCode) -> ExitCode {
    let kernel = Registry::new().finish(
        Logger::stdout(LogFilter::default()),
        Arc::new(ThreadRuntime::new()),
    );
    log!(
        Ctx::root(&kernel),
        error,
        *event,
        value = raw,
        code = code.as_str()
    );
    kernel.flush_log();
    ExitCode::FAILURE
}

/// The async executor could not start: report the operating system's reason,
/// which the failure does not carry, next to the failure's own code.
fn fatal_runtime(cause: &std::io::Error) -> ExitCode {
    let failure = failures::TimbreFailure::Runtime;
    let reason = cause.to_string();
    let kernel = Registry::new().finish(
        Logger::stdout(LogFilter::default()),
        Arc::new(ThreadRuntime::new()),
    );
    log!(
        Ctx::root(&kernel),
        error,
        log_events::RUNTIME_FAILED,
        code = failure.code().as_str(),
        message = failure.public_message(),
        reason = reason.as_str()
    );
    kernel.flush_log();
    ExitCode::FAILURE
}
