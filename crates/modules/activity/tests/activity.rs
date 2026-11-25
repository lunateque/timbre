//! End-to-end tests for `timbre-activity` without a catalog: stars, ratings,
//! plays, now playing, validation and top tracks.

#[expect(
    dead_code,
    reason = "each test binary uses a subset of the shared harness"
)]
mod support;

use std::time::{Duration, SystemTime, UNIX_EPOCH};
use support::{Harness, first, user};
use timbre_kernel::activity::{AlbumActivityList, ItemKind, Scrobble};
use timbre_kernel::page::MAX_PAGE_LIMIT;

#[test]
fn stars_are_idempotent_isolated_by_kind_and_user_and_capped() {
    let h = Harness::new("stars", "[activity]\nmax_stars_per_user = 3\n", false);
    let ctx = h.ctx();
    let act = h.port();
    let alice = user("alice");
    let bob = user("bob");

    act.star(&ctx, &alice, ItemKind::Track, &["1", "2", "1"])
        .unwrap();
    act.star(&ctx, &alice, ItemKind::Album, &["1"]).unwrap();
    act.star(&ctx, &bob, ItemKind::Track, &["2"]).unwrap();

    // Fourth distinct star exceeds `max_stars_per_user = 3`.
    assert_eq!(
        act.star(&ctx, &alice, ItemKind::Track, &["3"])
            .unwrap_err()
            .code(),
        timbre_activity::failures::LIMIT_REACHED
    );

    let tracks = act
        .starred(&ctx, &alice, ItemKind::Track, first(10))
        .unwrap();
    assert_eq!(tracks.items.len(), 2);
    let orig_ts = tracks
        .items
        .iter()
        .find(|i| i.id == "1")
        .unwrap()
        .starred_at;

    // Re-starring an already-starred id is idempotent and does not hit the cap.
    act.star(&ctx, &alice, ItemKind::Track, &["1"]).unwrap();
    let lookup = act
        .starred_by_ids(&ctx, &alice, ItemKind::Track, &["3", "1", "2"])
        .unwrap();
    assert_eq!(lookup.len(), 2);
    assert_eq!(lookup[0].id, "1");
    assert_eq!(lookup[0].starred_at, orig_ts);
    assert_eq!(lookup[1].id, "2");

    act.unstar(&ctx, &alice, ItemKind::Track, &["1"]).unwrap();
    assert_eq!(
        act.starred(&ctx, &alice, ItemKind::Track, first(10))
            .unwrap()
            .items
            .iter()
            .map(|i| i.id.as_str())
            .collect::<Vec<_>>(),
        ["2"]
    );
    assert_eq!(
        act.starred(&ctx, &alice, ItemKind::Album, first(10))
            .unwrap()
            .items
            .iter()
            .map(|i| i.id.as_str())
            .collect::<Vec<_>>(),
        ["1"]
    );

    let oversize: Vec<&str> = vec!["1"; MAX_PAGE_LIMIT + 1];
    assert_eq!(
        act.star(&ctx, &alice, ItemKind::Track, &oversize)
            .unwrap_err()
            .code(),
        timbre_kernel::failures::PAGE_TOO_LARGE
    );
    for bad in ["", " 1 ", "a\u{202e}b", "a\nb"] {
        assert_eq!(
            act.star(&ctx, &alice, ItemKind::Track, &[bad])
                .unwrap_err()
                .code(),
            timbre_activity::failures::INVALID_INPUT
        );
    }
}

#[test]
fn ratings_compute_multi_user_averages_clear_on_zero_and_enforce_cap() {
    let h = Harness::new("ratings", "[activity]\nmax_ratings_per_user = 2\n", false);
    let ctx = h.ctx();
    let act = h.port();
    let alice = user("alice");
    let bob = user("bob");

    assert_eq!(
        act.set_rating(&ctx, &alice, ItemKind::Album, "a1", 6)
            .unwrap_err()
            .code(),
        timbre_activity::failures::INVALID_INPUT
    );

    act.set_rating(&ctx, &alice, ItemKind::Album, "a1", 5)
        .unwrap();
    act.set_rating(&ctx, &alice, ItemKind::Album, "a3", 2)
        .unwrap();
    assert_eq!(
        act.set_rating(&ctx, &alice, ItemKind::Album, "a4", 4)
            .unwrap_err()
            .code(),
        timbre_activity::failures::LIMIT_REACHED
    );
    act.set_rating(&ctx, &alice, ItemKind::Album, "a3", 0)
        .unwrap();

    act.set_rating(&ctx, &bob, ItemKind::Album, "a1", 3)
        .unwrap();
    act.set_rating(&ctx, &bob, ItemKind::Album, "a2", 5)
        .unwrap();

    let rated = act
        .ratings(&ctx, &alice, ItemKind::Album, &["a2", "a1", "a3"])
        .unwrap();
    assert_eq!(rated.len(), 2);
    assert_eq!(rated[0].id, "a1");
    assert_eq!(rated[0].user_rating, Some(5));
    assert_eq!(rated[0].count, 2);
    assert!((rated[0].average - 4.0).abs() < 1e-9);
    assert_eq!(rated[1].id, "a2");
    assert_eq!(rated[1].user_rating, None);
    assert_eq!(rated[1].count, 1);

    assert_eq!(
        act.albums(&ctx, &alice, AlbumActivityList::Highest, first(10))
            .unwrap()
            .items,
        ["a2", "a1"]
    );
    act.set_rating(&ctx, &bob, ItemKind::Album, "a2", 0)
        .unwrap();
    assert_eq!(
        act.albums(&ctx, &alice, AlbumActivityList::Highest, first(10))
            .unwrap()
            .items,
        ["a1"]
    );
}

#[test]
fn scrobble_publishes_played_clears_now_playing_and_rejects_future_time() {
    let h = Harness::new("scrobble", "", false);
    let ctx = h.ctx();
    let act = h.port();
    let alice = user("alice");
    let bob = user("bob");
    let t1 = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let t2 = UNIX_EPOCH + Duration::from_secs(1_700_000_120);

    // Future timestamps beyond clock skew are refused.
    let far_future = SystemTime::now() + Duration::from_secs(3_600);
    assert_eq!(
        act.scrobble(
            &ctx,
            &alice,
            &Scrobble {
                track: "tr1",
                client: "App",
                at: Some(far_future),
                submission: true,
            },
        )
        .unwrap_err()
        .code(),
        timbre_activity::failures::INVALID_INPUT
    );

    act.scrobble(
        &ctx,
        &alice,
        &Scrobble {
            track: "tr1",
            client: "Symfonium",
            at: None,
            submission: false,
        },
    )
    .unwrap();
    assert_eq!(*h.played.lock().unwrap(), []);
    assert_eq!(
        act.now_playing(&ctx, None, first(10)).unwrap().items.len(),
        1
    );

    for (who, track, at) in [(&alice, "tr1", t1), (&alice, "tr1", t2), (&bob, "tr2", t2)] {
        act.scrobble(
            &ctx,
            who,
            &Scrobble {
                track,
                client: "Symfonium",
                at: Some(at),
                submission: true,
            },
        )
        .unwrap();
    }
    // Completed submission clears alice's matching now-playing entry.
    assert_eq!(
        act.now_playing(&ctx, Some(&alice), first(10))
            .unwrap()
            .items,
        []
    );

    let events = h.played.lock().unwrap().clone();
    assert_eq!(events.len(), 3);
    assert_eq!(
        (&events[0].user, events[0].track.as_str(), events[0].at),
        (&alice, "tr1", t1)
    );

    let stats = act.track_stats(&ctx, &alice, &["tr2", "tr1"]).unwrap();
    assert_eq!(stats.len(), 1);
    assert_eq!(stats[0].play_count, 2);
    assert_eq!(stats[0].last_played, Some(t2));

    // Global top tracks omit `last_played` so cross-user timestamps never leak.
    let global_top = act.top_tracks(&ctx, None, first(10)).unwrap();
    assert_eq!(
        global_top
            .items
            .iter()
            .map(|s| (s.track.as_str(), s.play_count, s.last_played))
            .collect::<Vec<_>>(),
        [("tr1", 2, None), ("tr2", 1, None)]
    );
}
