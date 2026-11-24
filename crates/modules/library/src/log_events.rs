//! The library module's log events.

use timbre_kernel::LogEvent;

/// Identity of the startup scan job.
pub const SCAN_RUN: LogEvent = LogEvent::new("library.scan_run");
/// Identity of the filesystem observation job.
pub const WATCH: LogEvent = LogEvent::new("library.watch");
/// A reconcile has entered the scanner.
pub const SCAN_STARTED: LogEvent = LogEvent::new("library.scan_started");
/// Progress on a reconcile running for at least five seconds.
pub const SCAN_PROGRESS: LogEvent = LogEvent::new("library.scan_progress");
/// Result, counters and duration of a completed reconcile.
pub const SCAN_FINISHED: LogEvent = LogEvent::new("library.scan_finished");
/// A file or folder was not readable.
pub const FILE_UNREADABLE: LogEvent = LogEvent::new("library.file_unreadable");
/// How many entries of one `kind` a scan could not read, and how many of
/// them were logged in full before the budget ran out.
pub const UNREADABLE_SUMMARY: LogEvent = LogEvent::new("library.unreadable_summary");
/// Multiple audio paths matched the same potential relink key.
pub const RELINK_AMBIGUOUS: LogEvent = LogEvent::new("library.relink_ambiguous");
/// The filesystem backend lost notifications; the next scan reconciles them.
pub const WATCH_OVERFLOW: LogEvent = LogEvent::new("library.watch_overflow");
/// An asynchronous user-initiated scan failed.
pub const SCAN_FAILED: LogEvent = LogEvent::new("library.scan_failed");
/// The scanned source no longer contains the requested artwork bytes.
pub const ART_SOURCE_STALE: LogEvent = LogEvent::new("library.art_source_stale");
