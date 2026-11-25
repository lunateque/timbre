//! Play queue, bookmarks, config validation, cross-user isolation and
//! `UserRemoved` / `UserAdded` cleanup tests.

#[expect(
    dead_code,
    reason = "each test binary uses a subset of the shared harness"
)]
mod support;

use support::{Harness, first, user};
use timbre_kernel::activity::{ItemKind, SaveBookmark, SavePlayQueue, Scrobble};
use timbre_kernel::signals::{UserAdded, UserRemoved};

#[test]
fn config_bounds_play_queue_and_cross_user_isolation_are_enforced() {
    let cfg_err = Harness::try_new("badcfg", "[activity]\nnow_playing_ttl_secs = 10\n", false)
        .err()
        .unwrap();
    assert_eq!(cfg_err.code(), timbre_activity::failures::INVALID_CONFIG);
    assert!(
        cfg_err.public_message().contains("now_playing_ttl_secs"),
        "expected key in public_message, got: {}",
        cfg_err.public_message()
    );
    assert_eq!(
        Harness::try_new("unknowncfg", "[activity]\nbogus = 1\n", false)
            .err()
            .unwrap()
            .code(),
        timbre_edge_config::failures::INVALID
    );

    let h = Harness::new("bounds", "[activity]\nmax_queue_tracks = 2\n", false);
    let ctx = h.ctx();
    let act = h.port();
    let alice = user("alice");
    let bob = user("bob");

    for bad_queue in [
        SavePlayQueue {
            tracks: &["t1"],
            current_index: Some(1),
            position_ms: 0,
            changed_by: "Feishin",
        },
        SavePlayQueue {
            tracks: &[],
            current_index: Some(0),
            position_ms: 0,
            changed_by: "",
        },
        SavePlayQueue {
            tracks: &["t1"],
            current_index: Some(0),
            position_ms: u64::MAX,
            changed_by: "Feishin",
        },
    ] {
        assert_eq!(
            act.save_play_queue(&ctx, &alice, &bad_queue)
                .unwrap_err()
                .code(),
            timbre_activity::failures::INVALID_INPUT
        );
    }
    assert_eq!(
        act.save_play_queue(
            &ctx,
            &alice,
            &SavePlayQueue {
                tracks: &["t1", "t2", "t3"],
                current_index: Some(0),
                position_ms: 0,
                changed_by: "Feishin",
            },
        )
        .unwrap_err()
        .code(),
        timbre_activity::failures::LIMIT_REACHED
    );

    act.save_play_queue(
        &ctx,
        &alice,
        &SavePlayQueue {
            tracks: &["t1", "t2"],
            current_index: Some(1),
            position_ms: 42_000,
            changed_by: "Feishin",
        },
    )
    .unwrap();
    // Bob cannot see Alice's queue.
    assert!(act.play_queue(&ctx, &bob).unwrap().is_none());
    let q = act.play_queue(&ctx, &alice).unwrap().unwrap();
    assert_eq!(q.tracks, ["t1", "t2"]);
    assert_eq!(q.current_index, Some(1));

    act.save_play_queue(
        &ctx,
        &alice,
        &SavePlayQueue {
            tracks: &[],
            current_index: None,
            position_ms: 0,
            changed_by: "",
        },
    )
    .unwrap();
    assert!(act.play_queue(&ctx, &alice).unwrap().is_none());
}

#[test]
fn bookmarks_upsert_isolate_users_and_enforce_limit() {
    let h = Harness::new("bm", "[activity]\nmax_bookmarks_per_user = 1\n", false);
    let ctx = h.ctx();
    let act = h.port();
    let alice = user("alice");
    let bob = user("bob");

    act.save_bookmark(
        &ctx,
        &alice,
        &SaveBookmark {
            track: "t1",
            position_ms: 10_000,
            comment: Some("intro"),
        },
    )
    .unwrap();
    // Bob cannot read or delete Alice's bookmark.
    assert_eq!(act.bookmarks(&ctx, &bob, first(10)).unwrap().items, []);
    assert_eq!(
        act.delete_bookmark(&ctx, &bob, "t1").unwrap_err().code(),
        timbre_activity::failures::NOT_FOUND
    );

    let first_bm = act
        .bookmarks(&ctx, &alice, first(10))
        .unwrap()
        .items
        .remove(0);
    act.save_bookmark(
        &ctx,
        &alice,
        &SaveBookmark {
            track: "t1",
            position_ms: 20_000,
            comment: Some("chorus"),
        },
    )
    .unwrap();
    let updated = act
        .bookmarks(&ctx, &alice, first(10))
        .unwrap()
        .items
        .remove(0);
    assert_eq!(updated.created_at, first_bm.created_at);
    assert_eq!(updated.position_ms, 20_000);
    assert_eq!(updated.comment.as_deref(), Some("chorus"));

    assert_eq!(
        act.save_bookmark(
            &ctx,
            &alice,
            &SaveBookmark {
                track: "t2",
                position_ms: 0,
                comment: None,
            },
        )
        .unwrap_err()
        .code(),
        timbre_activity::failures::LIMIT_REACHED
    );
    act.delete_bookmark(&ctx, &alice, "t1").unwrap();
    assert_eq!(act.bookmarks(&ctx, &alice, first(10)).unwrap().items, []);
}

#[test]
fn user_removed_and_user_added_wipe_data_for_that_user_only() {
    let h = Harness::new("cleanup", "", false);
    let ctx = h.ctx();
    let act = h.port();
    let alice = user("alice");
    let bob = user("bob");

    act.save_play_queue(
        &ctx,
        &alice,
        &SavePlayQueue {
            tracks: &["t1"],
            current_index: Some(0),
            position_ms: 5,
            changed_by: "Phone",
        },
    )
    .unwrap();
    act.save_bookmark(
        &ctx,
        &alice,
        &SaveBookmark {
            track: "t1",
            position_ms: 100,
            comment: None,
        },
    )
    .unwrap();
    act.star(&ctx, &alice, ItemKind::Track, &["t1"]).unwrap();
    act.star(&ctx, &bob, ItemKind::Track, &["t1"]).unwrap();
    act.set_rating(&ctx, &alice, ItemKind::Track, "t1", 4)
        .unwrap();
    for who in [&alice, &bob] {
        act.scrobble(
            &ctx,
            who,
            &Scrobble {
                track: "t1",
                client: "Phone",
                at: None,
                submission: false,
            },
        )
        .unwrap();
    }

    ctx.publish(&UserRemoved {
        user: alice.clone(),
    });
    assert!(act.play_queue(&ctx, &alice).unwrap().is_none());
    assert_eq!(act.bookmarks(&ctx, &alice, first(10)).unwrap().items, []);
    assert_eq!(
        act.starred(&ctx, &alice, ItemKind::Track, first(10))
            .unwrap()
            .items,
        []
    );
    assert_eq!(
        act.ratings(&ctx, &alice, ItemKind::Track, &["t1"]).unwrap(),
        []
    );
    let np = act.now_playing(&ctx, None, first(10)).unwrap();
    assert_eq!(np.items.len(), 1);
    assert_eq!(np.items[0].user, bob);

    // Simulate orphaned rows lingering before username reuse: UserAdded wipes them too.
    act.star(&ctx, &alice, ItemKind::Track, &["t9"]).unwrap();
    ctx.publish(&UserAdded {
        user: alice.clone(),
    });
    assert_eq!(
        act.starred(&ctx, &alice, ItemKind::Track, first(10))
            .unwrap()
            .items,
        []
    );
    assert_eq!(
        act.starred(&ctx, &bob, ItemKind::Track, first(10))
            .unwrap()
            .items
            .len(),
        1
    );
}
