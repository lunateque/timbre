use super::{
    GREET, LEGACY, authorized, body_text, denied, granted, harness, matched, ran, request,
};
use crate::LogFilter;
use crate::credential::{CredentialKind, RejectReason, Verdict};
use crate::guard::Guard;
use crate::guard::tests::Denials;
use crate::log_events::{ACCESS_DENIED, AUTH_FAILED};
use crate::testing::json_field;
use std::sync::atomic::Ordering;

#[test]
fn a_public_guard_yields_an_authorized_without_a_principal() {
    let (kernel, _sink, _asked, _denials) = harness(
        &[(
            "first",
            CredentialKind::ApiKey,
            Verdict::Recognized(granted(GREET)),
        )],
        None,
        LogFilter::default(),
    );
    let found = authorized(&kernel, "/public", &request(&[("k", "s")]));
    assert!(found.user().is_none(), "a public route earns no principal");
    assert_eq!(ran(found, &request(&[])), "ran:nobody");
}

#[test]
fn an_accepted_credential_gives_the_handler_the_principal() {
    let (kernel, _sink, asked, _denials) = harness(
        &[(
            "first",
            CredentialKind::ApiKey,
            Verdict::Recognized(granted(GREET)),
        )],
        None,
        LogFilter::default(),
    );
    let found = authorized(&kernel, "/me", &request(&[("k", "s")]));
    assert_eq!(
        found.user().map(ToString::to_string).as_deref(),
        Some("tester"),
        "the edge sees the canonical name"
    );
    assert_eq!(ran(found, &request(&[("k", "s")])), "ran:tester:greet=true");
    assert_eq!(asked.load(Ordering::SeqCst), 1);
}

#[test]
fn a_missing_or_conflicting_credential_denies_without_asking_the_provider() {
    let (kernel, _sink, asked, _denials) = harness(
        &[(
            "first",
            CredentialKind::ApiKey,
            Verdict::Recognized(granted(GREET)),
        )],
        None,
        LogFilter::default(),
    );
    let denial = denied(&kernel, "/me", &request(&[]));
    assert_eq!(denial.status, 401);
    assert_eq!(body_text(&denial), "missing");
    let denial = denied(&kernel, "/me", &request(&[("k", "s"), ("p", "x")]));
    assert_eq!(denial.status, 400);
    assert_eq!(body_text(&denial), "conflict");
    assert_eq!(
        asked.load(Ordering::SeqCst),
        0,
        "the provider was never asked"
    );
}

#[test]
fn a_kind_without_an_owner_denies_unsupported_naming_the_kind() {
    let (kernel, _sink, asked, _denials) = harness(&[], None, LogFilter::default());
    let denial = denied(&kernel, "/me", &request(&[("k", "s")]));
    assert_eq!(denial.status, 501);
    let text = body_text(&denial);
    assert!(
        text.contains("ApiKey"),
        "the denial names what the client offered: {text}"
    );
    assert_eq!(asked.load(Ordering::SeqCst), 0);
}

#[test]
fn a_rejected_credential_denies_without_saying_why() {
    // An invalid secret and a disabled account answer the same way: the
    // wire must not leak which of them happened.
    let (kernel, _sink, _asked, _denials) = harness(
        &[(
            "first",
            CredentialKind::Password,
            Verdict::Rejected(RejectReason::Invalid, None),
        )],
        None,
        LogFilter::default(),
    );
    let denial = denied(&kernel, "/me", &request(&[("u", "tester"), ("p", "wrong")]));
    assert_eq!(denial.status, 401);
    assert_eq!(body_text(&denial), "rejected");
    let (kernel, _sink, _asked, _denials) = harness(
        &[(
            "first",
            CredentialKind::Password,
            Verdict::Rejected(RejectReason::Disabled, None),
        )],
        None,
        LogFilter::default(),
    );
    let denial = denied(&kernel, "/me", &request(&[("u", "tester"), ("p", "x")]));
    assert_eq!(denial.status, 401);
    assert_eq!(
        body_text(&denial),
        "rejected",
        "a disabled account is not distinguishable"
    );
}

#[test]
fn a_throttled_credential_denies_as_throttled() {
    // Same credential, now behind a throttle that refuses everything: the
    // provider is not even asked, and the protocol still gets its 429 shape.
    let (kernel, _sink, asked, _denials) = harness(
        &[(
            "first",
            CredentialKind::Password,
            Verdict::Recognized(granted(GREET)),
        )],
        Some("limiter"),
        LogFilter::default(),
    );
    let denial = denied(&kernel, "/me", &request(&[("u", "tester"), ("p", "right")]));
    assert_eq!(denial.status, 429);
    assert_eq!(body_text(&denial), "throttled");
    assert_eq!(
        asked.load(Ordering::SeqCst),
        0,
        "the provider was not asked"
    );
}

#[test]
fn a_recognized_identity_without_the_permission_is_forbidden_and_audited() {
    let (kernel, sink, asked, denials) = harness(
        &[(
            "first",
            CredentialKind::ApiKey,
            Verdict::Recognized(granted(GREET)),
        )],
        None,
        // `kernel=error` hides warn lines: the access audit must survive it.
        LogFilter::parse("error").expect("a valid filter"),
    );
    let denial = denied(&kernel, "/write", &request(&[("k", "s")]));
    assert_eq!(denial.status, 403);
    assert_eq!(body_text(&denial), "forbidden:hello.write");
    assert_eq!(asked.load(Ordering::SeqCst), 1, "the provider was asked");
    kernel.flush_log();
    let text = sink.text();
    let line = text
        .lines()
        .find(|line| json_field(line, "event").as_deref() == Some(ACCESS_DENIED.as_str()))
        .unwrap_or_else(|| panic!("the audit line must survive the filter: {text}"));
    assert_eq!(json_field(line, "user").as_deref(), Some("tester"));
    assert_eq!(json_field(line, "client").as_deref(), Some("127.0.0.1"));
    assert_eq!(
        json_field(line, "permission").as_deref(),
        Some("hello.write")
    );
    assert_eq!(
        denials.entries(),
        ["127.0.0.1|tester|hello.write"],
        "the AccessDenied signal reached its handler"
    );
}

#[test]
fn a_public_match_carries_its_guard_and_no_param() {
    let (kernel, _sink, _asked, _denials) = harness(&[], None, LogFilter::default());
    let found = matched(&kernel, "/public");
    assert!(matches!(found.guard(), Guard::Public));
    assert_eq!(found.param(), None);
}

#[test]
fn the_scheme_kind_routes_to_its_owner_by_name() {
    // `first` owns the scheme only: an api key answers unsupported, the
    // scheme request reaches the scheme's owner.
    let (kernel, _sink, asked, _denials) = harness(
        &[(
            "first",
            CredentialKind::Scheme(LEGACY),
            Verdict::Recognized(granted(GREET)),
        )],
        None,
        LogFilter::default(),
    );
    let denial = denied(&kernel, "/me", &request(&[("k", "s")]));
    assert_eq!(denial.status, 501);
    let found = authorized(&kernel, "/me", &request(&[("u", "tester"), ("t", "tok")]));
    assert_eq!(
        found.user().map(ToString::to_string).as_deref(),
        Some("tester")
    );
    assert_eq!(asked.load(Ordering::SeqCst), 1);
}

#[test]
fn an_auth_failure_line_survives_a_filter_that_hides_everything() {
    let quiet = LogFilter::parse("kernel=error").expect("a valid filter");
    let (kernel, sink, _asked, _denials) = harness(
        &[(
            "first",
            CredentialKind::ApiKey,
            Verdict::Rejected(RejectReason::Invalid, None),
        )],
        None,
        quiet,
    );
    let _ = denied(&kernel, "/me", &request(&[("k", "s")]));
    kernel.flush_log();
    let text = sink.text();
    let line = text
        .lines()
        .find(|line| json_field(line, "event").as_deref() == Some(AUTH_FAILED.as_str()))
        .unwrap_or_else(|| panic!("the audit line must survive the filter: {text}"));
    assert_eq!(json_field(line, "reason").as_deref(), Some("invalid"));
    assert_eq!(json_field(line, "client").as_deref(), Some("127.0.0.1"));
}
