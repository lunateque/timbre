//! The hello module's log events.

use timbre_kernel::LogEvent;

/// The module greeted on the `Started` signal.
pub const GREETED: LogEvent = LogEvent::new("hello.greeted");
