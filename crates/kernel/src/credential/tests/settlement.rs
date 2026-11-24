//! How a reservation settles: a verdict, a refusal without a provider, or
//! a check that is dropped before any verdict arrives.

use super::*;
use crate::signals::AuthFailed;
use std::task::{Context as TaskContext, Waker};

/// A throttle that admits (or refuses) everything and writes down what the
/// kernel asked of it, so a test can see how an attempt was settled.
struct Recorder {
    refuse: bool,
    events: Mutex<Vec<String>>,
}

impl Recorder {
    fn new(refuse: bool) -> Self {
        Self {
            refuse,
            events: Mutex::new(Vec::new()),
        }
    }

    fn note(&self, line: String) {
        self.events
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(line);
    }

    fn events(&self) -> Vec<String> {
        self.events
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

fn shown(user: Option<&UserName>) -> &str {
    user.map_or("-", UserName::as_str)
}

impl Throttle for Recorder {
    fn reserve(&self, _: &Ctx, _: IpAddr, user: Option<&UserName>) -> Option<[bool; 2]> {
        self.note(format!("reserve user={}", shown(user)));
        (!self.refuse).then_some([true, user.is_some()])
    }

    fn finish(
        &self,
        _: &Ctx,
        _: IpAddr,
        user: Option<&UserName>,
        _: [bool; 2],
        reason: Option<RejectReason>,
    ) {
        self.note(format!(
            "finish user={} reason={}",
            shown(user),
            reason.map_or("none", RejectReason::as_str)
        ));
    }
}

/// A provider whose check never answers, so the only way out is a drop.
struct Never;

impl CredentialProvider for Never {
    fn check(&self, _: Ctx, _: Credential) -> BoxFuture<Verdict> {
        Box::pin(std::future::pending())
    }
}

struct Setup {
    kernel: Arc<Kernel>,
    throttle: Arc<Recorder>,
    journal: Arc<Log>,
}

impl Setup {
    fn build(
        refuse: bool,
        provider: Option<(CredentialKind, Arc<dyn CredentialProvider>)>,
    ) -> Self {
        let throttle = Arc::new(Recorder::new(refuse));
        let journal = Arc::new(Log::default());
        let mut registry = Registry::new();
        registry
            .provide::<dyn Journal>(Arc::clone(&journal) as Arc<dyn Journal>)
            .expect("journal");
        registry
            .throttle(Arc::clone(&throttle) as Arc<dyn Throttle>)
            .expect("throttle");
        if let Some((kind, provider)) = provider {
            registry
                .credential_provider(kind, provider)
                .expect("provider");
        }
        registry.on::<AuthFailed>(remember);
        let kernel = registry.finish(
            Logger::new(
                LogFilter::default(),
                Format::Json,
                Box::new(Shared::default()),
                fixed,
            ),
            Arc::new(ThreadRuntime::new()),
        );
        Self {
            kernel,
            throttle,
            journal,
        }
    }

    fn ctx(&self) -> Ctx {
        Ctx::root(&self.kernel)
    }
}

#[test]
fn a_check_dropped_before_its_verdict_counts_as_a_failure() {
    let setup = Setup::build(false, Some((CredentialKind::ApiKey, Arc::new(Never))));
    let mut attempt = setup
        .ctx()
        .authenticate(Credential::ApiKey(Secret::new("k".to_owned())), client());
    let mut task = TaskContext::from_waker(Waker::noop());
    assert!(
        attempt.as_mut().poll(&mut task).is_pending(),
        "the provider never answers"
    );
    // The client went away mid-check: the guess must still cost the window
    // a slot, but nobody was rejected, so nothing is audited.
    drop(attempt);
    assert_eq!(
        setup.throttle.events(),
        vec![
            "reserve user=-".to_owned(),
            "finish user=- reason=invalid".to_owned(),
        ]
    );
    assert!(setup.journal.entries().is_empty(), "no verdict, no signal");
}

#[test]
fn an_empty_scheme_user_is_counted_by_the_address_window() {
    const SCHEME: SchemeName = SchemeName::new("test.sig");
    let setup = Setup::build(false, None);
    let outcome = block_on(setup.ctx().authenticate(
        Credential::Scheme {
            name: SCHEME,
            user: String::new(),
            fields: Vec::new(),
        },
        client(),
    ));
    assert_eq!(outcome, AuthOutcome::Rejected(RejectReason::Invalid));
    assert_eq!(
        setup.throttle.events(),
        vec![
            "reserve user=-".to_owned(),
            "finish user=- reason=invalid".to_owned(),
        ]
    );
}

#[test]
fn an_empty_scheme_user_can_be_refused_by_the_address_window() {
    const SCHEME: SchemeName = SchemeName::new("test.sig");
    let setup = Setup::build(true, None);
    let outcome = block_on(setup.ctx().authenticate(
        Credential::Scheme {
            name: SCHEME,
            user: String::new(),
            fields: Vec::new(),
        },
        client(),
    ));
    assert_eq!(outcome, AuthOutcome::Rejected(RejectReason::Throttled));
    assert!(
        setup.journal.entries().is_empty(),
        "a throttled attempt is counted, not audited"
    );
}
