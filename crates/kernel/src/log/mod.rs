//! The structured logger: levels, named events, filters, formatting, output.

mod filter;
mod format;
mod logger;
mod time;
mod value;

#[cfg(test)]
pub(crate) use logger::tests as log_tests;

pub use filter::LogFilter;
pub use logger::{Clock, Format, Logger};
pub use value::{LogValue, Value};

use crate::names;

/// Log severity. Declaration order is severity order: a line is written when
/// its level is at or below the configured threshold.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Level {
    /// Something is broken.
    Error,
    /// Something unexpected happened, but the server continues.
    Warn,
    /// Normal operation worth recording.
    Info,
    /// Developer detail.
    Debug,
    /// Developer detail for hot paths.
    Trace,
}

impl Level {
    /// The lowercase word used in output.
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warn => "warn",
            Self::Info => "info",
            Self::Debug => "debug",
            Self::Trace => "trace",
        }
    }
}

/// A named log event, shaped `<module>.<snake_case_name>`, declared once per
/// event in the module's `log_events.rs`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct LogEvent(&'static str);

impl LogEvent {
    /// Build an event from a name, returning `None` when the name is invalid.
    pub const fn try_new(event: &'static str) -> Option<Self> {
        if names::is_valid(event) {
            Some(Self(event))
        } else {
            None
        }
    }

    /// Build an event from a name, failing the build where the constant is
    /// declared when the name is invalid.
    ///
    /// ```compile_fail
    /// const BAD: timbre_kernel::LogEvent = timbre_kernel::LogEvent::new("Bad");
    /// ```
    ///
    /// # Panics
    ///
    /// When `event` is not shaped `<module>.<snake_case_name>`; in a `const`
    /// context this is a compile-time error at the declaration.
    #[expect(
        clippy::panic,
        reason = "an invalid event name must fail the build where the constant is declared"
    )]
    pub const fn new(event: &'static str) -> Self {
        match Self::try_new(event) {
            Some(event) => event,
            None => panic!("log event must be <module>.<snake_case_name>"),
        }
    }

    /// The event as written at its declaration site.
    pub const fn as_str(self) -> &'static str {
        self.0
    }

    /// The module prefix of the event.
    pub fn module(self) -> &'static str {
        names::module_of(self.0)
    }
}

/// Emit one structured log line. Field expressions are evaluated only when the
/// line is enabled, so building fields for a disabled line costs nothing.
///
/// ```text
/// log!(ctx, info, GREETED, greeting = "hello", ms = elapsed)
/// ```
#[macro_export]
macro_rules! log {
    ($ctx:expr, $level:ident, $event:expr $(, $key:ident = $value:expr)* $(,)?) => {{
        let ctx: &$crate::Ctx = &$ctx;
        let level = $crate::__level!($level);
        let event: &$crate::LogEvent = &$event;
        if ctx.log_enabled(level, event) {
            ctx.log(
                level,
                event,
                &[$((stringify!($key), &$value as &dyn $crate::LogValue)),*],
            );
        }
    }};
}

/// Map the bare level identifier accepted by `log!` to a [`Level`] variant.
#[doc(hidden)]
#[macro_export]
macro_rules! __level {
    (error) => {
        $crate::Level::Error
    };
    (warn) => {
        $crate::Level::Warn
    };
    (info) => {
        $crate::Level::Info
    };
    (debug) => {
        $crate::Level::Debug
    };
    (trace) => {
        $crate::Level::Trace
    };
}
