//! Recording when a device was last seen, off the request path.

use super::Fixture;
use crate::lastuse::flush;

#[test]
fn many_checks_within_a_minute_store_one_update() {
    let auth = Fixture::new();
    let secret = auth.device("alice", "laptop");
    for _ in 0..5 {
        auth.identity(&secret);
    }
    // Nothing is written while requests are being served.
    assert_eq!(
        auth.dump("SELECT last_used FROM auth_devices"),
        vec![String::new()],
        "the request path must not write"
    );
    flush(auth.ctx()).unwrap();
    let first = auth.dump("SELECT last_used FROM auth_devices");
    assert_ne!(first, vec![String::new()]);
    for _ in 0..5 {
        auth.identity(&secret);
    }
    flush(auth.ctx()).unwrap();
    assert_eq!(
        auth.dump("SELECT last_used FROM auth_devices"),
        first,
        "a second minute has not started, so the stored time stands"
    );
    assert_eq!(
        auth.log()
            .matches(r#""event":"auth.last_use_written""#)
            .count(),
        1,
        "only the pass that had something to write reports one"
    );
}

#[test]
fn a_flush_with_nothing_pending_does_nothing() {
    let auth = Fixture::new();
    flush(auth.ctx()).unwrap();
    assert!(
        !auth.log().contains("auth.last_use_written"),
        "{}",
        auth.log()
    );
}

#[test]
fn only_devices_used_in_the_last_minute_are_remembered() {
    let auth = Fixture::new();
    let secret = auth.device("alice", "laptop");
    auth.identity(&secret);
    let state = crate::state(auth.ctx()).unwrap();
    assert_eq!(state.activity().tracked(), 1);
    flush(auth.ctx()).unwrap();
    assert_eq!(
        state.activity().tracked(),
        1,
        "the device stays while its minute runs"
    );
    // A drain more than a minute later forgets it, so the map holds only
    // devices that were used recently.
    let later = std::time::Instant::now() + std::time::Duration::from_secs(120);
    state.activity().take(later);
    assert_eq!(state.activity().tracked(), 0);
}

#[test]
fn a_failed_drain_retries_the_same_note() {
    let auth = Fixture::new();
    let key = auth.device("alice", "laptop");
    auth.identity(&key);
    let shared = crate::config::shared_db(auth.ctx()).unwrap();
    let database = shared.get(auth.ctx()).unwrap();
    database
        .write(|sql| {
            crate::store::exec(
                sql,
                "CREATE TRIGGER refuse_activity BEFORE UPDATE OF last_used ON auth_devices \
            BEGIN SELECT RAISE(ABORT, 'activity unavailable'); END",
                [],
            )?;
            Ok(())
        })
        .unwrap();
    assert!(flush(auth.ctx()).is_err());
    assert_eq!(
        auth.dump("SELECT last_used FROM auth_devices"),
        [String::new()]
    );
    database
        .write(|sql| {
            crate::store::exec(sql, "DROP TRIGGER refuse_activity", [])?;
            Ok(())
        })
        .unwrap();
    flush(auth.ctx()).unwrap();
    assert_ne!(
        auth.dump("SELECT last_used FROM auth_devices"),
        [String::new()]
    );
}

#[test]
fn an_old_secret_note_does_not_reappear_after_reissue() {
    let auth = Fixture::new();
    let old = auth.device("alice", "laptop");
    auth.identity(&old);
    let new = auth.issue(&format!("device reissue {}", auth.last_device()));
    flush(auth.ctx()).unwrap();
    assert_eq!(
        auth.dump("SELECT last_used FROM auth_devices"),
        [String::new()]
    );
    auth.identity(&new);
    flush(auth.ctx()).unwrap();
    assert_ne!(
        auth.dump("SELECT last_used FROM auth_devices"),
        [String::new()]
    );
}
