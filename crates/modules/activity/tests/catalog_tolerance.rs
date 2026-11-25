//! Catalog tolerance tests for `timbre-activity`: vanished catalog items are
//! skipped without truncating pages, album plays link via `Catalog::track`,
//! missing tracks refuse scrobble, and play queue indices adjust accurately.

#[expect(
    dead_code,
    reason = "each test binary uses a subset of the shared harness"
)]
mod support;

use std::time::{Duration, UNIX_EPOCH};
use support::{Harness, first, user};
use timbre_kernel::activity::{AlbumActivityList, ItemKind, SavePlayQueue, Scrobble};
use timbre_kernel::page::PageRequest;

#[test]
fn catalog_validation_and_album_plays_work_when_catalog_is_registered() {
    let h = Harness::new("tol-plays", "", true);
    let cat = h.catalog.as_ref().unwrap();
    let ctx = h.ctx();
    let act = h.port();
    let alice = user("alice");

    for (t, a) in [("t1", "a1"), ("t2", "a2"), ("t3", "a3"), ("t4", "a4")] {
        cat.add_track(t, a);
    }

    assert_eq!(
        act.scrobble(
            &ctx,
            &alice,
            &Scrobble {
                track: "missing",
                client: "App",
                at: None,
                submission: true,
            },
        )
        .unwrap_err()
        .code(),
        timbre_activity::failures::NOT_FOUND
    );
    assert_eq!(
        act.star(&ctx, &alice, ItemKind::Album, &["missing"])
            .unwrap_err()
            .code(),
        timbre_activity::failures::NOT_FOUND
    );
    assert_eq!(
        act.set_rating(&ctx, &alice, ItemKind::Album, "missing", 5)
            .unwrap_err()
            .code(),
        timbre_activity::failures::NOT_FOUND
    );
    assert_eq!(*h.played.lock().unwrap(), []);

    let base = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    for (track, offset_s) in [("t1", 10), ("t1", 20), ("t2", 30), ("t3", 40)] {
        act.scrobble(
            &ctx,
            &alice,
            &Scrobble {
                track,
                client: "App",
                at: Some(base + Duration::from_secs(offset_s)),
                submission: true,
            },
        )
        .unwrap();
    }
    let album_stats = act.album_stats(&ctx, &alice, &["a3", "a1", "a4"]).unwrap();
    assert_eq!(
        album_stats
            .iter()
            .map(|s| (s.album.as_str(), s.play_count))
            .collect::<Vec<_>>(),
        [("a1", 2), ("a3", 1)]
    );
    assert_eq!(
        act.albums(&ctx, &alice, AlbumActivityList::Frequent, first(10))
            .unwrap()
            .items,
        ["a1", "a3", "a2"]
    );
    assert_eq!(
        act.albums(&ctx, &alice, AlbumActivityList::Recent, first(10))
            .unwrap()
            .items,
        ["a3", "a2", "a1"]
    );
}

#[test]
fn filtered_paging_and_queue_adjustment_tolerate_vanished_items() {
    let h = Harness::new("tol-page", "", true);
    let cat = h.catalog.as_ref().unwrap();
    let ctx = h.ctx();
    let act = h.port();
    let alice = user("alice");
    for (t, a) in [("t1", "a1"), ("t2", "a2"), ("t3", "a3"), ("t4", "a4")] {
        cat.add_track(t, a);
    }

    act.star(&ctx, &alice, ItemKind::Album, &["a1", "a2", "a3", "a4"])
        .unwrap();
    cat.remove_album("a2");
    cat.remove_album("a3");
    let p1 = act
        .starred(&ctx, &alice, ItemKind::Album, first(1))
        .unwrap();
    assert_eq!(p1.items.len(), 1);
    assert_eq!(p1.items[0].id, "a1");
    assert!(p1.next.is_some());
    let p2 = act
        .starred(
            &ctx,
            &alice,
            ItemKind::Album,
            PageRequest {
                limit: 1,
                cursor: p1.next.as_ref(),
            },
        )
        .unwrap();
    assert_eq!(p2.items.len(), 1);
    assert_eq!(p2.items[0].id, "a4");
    assert!(p2.next.is_none());

    act.save_play_queue(
        &ctx,
        &alice,
        &SavePlayQueue {
            tracks: &["t1", "t2", "t3"],
            current_index: Some(2),
            position_ms: 9_000,
            changed_by: "App",
        },
    )
    .unwrap();
    cat.remove_track("t1");
    let q = act.play_queue(&ctx, &alice).unwrap().unwrap();
    assert_eq!(q.tracks, ["t2", "t3"]);
    assert_eq!(q.current_index, Some(1));
    assert_eq!(q.position_ms, 9_000);

    cat.remove_track("t3");
    let q2 = act.play_queue(&ctx, &alice).unwrap().unwrap();
    assert_eq!(q2.tracks, ["t2"]);
    assert_eq!(q2.current_index, None);
    assert_eq!(q2.position_ms, 0);
}
