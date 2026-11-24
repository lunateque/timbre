use crate::config::Settings;
use crate::windows::{Key, Windows};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};
use timbre_kernel::credential::{
    Credential, CredentialKind, CredentialProvider, RejectReason, Verdict,
};
use timbre_kernel::testing::Shared;
use timbre_kernel::{
    AuthOutcome, BoxFuture, Ctx, Fallible, Format, LogFilter, Logger, Registry, Secret,
    ThreadRuntime, block_on,
};

fn client(last: u8) -> IpAddr {
    IpAddr::V4(Ipv4Addr::new(198, 51, 100, last))
}

fn settings() -> Settings {
    Settings::default()
}

fn reserve(windows: &mut Windows, key: &Key, now: Instant, settings: &Settings) -> bool {
    windows
        .reserve([Some(key.clone()), None], now, settings)
        .is_some()
}

fn failure(windows: &mut Windows, key: &Key, now: Instant, settings: &Settings) {
    assert!(reserve(windows, key, now, settings));
    windows.finish(key.clone(), now, settings, true);
}

#[test]
fn the_eleventh_failure_from_one_address_is_refused() {
    let key = Key::client(client(7));
    let settings = settings();
    let mut windows = Windows::default();
    let now = Instant::now();
    for _ in 0..10 {
        failure(&mut windows, &key, now, &settings);
    }
    assert!(!reserve(&mut windows, &key, now, &settings));
}

#[test]
fn the_first_attempt_after_the_window_is_allowed_again() {
    let key = Key::client(client(7));
    let settings = settings();
    let mut windows = Windows::default();
    let now = Instant::now();
    for _ in 0..10 {
        failure(&mut windows, &key, now, &settings);
    }
    for elapsed in [Duration::ZERO, settings.window()] {
        assert!(!reserve(&mut windows, &key, now + elapsed, &settings));
    }
    assert!(reserve(
        &mut windows,
        &key,
        now + settings.window() + Duration::from_nanos(1),
        &settings
    ));
}

#[test]
fn two_addresses_in_one_ipv6_prefix_share_a_window() {
    let first = IpAddr::V6(Ipv6Addr::new(0x2001, 0xdb8, 0xab, 0xcd, 0, 0, 0, 1));
    let second = IpAddr::V6(Ipv6Addr::new(
        0x2001, 0xdb8, 0xab, 0xcd, 0xff, 0xff, 0xff, 0xff,
    ));
    let elsewhere = IpAddr::V6(Ipv6Addr::new(0x2001, 0xdb8, 0xab, 0xce, 0, 0, 0, 1));
    assert_eq!(Key::client(first), Key::client(second));
    assert_ne!(Key::client(first), Key::client(elsewhere));
}

#[test]
fn mapped_ipv4_clients_use_their_own_ipv4_windows() {
    let first = "192.0.2.4".parse().expect("an address");
    let mapped_first = "::ffff:192.0.2.4".parse().expect("an address");
    let mapped_second = "::ffff:192.0.2.5".parse().expect("an address");
    assert_eq!(Key::client(first), Key::client(mapped_first));
    assert_ne!(Key::client(first), Key::client(mapped_second));
}

#[test]
fn the_user_window_counts_separately_from_the_address_window() {
    let settings = settings();
    let user = Key::user("alice");
    let mut windows = Windows::default();
    let now = Instant::now();
    for step in 0..settings.user_failures {
        let address = Key::client(client(u8::try_from(step % 10).unwrap()));
        assert_eq!(
            windows.reserve([Some(address.clone()), Some(user.clone())], now, &settings),
            Some(([true, true], 0))
        );
        windows.finish(address, now, &settings, true);
        windows.finish(user.clone(), now, &settings, true);
    }
    assert!(
        windows
            .reserve([Some(Key::client(client(3))), Some(user)], now, &settings)
            .is_none()
    );
    assert!(
        windows
            .reserve(
                [Some(Key::client(client(3))), Some(Key::user("bob"))],
                now,
                &settings
            )
            .is_some()
    );
}

#[test]
fn a_flood_of_addresses_never_tracks_more_than_the_cap() {
    let settings = Settings {
        max_tracked: 64,
        address_failures: 2,
        ..settings()
    };
    let mut windows = Windows::default();
    let now = Instant::now();
    for step in 0..10_000 {
        let key = Key::client(Ipv4Addr::from_bits(step).into());
        failure(&mut windows, &key, now, &settings);
        assert!(
            windows.tracked() <= settings.max_tracked,
            "the cap held at step {step}"
        );
    }
    assert_eq!(windows.tracked(), settings.max_tracked);
    let recent = Key::client(Ipv4Addr::from_bits(9_999).into());
    failure(&mut windows, &recent, now, &settings);
    assert!(!reserve(&mut windows, &recent, now, &settings));
    let oldest = Key::client(Ipv4Addr::from_bits(0).into());
    failure(&mut windows, &oldest, now, &settings);
    assert!(reserve(&mut windows, &oldest, now, &settings));
}

/// A provider that refuses everything, so failures can be produced without
/// a module that owns real credentials.
struct Refuser;

impl CredentialProvider for Refuser {
    fn check(&self, _: Ctx, _: Credential) -> BoxFuture<Verdict> {
        Box::pin(async { Verdict::Rejected(RejectReason::Invalid, None) })
    }
}

struct Pending;

impl CredentialProvider for Pending {
    fn check(&self, _: Ctx, _: Credential) -> BoxFuture<Verdict> {
        Box::pin(std::future::pending())
    }
}

fn refusing(registry: &mut Registry) -> Fallible {
    registry.credential_provider(CredentialKind::Forwarded, Arc::new(Pending))?;
    registry.credential_provider(CredentialKind::ApiKey, Arc::new(Refuser))
}

fn kernel(limited: bool) -> Ctx {
    let runtime = Arc::new(ThreadRuntime::new());
    let registry = Registry::new().module("refuser", refusing).unwrap();
    let registry = if limited {
        registry.module("auth_guard", crate::register).unwrap()
    } else {
        registry
    };
    let kernel = registry.finish(
        Logger::new(
            LogFilter::default(),
            Format::Json,
            Box::new(Shared::default()),
            SystemTime::now,
        ),
        runtime,
    );
    Ctx::root(&kernel)
}

fn attempt(ctx: &Ctx) -> AuthOutcome {
    block_on(ctx.authenticate(
        Credential::ApiKey(Secret::new("whatever".to_owned())),
        client(1),
    ))
}

#[test]
fn a_registered_guard_throttles_after_the_configured_failures() {
    let ctx = kernel(true);
    ctx.kernel().check_config().unwrap();
    for _ in 0..10 {
        assert_eq!(attempt(&ctx), AuthOutcome::Rejected(RejectReason::Invalid));
    }
    assert_eq!(
        attempt(&ctx),
        AuthOutcome::Rejected(RejectReason::Throttled),
        "the eleventh attempt is refused without asking the provider"
    );
}

#[test]
fn an_unconfigured_guard_refuses_without_checking_credentials() {
    let ctx = kernel(true);
    assert_eq!(
        attempt(&ctx),
        AuthOutcome::Rejected(RejectReason::Throttled)
    );
}

#[test]
fn without_the_module_nothing_is_ever_throttled() {
    let ctx = kernel(false);
    for _ in 0..50 {
        assert_eq!(attempt(&ctx), AuthOutcome::Rejected(RejectReason::Invalid));
    }
}

#[test]
fn a_refusal_this_module_caused_is_not_counted_again() {
    let settings = Settings {
        address_failures: 1,
        ..settings()
    };
    let mut windows = Windows::default();
    let client = Key::client(client(1));
    let now = Instant::now();
    failure(&mut windows, &client, now, &settings);
    let end = now + settings.window();
    assert!(!reserve(&mut windows, &client, end, &settings));
    assert!(reserve(
        &mut windows,
        &client,
        end + Duration::from_nanos(1),
        &settings
    ));
}

#[test]
fn the_address_slots_include_attempts_waiting_for_the_provider() {
    use std::task::{Context, Poll, Waker};

    let ctx = kernel(true);
    ctx.kernel().check_config().unwrap();
    let mut task = Context::from_waker(Waker::noop());
    let mut waiting: Vec<_> = (0..10)
        .map(|_| {
            let mut attempt = ctx.authenticate(
                Credential::Forwarded {
                    user: "alice".to_owned(),
                },
                client(1),
            );
            assert_eq!(attempt.as_mut().poll(&mut task), Poll::Pending);
            attempt
        })
        .collect();
    assert_eq!(
        attempt(&ctx),
        AuthOutcome::Rejected(RejectReason::Throttled)
    );
    drop(waiting.pop());
    // A check dropped before its verdict settles as a failure, so the slot
    // it reserved stays taken instead of passing to the next caller.
    assert_eq!(
        attempt(&ctx),
        AuthOutcome::Rejected(RejectReason::Throttled)
    );
    drop(waiting);
    assert_eq!(
        attempt(&ctx),
        AuthOutcome::Rejected(RejectReason::Throttled)
    );
}

#[test]
fn malformed_names_fill_the_address_window_without_creating_user_windows() {
    let ctx = kernel(true);
    ctx.kernel().check_config().unwrap();
    for _ in 0..10 {
        let outcome = block_on(ctx.authenticate(
            Credential::Forwarded {
                user: "..".to_owned(),
            },
            client(1),
        ));
        assert_eq!(outcome, AuthOutcome::Rejected(RejectReason::Invalid));
    }
    let guard = crate::state(&ctx).unwrap();
    assert_eq!(guard.guard().lock().windows.tracked(), 1);
    assert_eq!(
        attempt(&ctx),
        AuthOutcome::Rejected(RejectReason::Throttled)
    );
}

#[test]
fn a_full_window_cannot_be_evicted_to_make_room() {
    let settings = Settings {
        max_tracked: 1,
        address_failures: 1,
        ..settings()
    };
    let mut windows = Windows::default();
    let first = Key::client(client(1));
    let another = Key::client(client(2));
    let now = Instant::now();
    failure(&mut windows, &first, now, &settings);
    assert_eq!(
        windows.reserve([Some(another.clone()), None], now, &settings),
        Some(([false, false], 1))
    );
    assert_eq!(windows.tracked(), 1, "the fresh address is not tracked");
    assert!(!reserve(&mut windows, &first, now, &settings));
    assert!(reserve(
        &mut windows,
        &another,
        now + settings.window() + Duration::from_nanos(1),
        &settings
    ));
}

#[test]
fn an_elapsed_table_full_interval_is_flushed_without_a_new_request() {
    use timbre_kernel::credential::Throttle as _;

    let ctx = kernel(true);
    ctx.kernel().check_config().unwrap();
    let state = crate::state(&ctx).unwrap();
    let guard = state.guard();
    let mut inner = guard.lock();
    let window = inner.settings.unwrap().window();
    inner.overflow = Some((Instant::now().checked_sub(window).unwrap(), 7));
    drop(inner);

    guard.report(&ctx);
    assert!(guard.lock().overflow.is_none());
}
