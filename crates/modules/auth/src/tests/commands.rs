//! What the `user` and `device` commands write, publish and refuse.

use super::Fixture;
use crate::failures::{EXISTS, NOT_FOUND, UNKNOWN_PERMISSION};
use timbre_kernel::failures::{OUTPUT_FAILED, USAGE};

#[test]
fn adding_a_user_reports_it_and_publishes_it() {
    let auth = Fixture::new();
    auth.run("user add alice").unwrap();
    assert!(
        auth.log()
            .contains(r#""event":"auth.user_added","name":"alice","admin":false"#),
        "{}",
        auth.log()
    );
    assert_eq!(auth.signals(), vec!["UserAdded user=alice".to_owned()]);
    assert_eq!(
        auth.run("user add Alice").unwrap_err().code(),
        EXISTS,
        "names are canonical, so Alice is alice"
    );
}

#[test]
fn an_admin_user_is_published_as_one() {
    let auth = Fixture::new();
    auth.run("user add root --admin").unwrap();
    assert_eq!(
        auth.signals(),
        vec![
            "UserAdded user=root".to_owned(),
            "PermissionsChanged user=root granted=[] revoked=[] admin=true".to_owned(),
        ]
    );
}

#[test]
fn listing_users_shows_what_they_hold() {
    let auth = Fixture::new();
    auth.run("user add alice").unwrap();
    auth.run("user grant alice other.listen").unwrap();
    auth.run("user add root --admin").unwrap();
    auth.run("user list").unwrap();
    let output = auth.output.text();
    assert!(
        output.contains(r#""alice"  admin=false  devices=0  permissions=other.listen"#),
        "{output}"
    );
    assert!(
        output.contains(r#""root"  admin=true  devices=0  permissions=auth.admin,other.listen"#),
        "{output}"
    );
}

#[test]
fn granting_a_permission_no_module_declares_is_refused() {
    let auth = Fixture::new();
    auth.run("user add alice").unwrap();
    let failure = auth.run("user grant alice library.scan").unwrap_err();
    assert_eq!(failure.code(), UNKNOWN_PERMISSION);
    assert!(
        auth.dump("SELECT permission FROM auth_user_permissions")
            .is_empty(),
        "nothing is stored for a permission nothing checks"
    );
}

#[test]
fn an_orphaned_permission_can_be_revoked_but_not_granted() {
    let auth = Fixture::new();
    auth.run("user add alice").unwrap();
    auth.write("INSERT INTO auth_user_permissions(user_id,permission) SELECT id,'removed.read' FROM auth_users");
    auth.run("user revoke alice removed.read").unwrap();
    assert_eq!(
        auth.dump("SELECT permission FROM auth_user_permissions"),
        Vec::<String>::new()
    );
    assert!(
        auth.signals()
            .iter()
            .any(|line| line.contains(r#"revoked=["removed.read"]"#))
    );
    assert_eq!(
        auth.run("user grant alice removed.read")
            .unwrap_err()
            .code(),
        UNKNOWN_PERMISSION
    );
}

#[test]
fn granting_and_revoking_publish_only_real_changes() {
    let auth = Fixture::new();
    auth.run("user add alice").unwrap();
    auth.run("user grant alice other.listen").unwrap();
    auth.run("user grant alice other.listen").unwrap();
    auth.run("user revoke alice other.listen").unwrap();
    let changes: Vec<String> = auth
        .signals()
        .into_iter()
        .filter(|signal| signal.starts_with("PermissionsChanged"))
        .collect();
    assert_eq!(
        changes,
        vec![
            r#"PermissionsChanged user=alice granted=["other.listen"] revoked=[] admin=false"#
                .to_owned(),
            r#"PermissionsChanged user=alice granted=[] revoked=["other.listen"] admin=false"#
                .to_owned(),
        ]
    );
}

#[test]
fn a_device_list_shows_no_digest_and_no_secret() {
    let auth = Fixture::new();
    let secret = auth.device("alice", "laptop");
    let digest = auth.dump("SELECT digest FROM auth_devices");
    let prior = auth.output.text().len();
    auth.run("device list").unwrap();
    auth.run("device list alice").unwrap();
    let output = auth.output.text();
    let listing = &output[prior..];
    assert!(listing.contains(r#"label="laptop""#), "{listing}");
    assert!(!listing.contains(secret.expose()));
    assert!(!listing.contains("digest"));
    for stored in digest {
        assert!(!listing.contains(&stored));
    }
}

#[test]
fn an_expiry_is_the_configured_default_unless_the_command_says_otherwise() {
    let auth = Fixture::with_config("[auth]\ndevice_default_expiry_days = 2\n");
    auth.run("user add alice").unwrap();
    auth.issue("device issue alice laptop");
    auth.issue("device issue alice phone --expires 10");
    let spans = auth.dump(
        "SELECT label || ':' || ((expires-created)/86400000) FROM auth_devices ORDER BY label",
    );
    assert_eq!(spans, vec!["laptop:2".to_owned(), "phone:10".to_owned()]);
}

#[test]
fn a_device_with_no_configured_default_never_expires() {
    let auth = Fixture::new();
    auth.device("alice", "laptop");
    assert_eq!(
        auth.dump("SELECT expires FROM auth_devices"),
        vec![String::new()]
    );
}

#[test]
fn every_command_refuses_a_line_it_cannot_read() {
    let auth = Fixture::new();
    auth.run("user add alice").unwrap();
    for line in [
        "user add",
        "user add alice --root",
        "user list extra",
        "user remove",
        "user grant alice",
        "user revoke alice other.listen extra",
        "device issue alice",
        "device issue alice laptop --expires 0",
        "device issue alice laptop --expires soon",
        "device reissue",
        "device list a b",
        "device revoke",
    ] {
        assert_eq!(
            auth.run(line).unwrap_err().code(),
            USAGE,
            "`{line}` must report a usage failure"
        );
    }
    for line in [
        "user remove bob",
        "device reissue nothing",
        "device revoke nothing",
    ] {
        assert_eq!(
            auth.run(line).unwrap_err().code(),
            NOT_FOUND,
            "for `{line}`"
        );
    }
}

#[test]
fn labels_and_expiry_are_bounded() {
    let auth = Fixture::new();
    auth.run("user add alice").unwrap();
    for label in [
        String::new(),
        "x\n".to_owned(),
        "\u{feff}".to_owned(),
        "é".repeat(65),
    ] {
        let args = ["device", "issue", "alice", &label].map(str::to_owned);
        assert_eq!(auth.run_args(&args).unwrap_err().code(), USAGE);
    }
    for days in ["", "+2", " 2", "2 ", "0", "36501", "٢", "4294967296"] {
        let args = ["device", "issue", "alice", "laptop", "--expires", days].map(str::to_owned);
        assert_eq!(auth.run_args(&args).unwrap_err().code(), USAGE);
    }
    let label = "é".repeat(64);
    let args = ["device", "issue", "alice", &label, "--expires", "36500"].map(str::to_owned);
    auth.run_args(&args).unwrap();
    assert_eq!(
        auth.dump("SELECT CAST((expires-created)/86400000 AS TEXT) FROM auth_devices"),
        vec!["36500"]
    );
}

#[test]
fn issuing_a_device_reports_a_broken_output_pipe() {
    struct Closed;
    impl std::io::Write for Closed {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::ErrorKind::BrokenPipe.into())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let auth = Fixture::with_output("", Some(Box::new(Closed)));
    auth.run("user add alice").unwrap();
    assert_eq!(
        auth.run("device issue alice laptop").unwrap_err().code(),
        OUTPUT_FAILED
    );
}

#[test]
fn both_cli_lists_read_past_one_page() {
    let auth = Fixture::new();
    let total = timbre_kernel::MAX_PAGE_LIMIT + 1;
    auth.write(&format!(
        "WITH RECURSIVE seq(n) AS (VALUES (1) UNION ALL SELECT n+1 FROM seq WHERE n<{total})
         INSERT INTO auth_users (name,admin,created) SELECT printf('user%06d',n),0,0 FROM seq"
    ));
    auth.write(&format!(
        "WITH RECURSIVE seq(n) AS (VALUES (1) UNION ALL SELECT n+1 FROM seq WHERE n<{total})
         INSERT INTO auth_devices (id,user_id,label,prefix,digest,created)
         SELECT printf('device%06d',n),1,'label',printf('prefix%06d',n),zeroblob(32),0 FROM seq"
    ));
    auth.run("user list").unwrap();
    let output = auth.output.text();
    assert_eq!(output.lines().count(), total);
    assert!(output.contains(&format!("user{total:06}")));
    auth.run("device list user000001").unwrap();
    let output = auth.output.text();
    assert_eq!(output.lines().count(), total * 2);
    assert!(output.contains(&format!("device{total:06}")));
}

#[test]
fn directory_rejects_a_large_page_and_a_malformed_cursor_with_shared_codes() {
    use timbre_kernel::directory::Directory;
    use timbre_kernel::{Cursor, MAX_PAGE_LIMIT, PageRequest};

    let auth = Fixture::new();
    let ctx = auth.ctx();
    let directory = ctx.ports().get::<dyn Directory>().unwrap();
    let invalid = Cursor::new("not an offset");
    for (limit, code) in [
        (MAX_PAGE_LIMIT + 1, "kernel.page_too_large"),
        (1, "kernel.cursor_invalid"),
    ] {
        let failure = directory
            .users(
                ctx,
                PageRequest {
                    limit,
                    cursor: Some(&invalid),
                },
            )
            .unwrap_err();
        assert_eq!(failure.code().as_str(), code);
    }
}

#[test]
fn directory_validates_only_visible_rows_and_logs_an_invalid_name_once() {
    use timbre_kernel::directory::Directory;
    use timbre_kernel::{PageRequest, UserName};

    let auth = Fixture::new();
    auth.run("user add alice").unwrap();
    auth.device("bob", "phone");
    let id = auth.last_device();
    auth.write("UPDATE auth_users SET name='z?bad' WHERE name='bob'");
    let ctx = auth.ctx();
    let port = ctx.ports().get::<dyn Directory>().unwrap();
    let first = port
        .users(
            ctx,
            PageRequest {
                limit: 1,
                cursor: None,
            },
        )
        .unwrap();
    assert_eq!(first.items[0].name.as_str(), "alice");
    assert!(first.next.is_some());
    assert!(!auth.log().contains("auth.invalid_stored_name"));
    let second = port
        .users(
            ctx,
            PageRequest {
                limit: 1,
                cursor: first.next.as_ref(),
            },
        )
        .unwrap();
    assert_eq!(second.items, Vec::new());
    assert!(second.next.is_none());
    assert_eq!(port.owner_of_device(ctx, &id).unwrap(), None);
    assert_eq!(
        port.user(ctx, &UserName::normalize("bob").unwrap())
            .unwrap(),
        None
    );
    assert_eq!(auth.log().matches("auth.invalid_stored_name").count(), 1);
}

#[test]
fn diagnostics_keep_user_input_out_of_public_messages() {
    let auth = Fixture::new();
    auth.run("user add alice").unwrap();
    for (line, value) in [
        ("user add alice", "alice"),
        ("user remove missing", "missing"),
        ("device revoke unknown-id", "unknown-id"),
        ("user grant alice unknown.perm", "unknown.perm"),
    ] {
        let error = auth.run(line).unwrap_err();
        assert!(!error.public_message().contains(value));
    }
    assert_eq!(
        crate::failures::AuthFailure::NotConfigured.boxed().status(),
        503
    );
}
