use super::{DenyAll, LEGACY, refused, rejecting};
use crate::Registry;
use crate::credential::CredentialKind;
use crate::credential::Throttle;
use crate::failures::{CREDENTIAL_KIND_TAKEN, MODULE_INVALID, THROTTLE_TAKEN};
use std::sync::Arc;

#[test]
fn a_malformed_or_reserved_module_id_is_refused() {
    for id in ["", "Hello", "9lives", "a.b", "kernel", "timbre", "xtask"] {
        let error = refused(Registry::new().module(id, |_| Ok(())));
        assert_eq!(error.code(), MODULE_INVALID, "for id `{id}`");
    }
}

#[test]
fn a_second_owner_of_a_kind_names_both_modules() {
    let stub = rejecting();
    let first = Arc::clone(&stub);
    let registry = Registry::new()
        .module("first", move |r| {
            r.credential_provider(CredentialKind::ApiKey, Arc::clone(&first))
        })
        .expect("the first owner registers");
    let error = refused(registry.module("second", move |r| {
        r.credential_provider(CredentialKind::ApiKey, stub)
    }));
    assert_eq!(error.code(), CREDENTIAL_KIND_TAKEN);
    let text = format!("{error:?}");
    assert!(text.contains("first"), "names the first module: {text}");
    assert!(text.contains("second"), "names the second module: {text}");
}

#[test]
fn a_second_owner_of_a_scheme_kind_names_both_modules() {
    let registry = Registry::new()
        .module("first", move |r| {
            r.credential_provider(CredentialKind::Scheme(LEGACY), rejecting())
        })
        .expect("the first scheme owner registers");
    let error = refused(registry.module("second", move |r| {
        r.credential_provider(CredentialKind::Scheme(LEGACY), rejecting())
    }));
    let text = format!("{error:?}");
    assert_eq!(error.code(), CREDENTIAL_KIND_TAKEN);
    assert!(
        text.contains("other.sig"),
        "the clash names the scheme: {text}"
    );
}

#[test]
fn a_second_throttle_is_refused() {
    let registry = Registry::new()
        .module("first", |r| {
            r.throttle(Arc::new(DenyAll) as Arc<dyn Throttle>)
        })
        .expect("the first throttle registers");
    let error = refused(registry.module("second", |r| {
        r.throttle(Arc::new(DenyAll) as Arc<dyn Throttle>)
    }));
    assert_eq!(error.code(), THROTTLE_TAKEN);
    let text = format!("{error:?}");
    assert!(
        text.contains("first") && text.contains("second"),
        "names both registrations: {text}"
    );
}
