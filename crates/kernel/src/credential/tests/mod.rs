use super::*;
use crate::failures::CREDENTIAL_KIND_TAKEN;
use crate::identity::{Identity, UserName};
use crate::log_events::AUTH_FAILED;
use crate::port::Port;
use crate::signals::AuthFailed;
use crate::testing::{Shared, fixed};
use crate::{Ctx, Format, Kernel, LogFilter, Logger, Registry, ThreadRuntime, block_on};
use std::net::Ipv4Addr;
use std::sync::{Arc, Mutex, PoisonError};

mod user_names;

fn user(text: &str) -> UserName {
    UserName::normalize(text).unwrap_or_else(|rejected| panic!("`{rejected}` is not canonical"))
}

fn identity() -> Identity {
    Identity {
        user: user("tester"),
        permissions: Vec::new(),
    }
}

struct Stub {
    verdict: Verdict,
    asked: Arc<Mutex<usize>>,
}

impl CredentialProvider for Stub {
    fn check(&self, _ctx: Ctx, _credential: Credential) -> BoxFuture<Verdict> {
        let mut asked = self.asked.lock().unwrap_or_else(PoisonError::into_inner);
        *asked += 1;
        drop(asked);
        // The test verdicts are constant, so rebuilding one is cheap.
        let verdict = match &self.verdict {
            Verdict::Recognized(_) => Verdict::Recognized(identity()),
            Verdict::Rejected(reason) => Verdict::Rejected(*reason),
        };
        Box::pin(async move { verdict })
    }
}

trait Journal: Port {
    fn add(&self, entry: String);
    fn entries(&self) -> Vec<String>;
}

#[derive(Default)]
struct Log(Mutex<Vec<String>>);

impl Journal for Log {
    fn add(&self, entry: String) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(entry);
    }

    fn entries(&self) -> Vec<String> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

impl Port for Log {}

struct Deny;

impl Throttle for Deny {
    fn allow(&self, _client: IpAddr, _user: Option<&UserName>) -> bool {
        false
    }
}

// Both signatures are fixed by the `Handler`/`CredentialProvider` types.
#[expect(
    clippy::unnecessary_wraps,
    reason = "the signature is Handler<AuthFailed>"
)]
fn remember(ctx: &Ctx, signal: &AuthFailed) -> crate::Fallible {
    if let Some(journal) = ctx.ports().get::<dyn Journal>() {
        journal.add(format!(
            "{}|{}|{}",
            signal.client,
            signal.user.as_ref().map_or("", UserName::as_str),
            signal.reason.as_str()
        ));
    }
    Ok(())
}

struct Harness {
    kernel: Arc<Kernel>,
    sink: Shared,
    journal: Arc<Log>,
    asked: Arc<Mutex<usize>>,
}

impl Harness {
    fn build(provider: Option<(CredentialKind, Verdict)>, throttle: bool) -> Harness {
        Harness::build_with_filter(provider, throttle, LogFilter::default())
    }

    fn build_with_filter(
        provider: Option<(CredentialKind, Verdict)>,
        throttle: bool,
        filter: LogFilter,
    ) -> Harness {
        let sink = Shared::default();
        let journal = Arc::new(Log::default());
        let asked = Arc::new(Mutex::new(0usize));
        let mut registry = Registry::new();
        registry
            .provide::<dyn Journal>(Arc::clone(&journal) as Arc<dyn Journal>)
            .expect("journal");
        if throttle {
            registry
                .throttle(Arc::new(Deny) as Arc<dyn Throttle>)
                .expect("throttle");
        }
        if let Some((kind, verdict)) = provider {
            registry
                .credential_provider(
                    kind,
                    Arc::new(Stub {
                        verdict,
                        asked: Arc::clone(&asked),
                    }),
                )
                .expect("provider");
        }
        registry.on::<AuthFailed>(remember);
        let kernel = registry.finish(
            Logger::new(filter, Format::Json, Box::new(sink.clone()), fixed),
            Arc::new(ThreadRuntime::new()),
        );
        Harness {
            kernel,
            sink,
            journal,
            asked,
        }
    }

    fn ctx(&self) -> Ctx {
        Ctx::root(&self.kernel)
    }

    fn asked_count(&self) -> usize {
        *self.asked.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

fn client() -> IpAddr {
    IpAddr::V4(Ipv4Addr::LOCALHOST)
}

#[test]
fn a_recognized_credential_reaches_the_identity_and_stays_quiet() {
    let harness = Harness::build(
        Some((CredentialKind::ApiKey, Verdict::Recognized(identity()))),
        false,
    );
    let outcome = block_on(
        harness
            .ctx()
            .authenticate(Credential::ApiKey(Secret::new("k".to_owned())), client()),
    );
    assert_eq!(outcome, AuthOutcome::Recognized(identity()));
    assert_eq!(harness.asked_count(), 1, "the owner was asked");
    assert!(harness.journal.entries().is_empty(), "no signal on success");
    harness.kernel.logger().flush();
    assert_eq!(harness.sink.text(), "");
}

#[test]
fn a_kind_without_an_owner_is_unsupported_and_no_provider_runs() {
    let harness = Harness::build(
        Some((CredentialKind::ApiKey, Verdict::Recognized(identity()))),
        false,
    );
    let outcome = block_on(
        harness
            .ctx()
            .authenticate(Credential::Bearer(Secret::new("t".to_owned())), client()),
    );
    assert_eq!(outcome, AuthOutcome::Unsupported);
    assert_eq!(harness.asked_count(), 0, "the api key owner is not asked");
    assert!(
        harness.journal.entries().is_empty(),
        "unsupported is not a rejection"
    );
}

#[test]
fn a_second_owner_of_a_kind_is_refused_naming_both_modules() {
    let stub = || {
        Arc::new(Stub {
            verdict: Verdict::Rejected(RejectReason::Invalid),
            asked: Arc::new(Mutex::new(0usize)),
        }) as Arc<dyn CredentialProvider>
    };
    let mut registry = Registry::new();
    registry
        .credential_provider(CredentialKind::ApiKey, stub())
        .expect("first owner");
    let error = registry
        .credential_provider(CredentialKind::ApiKey, stub())
        .expect_err("a second owner must fail");
    assert_eq!(error.code(), CREDENTIAL_KIND_TAKEN);
    // Outside a module registration both owners are "the binary"; a guarded
    // registration naming real modules is covered in the guard tests.
    let text = format!("{error:?}");
    assert!(
        text.contains("first") && text.contains("second"),
        "the failure must carry both owners: {text}"
    );
}

#[test]
fn a_rejection_publishes_its_detail_and_never_the_secret() {
    let harness = Harness::build(
        Some((
            CredentialKind::Password,
            Verdict::Rejected(RejectReason::Invalid),
        )),
        false,
    );
    let outcome = block_on(harness.ctx().authenticate(
        Credential::Password {
            user: "tester".to_owned(),
            secret: Secret::new("sup3rs3cr3t".to_owned()),
        },
        client(),
    ));
    assert_eq!(outcome, AuthOutcome::Rejected(RejectReason::Invalid));
    assert_eq!(
        harness.journal.entries(),
        ["127.0.0.1|tester|invalid"],
        "the handler saw client, user and reason"
    );
    harness.kernel.logger().flush();
    let line = harness.sink.text();
    assert!(
        line.contains(&format!("\"event\":\"{}\"", AUTH_FAILED.as_str())),
        "{line}"
    );
    assert!(line.contains(r#""user":"tester""#), "{line}");
    assert!(line.contains(r#""reason":"invalid""#), "{line}");
    assert!(
        !line.contains("sup3rs3cr3t"),
        "the secret must not be logged: {line}"
    );
}

#[test]
fn a_throttle_refuses_before_the_provider_is_asked() {
    let harness = Harness::build(
        Some((CredentialKind::ApiKey, Verdict::Recognized(identity()))),
        true,
    );
    let outcome = block_on(
        harness
            .ctx()
            .authenticate(Credential::ApiKey(Secret::new("k".to_owned())), client()),
    );
    assert_eq!(outcome, AuthOutcome::Rejected(RejectReason::Throttled));
    assert_eq!(harness.asked_count(), 0, "the provider was never asked");
    assert_eq!(
        harness.journal.entries(),
        ["127.0.0.1||throttled"],
        "an unnamed credential still publishes the client"
    );
}

#[test]
fn an_auth_failure_is_logged_even_when_the_filter_hides_warn() {
    let quiet = LogFilter::parse("error").expect("a valid filter");
    let harness = Harness::build_with_filter(
        Some((
            CredentialKind::Password,
            Verdict::Rejected(RejectReason::Invalid),
        )),
        false,
        quiet,
    );
    let _ = block_on(harness.ctx().authenticate(
        Credential::Password {
            user: "tester".to_owned(),
            secret: Secret::new("s".to_owned()),
        },
        client(),
    ));
    harness.kernel.logger().flush();
    let line = harness.sink.text();
    assert!(
        line.contains(&format!("\"event\":\"{}\"", AUTH_FAILED.as_str())),
        "the audit line must survive a filter that hides warn: {line}"
    );
}
