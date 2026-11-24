//! The log events of `audit`.

use timbre_kernel::LogEvent;

/// Events were written to the journal.
pub const WRITTEN: LogEvent = LogEvent::new("audit.written");
/// The queue was full and events were lost; the count says how many since
/// the last pass. This is the soft guarantee the design accepts: the server
/// keeps serving and says what it could not record.
pub const DROPPED: LogEvent = LogEvent::new("audit.dropped");
/// Rows older than the retention window were deleted.
pub const PRUNED: LogEvent = LogEvent::new("audit.pruned");
/// A drain pass failed; the queue keeps what it still holds.
pub const DRAIN_FAILED: LogEvent = LogEvent::new("audit.drain_failed");
/// A retention pass failed; the next one tries again.
pub const RETENTION_FAILED: LogEvent = LogEvent::new("audit.retention_failed");
/// An audit command flag or value could not be read.
pub const INVALID_ARGUMENT: LogEvent = LogEvent::new("audit.invalid_argument");
