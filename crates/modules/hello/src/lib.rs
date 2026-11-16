//! The example module: it exists to exercise the kernel contracts (a log
//! event, a failure, a signal handler) and to prove that a module can be
//! deleted with its directory. It goes away when the first real module lands.

pub mod failures;
pub mod log_events;

use log_events::GREETED;
use timbre_kernel::signals::Started;
use timbre_kernel::{Ctx, Fallible, Registry, log};

/// Register the module: one handler for `Started`.
///
/// # Errors
///
/// Never fails today; the signature is the module contract.
pub fn register(registry: &mut Registry) -> Fallible {
    registry.on::<Started>(greet);
    Ok(())
}

// Both signatures are fixed by `Handler<Started>`: the bus hands over a reference
// and every handler answers with `Fallible`.
#[expect(
    clippy::trivially_copy_pass_by_ref,
    clippy::unnecessary_wraps,
    reason = "the signature is the Handler<Started> type"
)]
fn greet(ctx: &Ctx, _: &Started) -> Fallible {
    log!(ctx, info, GREETED, greeting = "hello");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::{Arc, Mutex};
    use timbre_kernel::{Format, LogFilter, Logger};

    #[derive(Clone, Default)]
    struct Shared(Arc<Mutex<Vec<u8>>>);

    impl Write for Shared {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            // Lock errors mean another test panicked while writing; recovering
            // keeps this test's own assertion the one that matters.
            let mut guard = self
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            guard.extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl Shared {
        fn text(&self) -> String {
            let guard = self
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            String::from_utf8(guard.clone()).expect("logs are utf-8")
        }
    }

    #[test]
    fn started_publishes_one_greeting_line() {
        let sink = Shared::default();
        let registry = Registry::new().module(register).expect("hello registers");
        let logger = Logger::new(
            LogFilter::default(),
            Format::Json,
            Box::new(sink.clone()),
            || std::time::SystemTime::UNIX_EPOCH,
        );
        let kernel = registry.finish(logger);
        Ctx::root(&kernel).publish(&Started);
        assert_eq!(
            sink.text(),
            "{\"ts\":\"1970-01-01T00:00:00.000Z\",\"level\":\"info\",\"event\":\"hello.greeted\",\"greeting\":\"hello\"}\n"
        );
    }
}
