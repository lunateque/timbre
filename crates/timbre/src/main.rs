//! The server binary: configuration, startup, CLI dispatch, serving.
//!
//! Every command shares one startup: config (argument, `TIMBRE_CONFIG`,
//! `./timbre.toml`), log filter (`TIMBRE_LOG` wins over `[log].filter`), the
//! registry with every built-in module, the kernel, and the migrated
//! database. `serve` runs on tokio; CLI commands on the kernel's own
//! `ThreadRuntime`. `help` is answered without the database.

mod db;
mod failures;
mod log_events;
mod modules;

use log_events::{
    COMMAND_FAILED, CONFIG_FAILED, DATABASE_FAILED, HELP, LOG_FILTER_INVALID, REGISTRATION_FAILED,
    RUNTIME_FAILED, STARTED,
};
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use timbre_edge_config::{Config, HttpSection, LogSection, ServerSection};
use timbre_kernel::signals::Started;
use timbre_kernel::{
    Ctx, Failure, FailureCode, Fallible, Kernel, LogEvent, LogFilter, Logger, Registry,
    ThreadRuntime, log,
};

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let mut config_path: Option<PathBuf> = None;
    let mut args = Vec::new();
    let mut next_is_config = false;
    for argument in arguments {
        if next_is_config {
            config_path = Some(PathBuf::from(argument));
            next_is_config = false;
        } else if argument == "--config" {
            next_is_config = true;
        } else {
            args.push(argument);
        }
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
    let registry = match modules::registry() {
        Ok(registry) => registry,
        Err(failure) => return fatal(&REGISTRATION_FAILED, &*failure),
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
        match timbre_edge_runtime::TokioRuntime::block_on(serve_with(registry, logger, &config)) {
            Ok(outcome) => outcome,
            // The executor itself could not start: no future ran, so the
            // failure is reported without a request context.
            Err(cause) => return fatal_runtime(&cause),
        }
    } else {
        let kernel = registry.finish(logger, Arc::new(ThreadRuntime::new()));
        let ctx = Ctx::root(&kernel);
        command(&ctx, &kernel, &config, &args)
    };
    match outcome {
        Ok(code) => code,
        Err(failure) => fatal(&COMMAND_FAILED, &*failure),
    }
}

/// The serving path, already inside the tokio runtime so the kernel is
/// finished with the real runtime.
async fn serve_with(registry: Registry, logger: Logger, config: &Config) -> Fallible<ExitCode> {
    let http = http_config(config);
    let listen = http.listen.join(",");
    let runtime = timbre_edge_runtime::TokioRuntime::current();
    let kernel = registry.finish(
        logger,
        Arc::clone(&runtime) as Arc<dyn timbre_kernel::Runtime>,
    );
    let ctx = Ctx::root(&kernel);
    if let Err(failure) = open_database(&kernel, config) {
        log!(
            ctx,
            error,
            DATABASE_FAILED,
            code = failure.code().as_str(),
            message = failure.public_message()
        );
        return Ok(ExitCode::FAILURE);
    }
    log!(ctx, info, STARTED, listen = listen.as_str());
    ctx.publish(&Started);
    kernel.start_jobs(&ctx);
    timbre_edge_http::serve(ctx.clone(), http).await?;
    Ok(ExitCode::SUCCESS)
}

/// The non-serving commands.
fn command(
    ctx: &Ctx,
    kernel: &Arc<Kernel>,
    config: &Config,
    args: &[String],
) -> Fallible<ExitCode> {
    let first = args.first().map_or("", String::as_str);
    if first == "help" {
        print_help(ctx, kernel);
        return Ok(ExitCode::SUCCESS);
    }
    if args.is_empty() || (first != "db" && kernel.find_command(args).is_none()) {
        print_help(ctx, kernel);
        return Ok(ExitCode::from(2));
    }
    let database = match open_database(kernel, config) {
        Ok(database) => database,
        Err(failure) => {
            log!(
                ctx,
                error,
                DATABASE_FAILED,
                code = failure.code().as_str(),
                message = failure.public_message()
            );
            return Ok(ExitCode::FAILURE);
        }
    };
    if first == "db" {
        return db::run(ctx, kernel, &database, args);
    }
    if let Some((handler, rest)) = kernel.find_command(args) {
        // CLI handlers are futures; `block_on` drives one on this thread,
        // which is why the command path runs on `ThreadRuntime`.
        timbre_kernel::block_on(handler(ctx.clone(), rest))?;
    }
    Ok(ExitCode::SUCCESS)
}

/// One `timbre.help` line per command, the built-ins first.
fn print_help(ctx: &Ctx, kernel: &Kernel) {
    for line in [
        "usage: timbre [--config <path>] <command> [args]",
        "  serve              run the server until the stop signal",
        "  db check           run SQLite's integrity check",
        "  db backup <file>   copy the database to <file>",
        "  db prune [--yes]   list (with --yes: drop) tables no registered module owns",
        "  help               show this list",
    ] {
        log!(ctx, info, HELP, text = line);
    }
    for command in kernel.commands() {
        log!(
            ctx,
            info,
            HELP,
            text = format!("  {:18} {}", command.name, command.help)
        );
    }
}

/// Open (creating `[server].data_dir`, mode 700 on Unix) and migrate the
/// database with every registered module's steps; the handle is returned so
/// a command reuses this connection instead of opening the file again.
fn open_database(kernel: &Arc<Kernel>, config: &Config) -> Fallible<timbre_edge_sqlite::Database> {
    let path = database_path(config);
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent).map_err(|error| {
            Box::new(timbre_edge_sqlite::SqliteFailure::Open {
                reason: error.to_string(),
            }) as Box<dyn Failure>
        })?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700));
        }
    }
    let database =
        timbre_edge_sqlite::Database::open(&path).map_err(|failure| database_failure(&*failure))?;
    database.migrate(kernel.migrations())?;
    Ok(database)
}

/// The database could not be opened or migrated: a startup failure with the
/// edge's own code inside `reason`, so the operator sees the file error.
fn database_failure(failure: &dyn Failure) -> Box<dyn Failure> {
    let reason = format!("{}: {}", failure.public_message(), failure.code().as_str());
    Box::new(timbre_edge_sqlite::SqliteFailure::Open { reason })
}

fn database_path(config: &Config) -> PathBuf {
    let section: ServerSection = config.section("server").unwrap_or_default();
    Path::new(&section.data_dir).join("timbre.db")
}

fn http_config(config: &Config) -> timbre_edge_http::HttpConfig {
    let section: HttpSection = config.section("http").unwrap_or_default();
    let defaults = timbre_edge_http::HttpConfig::default();
    timbre_edge_http::HttpConfig {
        listen: section.listen.unwrap_or(defaults.listen),
        trusted_proxies: section.trusted_proxies.unwrap_or(defaults.trusted_proxies),
        max_body_bytes: section.max_body_bytes.unwrap_or(defaults.max_body_bytes),
        body_read_timeout: defaults.body_read_timeout,
    }
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
fn fatal(event: &LogEvent, failure: &dyn Failure) -> ExitCode {
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
    ExitCode::FAILURE
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
    ExitCode::FAILURE
}

/// The async executor could not start: report the operating system's reason,
/// which the failure does not carry, next to the failure's own code.
fn fatal_runtime(cause: &std::io::Error) -> ExitCode {
    let failure = failures::RuntimeFailure;
    let reason = cause.to_string();
    let kernel = Registry::new().finish(
        Logger::stdout(LogFilter::default()),
        Arc::new(ThreadRuntime::new()),
    );
    log!(
        Ctx::root(&kernel),
        error,
        RUNTIME_FAILED,
        code = failure.code().as_str(),
        message = failure.public_message(),
        reason = reason.as_str()
    );
    ExitCode::FAILURE
}
