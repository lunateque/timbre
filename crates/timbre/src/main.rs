//! The composition root: read the log filter, register the built-in modules,
//! publish `Started`, exit. Serving arrives with the HTTP edge in a later phase.

mod log_events;
mod modules;

use log_events::{LOG_FILTER_INVALID, REGISTRATION_FAILED, STARTED};
use std::env::VarError;
use std::process::ExitCode;
use timbre_kernel::signals::Started;
use timbre_kernel::{Ctx, FailureCode, LogFilter, Logger, Registry, failures, log};

fn main() -> ExitCode {
    let filter = match filter_from_env() {
        Ok(filter) => filter,
        Err((raw, code)) => return reject_filter(&raw, code),
    };
    let registry = match modules::registry() {
        Ok(registry) => registry,
        Err(failure) => {
            // The filter parsed, so the failure line honors it.
            let kernel = Registry::new().finish(Logger::stdout(filter));
            log!(
                Ctx::root(&kernel),
                error,
                REGISTRATION_FAILED,
                code = failure.code().as_str()
            );
            return ExitCode::FAILURE;
        }
    };
    let kernel = registry.finish(Logger::stdout(filter));
    let ctx = Ctx::root(&kernel);
    log!(ctx, info, STARTED);
    ctx.publish(&Started);
    ExitCode::SUCCESS
}

/// The filter named by `TIMBRE_LOG`; on `Err` the raw value and the code that
/// explain the rejection, for a line that must itself not depend on the filter.
fn filter_from_env() -> Result<LogFilter, (String, FailureCode)> {
    let raw = match std::env::var("TIMBRE_LOG") {
        Err(VarError::NotPresent) => return Ok(LogFilter::default()),
        Err(VarError::NotUnicode(_)) => {
            return Err(("<not unicode>".to_owned(), failures::LOG_FILTER_INVALID));
        }
        Ok(raw) => raw,
    };
    LogFilter::parse(&raw).map_err(|failure| (raw, failure.code()))
}

fn reject_filter(raw: &str, code: FailureCode) -> ExitCode {
    // Nothing trustworthy exists yet: log with the default filter and say why.
    let kernel = Registry::new().finish(Logger::stdout(LogFilter::default()));
    log!(
        Ctx::root(&kernel),
        error,
        LOG_FILTER_INVALID,
        value = raw,
        code = code.as_str()
    );
    ExitCode::FAILURE
}
