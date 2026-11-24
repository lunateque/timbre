//! Configuration must be validated before dispatch, including built-in commands.

#[path = "../src/modules.rs"]
mod modules;

use std::process::Command;
use std::sync::Arc;
use timbre_kernel::{LogFilter, Logger, ThreadRuntime};

#[test]
fn a_misspelled_key_stops_startup_when_its_module_is_enabled() {
    let kernel = modules::registry().unwrap().finish(
        Logger::stdout(LogFilter::default()),
        Arc::new(ThreadRuntime::new()),
    );
    let audit_enabled = kernel
        .migrations()
        .iter()
        .any(|migration| migration.module() == "audit");
    let output = Command::new(env!("CARGO_BIN_EXE_timbre"))
        .env("TIMBRE_LOG", "info")
        .env("TIMBRE_AUDIT__DRAIN_SECOND", "2")
        .arg("help")
        .output()
        .unwrap();
    let log = String::from_utf8(output.stdout).unwrap();
    if audit_enabled {
        assert_eq!(output.status.code(), Some(1), "{log}");
        assert!(log.contains(r#""event":"timbre.config_failed""#), "{log}");
        assert!(log.contains(r#""code":"config.invalid""#), "{log}");
    } else {
        assert_eq!(output.status.code(), Some(0), "{log}");
    }
    // The command list only runs when the configuration passed its check.
    assert_eq!(log.contains("usage: timbre"), !audit_enabled, "{log}");
}
