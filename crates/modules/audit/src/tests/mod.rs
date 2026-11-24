use crate::queue::Entry;
use std::net::{IpAddr, Ipv4Addr};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use timbre_edge_config::Config;
use timbre_edge_sqlite::SharedDatabase;
use timbre_kernel::clock::{DAY_MILLIS, now_unix_millis};
use timbre_kernel::credential::RejectReason;
use timbre_kernel::identity::{Permission, UserName};
use timbre_kernel::signals::{
    AccessDenied, AuthFailed, DeviceIssued, DeviceRevoked, PermissionsChanged, UserAdded,
    UserRemoved,
};
use timbre_kernel::testing::Shared;
use timbre_kernel::{
    Ctx, Fallible, Format, LogFilter, Logger, Registry, Resources, Secret, ThreadRuntime, block_on,
};

mod queueing;

const CLIENT: IpAddr = IpAddr::V4(Ipv4Addr::new(192, 0, 2, 10));
const LISTEN: Permission = Permission::new("other.listen");

static NEXT: AtomicUsize = AtomicUsize::new(0);

fn other_module(registry: &mut Registry) -> Fallible {
    registry.permission(LISTEN, "Listen to something")
}

struct Fixture {
    base: PathBuf,
    sink: Shared,
    output: Shared,
    ctx: Ctx,
    runtime: Arc<ThreadRuntime>,
}

impl Fixture {
    fn new() -> Self {
        Self::with_config("")
    }

    fn with_config(extra: &str) -> Self {
        Self::with_filter(extra, "debug")
    }

    fn with_filter(extra: &str, filter: &str) -> Self {
        let base = std::env::temp_dir().join(format!(
            "timbre-audit-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let config = base.join("config.toml");
        std::fs::write(&config, extra).unwrap();
        let sink = Shared::default();
        let output = Shared::default();
        let resources = Resources::new()
            .with(Arc::new(Config::load(Some(&config)).unwrap()))
            .unwrap()
            .with(Arc::new(SharedDatabase::new(
                base.join("timbre.db"),
                |_, _| Ok(()),
            )))
            .unwrap()
            .with(Arc::new(timbre_kernel::command::Output::new(Box::new(
                output.clone(),
            ))))
            .unwrap();
        let runtime = Arc::new(ThreadRuntime::new());
        let kernel = Registry::new()
            .module("audit", crate::register)
            .unwrap()
            .module("other", other_module)
            .unwrap()
            .finish_with_resources(
                Logger::new(
                    LogFilter::parse(filter).unwrap(),
                    Format::Json,
                    Box::new(sink.clone()),
                    std::time::SystemTime::now,
                ),
                runtime.clone(),
                resources,
            );
        kernel.check_config().unwrap();
        Self {
            base,
            sink,
            output,
            ctx: Ctx::root(&kernel).for_cli(),
            runtime,
        }
    }

    fn run(&self, line: &str) -> Fallible {
        let args: Vec<String> = line.split_whitespace().map(str::to_owned).collect();
        let (handler, rest) = self
            .ctx
            .kernel()
            .find_command(&args)
            .unwrap_or_else(|| panic!("no command matches `{line}`"));
        block_on(handler(self.ctx.clone(), rest))
    }

    fn log(&self) -> String {
        assert!(self.ctx.kernel().flush_log());
        self.sink.text()
    }

    /// Every journal row as `kind|user|client|detail`, oldest first.
    fn rows(&self) -> Vec<String> {
        let shared = crate::config::shared_db(&self.ctx).unwrap();
        shared
            .get(&self.ctx)
            .unwrap()
            .read(|sql| {
                let mut statement = sql
                    .prepare("SELECT kind,user,client,detail FROM audit_events ORDER BY id")
                    .unwrap();
                let rows = statement
                    .query_map([], |row| {
                        Ok(format!(
                            "{}|{}|{}|{}",
                            row.get::<_, String>(0)?,
                            row.get::<_, Option<String>>(1)?.unwrap_or_default(),
                            row.get::<_, Option<String>>(2)?.unwrap_or_default(),
                            row.get::<_, String>(3)?
                        ))
                    })
                    .unwrap()
                    .collect::<Result<Vec<_>, _>>()
                    .unwrap();
                Ok(rows)
            })
            .unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

fn user(text: &str) -> UserName {
    name(text)
}

fn name(text: &str) -> UserName {
    UserName::normalize(text).expect("a usable name")
}

#[test]
fn every_recorded_signal_becomes_one_row() {
    let audit = Fixture::new();
    audit.ctx.publish(&UserAdded {
        user: user("alice"),
    });
    audit.ctx.publish(&PermissionsChanged {
        user: user("alice"),
        granted: vec![LISTEN],
        revoked: Vec::new(),
        admin: false,
    });
    audit.ctx.publish(&DeviceIssued {
        user: user("alice"),
        device: "dev1".to_owned(),
        label: "laptop".to_owned(),
        secret: Secret::new("super-secret-value".to_owned()),
    });
    audit.ctx.publish(&DeviceRevoked {
        user: user("alice"),
        device: "dev1".to_owned(),
    });
    audit.ctx.publish(&AccessDenied {
        user: name("alice"),
        client: CLIENT,
        permission: LISTEN,
    });
    audit.ctx.publish(&AuthFailed {
        client: CLIENT,
        user: None,
        reason: RejectReason::Invalid,
        detail: timbre_kernel::RejectDetail::try_new("malformed"),
    });
    audit.ctx.publish(&UserRemoved {
        user: user("alice"),
    });
    let rows = audit.rows();
    assert_eq!(rows.len(), 7, "{rows:?}");
    assert_eq!(
        rows.first().map(String::as_str),
        Some("user_added|alice||{}")
    );
    assert_eq!(
        rows.get(1).map(String::as_str),
        Some(
            r#"permissions_changed|alice||{"admin":"false","granted":"other.listen","revoked":""}"#
        )
    );
    assert_eq!(
        rows.get(2).map(String::as_str),
        Some(r#"device_issued|alice||{"device":"dev1","label":"laptop"}"#)
    );
    assert_eq!(
        rows.get(4).map(String::as_str),
        Some(r#"access_denied|alice|192.0.2.10|{"permission":"other.listen"}"#)
    );
    assert_eq!(
        rows.get(5).map(String::as_str),
        Some(r#"auth_failed||192.0.2.10|{"detail":"malformed","reason":"invalid"}"#)
    );
    let stored = rows.join("\n");
    assert!(
        !stored.contains("super-secret-value") && !audit.log().contains("super-secret-value"),
        "the issued secret must not reach the journal"
    );
}

#[test]
fn a_full_queue_drops_events_and_says_how_many() {
    let audit = Fixture::with_filter("[audit]\nqueue_capacity = 2\n", "error");
    let queue = crate::state(&audit.ctx).unwrap();
    for index in 0..5 {
        queue.queue().push(
            Entry {
                time: 1,
                kind: "access_denied",
                user: None,
                client: None,
                detail: format!("{{\"n\":\"{index}\"}}"),
            },
            2,
        );
    }
    assert_eq!(queue.queue().waiting(), 2, "the queue stays bounded");
    crate::jobs::drain(&audit.ctx).unwrap();
    assert!(
        audit
            .log()
            .contains(r#""event":"audit.dropped","events":3"#),
        "{}",
        audit.log()
    );
    assert_eq!(audit.rows().len(), 2);
    // The count resets, so the next pass reports only new losses.
    crate::jobs::drain(&audit.ctx).unwrap();
    assert_eq!(audit.log().matches("audit.dropped").count(), 1);
}

#[test]
fn rows_past_the_retention_window_are_deleted() {
    let audit = Fixture::with_config("[audit]\nkeep_days = 1\n");
    let state = crate::state(&audit.ctx).unwrap();
    let old = now_unix_millis() - 3 * DAY_MILLIS;
    state.queue().push(
        Entry {
            time: old,
            kind: "auth_failed",
            user: None,
            client: None,
            detail: "{}".to_owned(),
        },
        16,
    );
    crate::jobs::drain(&audit.ctx).unwrap();
    audit.ctx.publish(&UserAdded {
        user: user("alice"),
    });
    assert_eq!(audit.rows().len(), 2);
    crate::jobs::prune(&audit.ctx).unwrap();
    let rows = audit.rows();
    assert_eq!(rows.len(), 1, "only the old row went: {rows:?}");
    assert!(
        audit.log().contains(r#""event":"audit.pruned","events":1"#),
        "{}",
        audit.log()
    );
}

#[test]
fn the_command_reports_what_its_filters_ask_for() {
    let audit = Fixture::with_filter("", "error");
    audit.ctx.publish(&UserAdded {
        user: user("alice"),
    });
    audit.ctx.publish(&UserAdded { user: user("bob") });
    audit.ctx.publish(&AuthFailed {
        client: CLIENT,
        user: Some(name("alice")),
        reason: RejectReason::Invalid,
        detail: None,
    });
    audit.run("audit --user alice").unwrap();
    let output = audit.output.text();
    assert_eq!(output.lines().count(), 2, "{output}");
    assert!(!output.contains("bob"), "{output}");

    let audit = Fixture::new();
    audit.ctx.publish(&UserAdded {
        user: user("alice"),
    });
    audit.ctx.publish(&AuthFailed {
        client: CLIENT,
        user: None,
        reason: RejectReason::Invalid,
        detail: None,
    });
    audit.run("audit --kind auth_failed --limit 1").unwrap();
    let output = audit.output.text();
    assert_eq!(output.lines().count(), 1, "{output}");
    assert!(output.contains("kind=auth_failed"), "{output}");
}

#[test]
fn a_since_date_hides_everything_older() {
    let audit = Fixture::new();
    audit.ctx.publish(&UserAdded {
        user: user("alice"),
    });
    audit.run("audit --since 1970-01-01").unwrap();
    assert_eq!(audit.output.text().lines().count(), 1);
    audit.run("audit --since 2999-01-01").unwrap();
    assert_eq!(
        audit.output.text().lines().count(),
        1,
        "nothing is newer than the far future"
    );
}

#[test]
fn the_command_refuses_a_line_it_cannot_read() {
    let audit = Fixture::new();
    for line in [
        "audit --since yesterday",
        "audit --limit 0",
        "audit --limit all",
        "audit --user ",
        "audit --nope x",
        "audit --limit",
    ] {
        assert_eq!(
            audit.run(line).unwrap_err().code(),
            timbre_kernel::failures::USAGE,
            "`{line}` must report a usage failure"
        );
    }
}
