//! What the `ApiKey` check accepts, what it refuses, and what it never says.

use super::{CLIENT, Fixture, OTHER};
use crate::tests::names;
use std::net::{IpAddr, Ipv6Addr};
use timbre_kernel::credential::RejectReason;
use timbre_kernel::{AuthOutcome, Secret};

#[test]
fn an_issued_secret_authenticates_as_its_user_with_exactly_its_permissions() {
    let auth = Fixture::new();
    let secret = auth.device("alice", "laptop");
    auth.run("user grant alice other.listen").unwrap();
    let identity = auth.identity(&secret);
    assert_eq!(identity.user.as_str(), "alice");
    assert_eq!(names(&identity.permissions), vec![OTHER.as_str()]);
}

#[test]
fn an_admin_holds_a_permission_another_module_declared() {
    let auth = Fixture::new();
    auth.run("user add root --admin").unwrap();
    let secret = auth.issue("device issue root console");
    let identity = auth.identity(&secret);
    let held = names(&identity.permissions);
    assert!(held.contains(&"auth.admin"), "{held:?}");
    assert!(
        held.contains(&OTHER.as_str()),
        "an admin holds what other modules declare: {held:?}"
    );
}

#[test]
fn a_revoked_device_stops_authenticating() {
    let auth = Fixture::new();
    let secret = auth.device("alice", "laptop");
    assert!(matches!(
        auth.authenticate(&secret),
        AuthOutcome::Recognized(_)
    ));
    auth.run(&format!("device revoke {}", auth.last_device()))
        .unwrap();
    assert_eq!(auth.rejection(&secret), RejectReason::Invalid);
    assert!(
        auth.log().contains("\"detail\":\"unknown\""),
        "{}",
        auth.log()
    );
}

#[test]
fn the_secret_a_reissue_replaced_stops_authenticating() {
    let auth = Fixture::new();
    let old = auth.device("alice", "laptop");
    let device = auth.last_device();
    let new = auth.issue(&format!("device reissue {device}"));
    assert_ne!(old.expose(), new.expose());
    assert!(matches!(
        auth.authenticate(&new),
        AuthOutcome::Recognized(_)
    ));
    assert_eq!(auth.rejection(&old), RejectReason::Invalid);
    assert_eq!(auth.last_device(), device, "a reissue keeps the device id");
}

#[test]
fn an_expired_device_is_refused_as_disabled() {
    let auth = Fixture::new();
    auth.run("user add alice").unwrap();
    let secret = auth.issue("device issue alice laptop --expires 1");
    // Reach into the stored row rather than wait a day: the rule under test
    // is the comparison against the expiry, not how it was computed.
    let shared = crate::config::shared_db(auth.ctx()).unwrap();
    shared
        .get(auth.ctx())
        .unwrap()
        .write(|sql| {
            crate::store::exec(sql, "UPDATE auth_devices SET expires=1", [])?;
            Ok(())
        })
        .unwrap();
    assert_eq!(auth.rejection(&secret), RejectReason::Disabled);
    assert!(
        auth.log().contains("\"detail\":\"expired\""),
        "{}",
        auth.log()
    );
}

#[test]
fn every_malformed_shape_and_an_unknown_prefix_are_refused() {
    for (shape, reason) in [
        ("empty", "malformed"),
        ("prefix only", "malformed"),
        ("no secret", "malformed"),
        ("doubled secret", "malformed"),
        ("uppercased", "malformed"),
        ("unknown prefix", "unknown"),
        ("wrong secret", "mismatch"),
    ] {
        // One fixture per shape: the log of a run holds one reason, so the
        // assertion cannot pass on an earlier attempt's line.
        let auth = Fixture::new();
        let secret = auth.device("alice", "laptop");
        let text = secret.expose().clone();
        let (prefix, rest) = text.split_once('_').unwrap();
        let presented = match shape {
            "empty" => String::new(),
            "prefix only" => prefix.to_owned(),
            "no secret" => format!("{prefix}_"),
            "doubled secret" => format!("{prefix}_{rest}{rest}"),
            "uppercased" => format!("{prefix}_{}", rest.to_uppercase()),
            "unknown prefix" => format!("aaaaaaaaaaaaa_{rest}"),
            _ => format!("{prefix}_{}", flip(rest)),
        };
        assert_eq!(
            auth.rejection(&Secret::new(presented.clone())),
            RejectReason::Invalid,
            "for a {shape} key"
        );
        let log = auth.log();
        assert!(
            log.contains(&format!("\"detail\":\"{reason}\"")),
            "expected {reason} for a {shape} key, log was {log}"
        );
        assert_eq!(log.matches(r#""event":"kernel.auth_failed""#).count(), 1);
        assert!(!log.contains("auth.key_rejected"));
    }
}

/// The same secret with one symbol changed: still canonical, still the right
/// shape, simply not the stored one.
fn flip(secret: &str) -> String {
    let mut flipped = secret.to_owned();
    let replacement = if secret.starts_with('a') { "b" } else { "a" };
    flipped.replace_range(0..1, replacement);
    flipped
}

#[test]
fn a_removed_users_secrets_stop_working() {
    let auth = Fixture::new();
    let secret = auth.device("alice", "laptop");
    auth.run("user remove alice").unwrap();
    assert_eq!(auth.rejection(&secret), RejectReason::Invalid);
    assert!(
        auth.dump("SELECT id FROM auth_devices").is_empty(),
        "removing a user deletes their devices"
    );
    let signals = auth.signals().join("\n");
    assert!(signals.contains("DeviceRevoked"), "{signals}");
    assert!(signals.contains("UserRemoved user=alice"), "{signals}");
}

#[test]
fn no_secret_reaches_the_log_or_a_rejection_signal() {
    let auth = Fixture::new();
    let secret = auth.device("alice", "laptop");
    let text = secret.expose().clone();
    auth.identity(&secret);
    assert_eq!(
        auth.rejection(&Secret::new(flip(&text))),
        RejectReason::Invalid
    );
    let log = auth.log();
    assert!(!log.contains(&text), "the secret reached the log: {log}");
    let (prefix, rest) = text.split_once('_').unwrap();
    assert!(!log.contains(rest), "the secret reached the log: {log}");
    for signal in auth.signals() {
        if signal.starts_with("DeviceIssued") {
            // The issuing signal is where the secret travels in process.
            continue;
        }
        assert!(!signal.contains(rest), "a secret reached {signal}");
    }
    assert!(
        auth.signals()
            .iter()
            .any(|signal| signal.starts_with("AuthFailed") && signal.contains("reason=invalid")),
        "the kernel reports the rejection: {:?}",
        auth.signals()
    );
    assert!(
        !auth
            .dump("SELECT prefix,digest FROM auth_devices")
            .join("|")
            .contains(rest),
        "the secret must not be stored"
    );
    assert!(
        auth.dump("SELECT prefix FROM auth_devices") == vec![prefix.to_owned()],
        "the prefix is a lookup handle and is stored"
    );
}

#[test]
fn the_unknown_prefix_path_does_the_same_work_as_a_mismatch() {
    let auth = Fixture::new();
    let secret = auth.device("alice", "laptop");
    let text = secret.expose().clone();
    let (prefix, rest) = text.split_once('_').unwrap();
    let before = auth.work();
    assert_eq!(
        auth.rejection(&Secret::new(format!("aaaaaaaaaaaaa_{rest}"))),
        RejectReason::Invalid
    );
    let unknown = auth.work();
    assert_eq!(
        auth.rejection(&Secret::new(format!("{prefix}_{}", flip(rest)))),
        RejectReason::Invalid
    );
    let mismatch = auth.work();
    assert_eq!(
        (unknown.0 - before.0, unknown.1 - before.1),
        (mismatch.0 - unknown.0, mismatch.1 - unknown.1),
        "an unknown prefix must cost the same digests and comparisons as a wrong secret"
    );
    assert_eq!(unknown.0 - before.0, 1, "exactly one digest per attempt");
    assert_eq!(
        unknown.1 - before.1,
        1,
        "exactly one comparison per attempt"
    );
    let malformed = auth.work();
    assert_eq!(
        auth.rejection(&Secret::new("nonsense".to_owned())),
        RejectReason::Invalid
    );
    let after = auth.work();
    assert_eq!(
        (after.0 - malformed.0, after.1 - malformed.1),
        (1, 1),
        "a malformed key costs the same as a miss"
    );
}

#[test]
fn the_client_address_of_a_check_reaches_the_provider() {
    let auth = Fixture::new();
    let secret = auth.device("alice", "laptop");
    let client = IpAddr::V6(Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 7));
    assert!(matches!(
        auth.authenticate_from(&secret, client),
        AuthOutcome::Recognized(_)
    ));
    crate::lastuse::flush(auth.ctx()).unwrap();
    assert_eq!(
        auth.dump("SELECT last_client FROM auth_devices"),
        vec![client.to_string()]
    );
    assert_ne!(client.to_string(), CLIENT.to_string());
}
