use super::*;
use std::time::{Duration, Instant};
use timbre_edge_sqlite::failures::query_error;
use timbre_kernel::{RejectDetail, RequestId};

#[test]
fn journal_batches_duplicates_and_reserves_room_for_other_kinds() {
    let audit = Fixture::with_config("[audit]\nqueue_capacity = 2\n");
    let req = Ctx::root(audit.ctx.kernel()).with_request(RequestId::new(1));
    let event = AuthFailed {
        client: CLIENT,
        user: None,
        reason: RejectReason::Invalid,
        detail: None,
    };
    req.publish(&event);
    req.publish(&event);
    req.publish(&AuthFailed {
        detail: RejectDetail::try_new("malformed"),
        ..event
    });
    req.publish(&UserAdded {
        user: user("alice"),
    });
    crate::jobs::drain(&audit.ctx).unwrap();
    assert_eq!(
        audit.rows(),
        [
            r#"auth_failed||192.0.2.10|{"reason":"invalid"}"#,
            "user_added|alice||{}",
        ]
    );
    let shared = crate::config::shared_db(&audit.ctx).unwrap();
    let database = shared.get(&audit.ctx).unwrap();
    let counts = database
        .read(|sql| {
            sql.query_row(
                "SELECT count FROM audit_events WHERE kind='auth_failed'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .map_err(query_error)
        })
        .unwrap();
    assert_eq!(counts, 2);
}

#[test]
fn a_failed_write_requeues_the_oldest_batch_ahead_of_new_entries() {
    let audit = Fixture::with_config("[audit]\nqueue_capacity = 2\n");
    let req = Ctx::root(audit.ctx.kernel()).with_request(RequestId::new(1));
    req.publish(&UserAdded {
        user: user("first"),
    });
    let shared = crate::config::shared_db(&audit.ctx).unwrap();
    let database = shared.get(&audit.ctx).unwrap();
    database
        .write(|sql| {
            sql.execute_batch(
                "CREATE TRIGGER refuse_audit BEFORE INSERT ON audit_events \
            BEGIN SELECT RAISE(ABORT, 'audit unavailable'); END",
            )
            .map_err(query_error)
        })
        .unwrap();
    assert!(crate::jobs::drain(&audit.ctx).is_err());
    req.publish(&UserAdded {
        user: user("second"),
    });
    req.publish(&UserAdded {
        user: user("third"),
    });
    database
        .write(|sql| {
            sql.execute_batch("DROP TRIGGER refuse_audit")
                .map_err(query_error)
        })
        .unwrap();
    crate::jobs::drain(&audit.ctx).unwrap();
    assert_eq!(
        audit.rows(),
        ["user_added|first||{}", "user_added|second||{}"]
    );
    assert!(
        audit
            .log()
            .contains(r#""event":"audit.dropped","events":1"#)
    );
}

#[test]
fn the_final_drain_writes_rows_from_requests_that_finish_after_job_shutdown() {
    let audit = Fixture::with_config("[audit]\ndrain_seconds = 60\n");
    let req = Ctx::root(audit.ctx.kernel()).with_request(RequestId::new(1));
    req.publish(&UserAdded {
        user: user("first"),
    });
    let job = crate::jobs::draining(audit.ctx.clone());
    let worker = std::thread::spawn(move || block_on(job));
    let by = Instant::now() + Duration::from_secs(5);
    while audit.rows().is_empty() && Instant::now() < by {
        std::thread::sleep(Duration::from_millis(10));
    }
    let initial = audit.rows();
    audit.runtime.trigger_shutdown();
    worker.join().unwrap().unwrap();
    req.publish(&UserAdded {
        user: user("second"),
    });
    Ctx::root(audit.ctx.kernel()).publish(&timbre_kernel::signals::ServerDrained);
    assert_eq!(initial, ["user_added|first||{}"]);
    assert_eq!(
        audit.rows(),
        ["user_added|first||{}", "user_added|second||{}"]
    );
}

#[test]
fn work_without_a_request_but_outside_cli_is_still_buffered() {
    let audit = Fixture::new();
    let root = Ctx::root(audit.ctx.kernel());
    root.publish(&UserAdded {
        user: user("alice"),
    });
    assert_eq!(audit.rows(), Vec::<String>::new());
    crate::jobs::drain(&root).unwrap();
    assert_eq!(audit.rows(), ["user_added|alice||{}"]);
}
