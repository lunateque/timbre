use super::*;
use crate::failures::CREDENTIAL_KIND_TAKEN;
use crate::identity::UserId;
use crate::log_events::AUTH_FAILED;
use crate::signals::AuthFailed;
use crate::testing::{Shared, fixed};
use crate::{Ctx, Format, Kernel, LogFilter, Logger, Registry, ThreadRuntime, block_on};
use std::net::Ipv4Addr;
use std::sync::{Arc, Mutex, PoisonError};

fn principal() -> Principal {
    Principal {
        user: UserId::new("tester"),
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
            Verdict::Recognized(_) => Verdict::Recognized(principal()),
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

impl Port for Deny {}

impl Throttle for Deny {
    fn allow(&self, _client: IpAddr, _user: Option<&str>) -> bool {
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
            signal.user.clone().unwrap_or_default(),
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
        let sink = Shared::default();
        let journal = Arc::new(Log::default());
        let asked = Arc::new(Mutex::new(0usize));
        let mut registry = Registry::new();
        registry
            .provide::<dyn Journal>(Arc::clone(&journal) as Arc<dyn Journal>)
            .expect("journal");
        if throttle {
            registry
                .provide::<dyn Throttle>(Arc::new(Deny) as Arc<dyn Throttle>)
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
            Logger::new(
                LogFilter::default(),
                Format::Json,
                Box::new(sink.clone()),
                fixed,
            ),
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
        Ctx::root(&self.kernel).with_client(IpAddr::V4(Ipv4Addr::LOCALHOST))
    }

    fn asked_count(&self) -> usize {
        *self.asked.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[test]
fn a_recognized_credential_reaches_the_principal_and_stays_quiet() {
    let harness = Harness::build(
        Some((CredentialKind::ApiKey, Verdict::Recognized(principal()))),
        false,
    );
    let outcome = block_on(
        harness
            .ctx()
            .authenticate(Credential::ApiKey(Secret::new("k".to_owned()))),
    );
    assert_eq!(outcome, AuthOutcome::Recognized(principal()));
    assert_eq!(harness.asked_count(), 1, "the owner was asked");
    assert!(harness.journal.entries().is_empty(), "no signal on success");
    harness.kernel.logger().flush();
    assert_eq!(harness.sink.text(), "");
}

#[test]
fn a_kind_without_an_owner_is_unsupported_and_no_provider_runs() {
    let harness = Harness::build(
        Some((CredentialKind::ApiKey, Verdict::Recognized(principal()))),
        false,
    );
    let outcome = block_on(
        harness
            .ctx()
            .authenticate(Credential::Bearer(Secret::new("t".to_owned()))),
    );
    assert_eq!(outcome, AuthOutcome::Unsupported);
    assert_eq!(harness.asked_count(), 0, "the api key owner is not asked");
    assert!(
        harness.journal.entries().is_empty(),
        "unsupported is not a rejection"
    );
}

#[test]
fn a_second_owner_of_a_kind_is_refused() {
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
    let outcome = block_on(harness.ctx().authenticate(Credential::Password {
        user: "tester".to_owned(),
        secret: Secret::new("sup3rs3cr3t".to_owned()),
    }));
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
        Some((CredentialKind::ApiKey, Verdict::Recognized(principal()))),
        true,
    );
    let outcome = block_on(
        harness
            .ctx()
            .authenticate(Credential::ApiKey(Secret::new("k".to_owned()))),
    );
    assert_eq!(outcome, AuthOutcome::Rejected(RejectReason::Throttled));
    assert_eq!(harness.asked_count(), 0, "the provider was never asked");
    assert_eq!(
        harness.journal.entries(),
        ["127.0.0.1||throttled"],
        "an unnamed credential still publishes the client"
    );
}
