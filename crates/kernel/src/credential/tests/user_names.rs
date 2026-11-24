//! User-name handling in `Ctx::authenticate`: canonicalization, rejection
//! and the clip that keeps a probe from bloating the audit line.

use super::*;

#[test]
fn a_malformed_name_can_be_refused_by_the_address_throttle() {
    let harness = Harness::build(
        Some((CredentialKind::Password, Verdict::Recognized(identity()))),
        true,
    );
    let outcome = block_on(harness.ctx().authenticate(
        Credential::Password {
            user: "a b".to_owned(),
            secret: Secret::new("s".to_owned()),
        },
        client(),
    ));
    assert_eq!(outcome, AuthOutcome::Rejected(RejectReason::Throttled));
    assert_eq!(harness.asked_count(), 0);
}

#[test]
fn a_malformed_user_name_never_reaches_the_provider() {
    let harness = Harness::build(
        Some((CredentialKind::Password, Verdict::Recognized(identity()))),
        false,
    );
    let outcome = block_on(harness.ctx().authenticate(
        Credential::Password {
            user: "a b".to_owned(),
            secret: Secret::new("s".to_owned()),
        },
        client(),
    ));
    assert_eq!(outcome, AuthOutcome::Rejected(RejectReason::Invalid));
    assert_eq!(
        harness.asked_count(),
        0,
        "an unnormalizable name never reaches the provider"
    );
    assert_eq!(
        harness.journal.entries(),
        ["127.0.0.1||invalid"],
        "a name the kernel cannot canonicalize is not a `UserName`, so the signal carries none"
    );
    harness.kernel.logger().flush();
    let line = harness.sink.text();
    assert!(
        line.contains(r#""user":"a b""#),
        "the audit line still names the given candidate: {line}"
    );
}

#[test]
fn a_long_rejected_name_candidate_is_clipped_before_logging() {
    let harness = Harness::build(None, false);
    // Not a user name (it contains a space), so it reaches the log only as
    // a clipped candidate: a 200-byte probe must not bloat the audit line.
    let given = format!("{} tail", "x".repeat(200));
    let _ = block_on(harness.ctx().authenticate(
        Credential::Password {
            user: given,
            secret: Secret::new("s".to_owned()),
        },
        client(),
    ));
    harness.kernel.logger().flush();
    let line = harness.sink.text();
    assert!(
        line.contains(&format!("\"user\":\"{}\"", "x".repeat(64))),
        "the candidate is clipped to the user-name bound: {line}"
    );
    assert!(
        !line.contains(&"x".repeat(65)),
        "nothing past the clip is logged: {line}"
    );
    // The clip must end on a char boundary: 63 one-byte chars plus a
    // three-byte one straddles the 64-byte bound, so the multi-byte char
    // is dropped whole rather than sliced.
    let multibyte = format!("{}日 tail", "x".repeat(63));
    let harness = Harness::build(None, false);
    let _ = block_on(harness.ctx().authenticate(
        Credential::Password {
            user: multibyte,
            secret: Secret::new("s".to_owned()),
        },
        client(),
    ));
    harness.kernel.logger().flush();
    let line = harness.sink.text();
    assert!(
        line.contains(&format!("\"user\":\"{}\"", "x".repeat(63))),
        "the clip stops before the straddling char: {line}"
    );
    assert!(
        !line.contains('日'),
        "no half-char reaches the line: {line}"
    );
}

#[test]
fn a_scheme_with_an_empty_user_names_no_one() {
    // A scheme may derive the user from its own fields; the kernel must not
    // canonicalize (and so cannot reject) the empty placeholder.
    let credential = Credential::Scheme {
        name: SchemeName::new("test.sig"),
        user: String::new(),
        fields: Vec::new(),
    };
    assert_eq!(credential.user(), None);
    let named = Credential::Scheme {
        name: SchemeName::new("test.sig"),
        user: "alice".to_owned(),
        fields: Vec::new(),
    };
    assert_eq!(named.user(), Some("alice"));
}

#[test]
fn an_empty_scheme_user_requires_its_owners_opt_in() {
    const SCHEME: SchemeName = SchemeName::new("test.sig");
    // No throttle here: the refusal itself is what this test pins, and the
    // address window that now sees it has its own tests in `settlement`.
    let harness = Harness::build(
        Some((
            CredentialKind::Scheme(SCHEME),
            Verdict::Recognized(identity()),
        )),
        false,
    );
    let outcome = block_on(harness.ctx().authenticate(
        Credential::Scheme {
            name: SCHEME,
            user: String::new(),
            fields: Vec::new(),
        },
        client(),
    ));
    assert_eq!(outcome, AuthOutcome::Rejected(RejectReason::Invalid));
    assert_eq!(harness.asked_count(), 0);
}

#[test]
fn a_credential_user_name_is_canonicalized_for_provider_and_log() {
    #[derive(Debug)]
    struct Echo;
    impl CredentialProvider for Echo {
        fn check(&self, _ctx: Ctx, credential: Credential) -> BoxFuture<Verdict> {
            // The provider sees the canonical spelling, not the wire text.
            let named = credential.user() == Some("ann-marie_2.x");
            Box::pin(async move {
                if named {
                    Verdict::Recognized(identity())
                } else {
                    Verdict::Rejected(RejectReason::Invalid, None)
                }
            })
        }
    }
    let sink = Shared::default();
    let mut registry = Registry::new();
    registry
        .credential_provider(CredentialKind::Forwarded, Arc::new(Echo))
        .expect("provider");
    let kernel = registry.finish(
        Logger::new(
            LogFilter::default(),
            Format::Json,
            Box::new(sink.clone()),
            crate::testing::fixed,
        ),
        Arc::new(ThreadRuntime::new()),
    );
    let outcome = block_on(Ctx::root(&kernel).authenticate(
        Credential::Forwarded {
            user: "  Ann-Marie_2.X ".to_owned(),
        },
        client(),
    ));
    assert_eq!(
        outcome,
        AuthOutcome::Recognized(identity()),
        "the provider only accepts the canonical spelling"
    );
    // A recognized check logs nothing.
    drop(kernel);
    assert_eq!(sink.text(), "");
}
