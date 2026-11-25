//! End-to-end tests for `timbre-playlists`: IDOR/visibility rules, CRUD,
//! duplicate preservation, catalog tolerance, caps, config and `UserRemoved`.

mod support;

use std::time::Duration;
use support::{Harness, first, user};
use timbre_kernel::playlists::{CreatePlaylist, UpdatePlaylist};
use timbre_kernel::signals::{UserAdded, UserRemoved};

#[test]
fn idor_and_visibility_rules_protect_private_and_public_playlists() {
    let h = Harness::new("idor", "", false);
    let ctx = h.ctx();
    let pl = h.port();
    let alice = user("alice");
    let bob = user("bob");

    let secret = pl
        .create(
            &ctx,
            &alice,
            &CreatePlaylist {
                name: "Secret Mix",
                comment: None,
                public: false,
                tracks: &["t1", "t2"],
            },
        )
        .unwrap();
    let shared = pl
        .create(
            &ctx,
            &alice,
            &CreatePlaylist {
                name: "Public Mix",
                comment: Some("for all"),
                public: true,
                tracks: &["t1"],
            },
        )
        .unwrap();

    // Private playlist answers NotFound to bob on get, update and delete.
    assert_eq!(
        pl.get(&ctx, &bob, &secret.id, first(10))
            .unwrap_err()
            .code(),
        timbre_playlists::failures::NOT_FOUND
    );
    assert_eq!(
        pl.update(
            &ctx,
            &bob,
            &secret.id,
            &UpdatePlaylist {
                name: Some("Hijacked"),
                ..UpdatePlaylist::default()
            },
        )
        .unwrap_err()
        .code(),
        timbre_playlists::failures::NOT_FOUND
    );
    assert_eq!(
        pl.delete(&ctx, &bob, &secret.id).unwrap_err().code(),
        timbre_playlists::failures::NOT_FOUND
    );

    // Public playlist is readable by bob, but update and delete answer Forbidden.
    let read_shared = pl.get(&ctx, &bob, &shared.id, first(10)).unwrap();
    assert_eq!(read_shared.summary.name, "Public Mix");
    assert_eq!(read_shared.entries.items, ["t1"]);
    assert_eq!(
        pl.update(
            &ctx,
            &bob,
            &shared.id,
            &UpdatePlaylist {
                name: Some("Hijacked"),
                ..UpdatePlaylist::default()
            },
        )
        .unwrap_err()
        .code(),
        timbre_playlists::failures::FORBIDDEN
    );
    assert_eq!(
        pl.delete(&ctx, &bob, &shared.id).unwrap_err().code(),
        timbre_playlists::failures::FORBIDDEN
    );

    let bob_all = pl.list(&ctx, &bob, None, first(10)).unwrap();
    assert_eq!(
        bob_all
            .items
            .iter()
            .map(|s| s.id.as_str())
            .collect::<Vec<_>>(),
        [shared.id.as_str()]
    );
    let bob_of_alice = pl.list(&ctx, &bob, Some(&alice), first(10)).unwrap();
    assert_eq!(bob_of_alice.items.len(), 1);
    assert_eq!(bob_of_alice.items[0].id, shared.id);
    assert_eq!(
        pl.list(&ctx, &alice, Some(&alice), first(10))
            .unwrap()
            .items
            .len(),
        2
    );
}

#[test]
fn updates_preserve_duplicates_and_align_indices_with_live_catalog_tracks() {
    let h = Harness::new("catalog", "", true);
    let cat = h.catalog.as_ref().unwrap();
    cat.set("t1", 100);
    cat.set("t2", 200);
    cat.set("t3", 300);

    let ctx = h.ctx();
    let pl = h.port();
    let alice = user("alice");

    let created = pl
        .create(
            &ctx,
            &alice,
            &CreatePlaylist {
                name: "Roadtrip",
                comment: Some("v1"),
                public: false,
                tracks: &["t1", "t2", "t1", "t3"],
            },
        )
        .unwrap();
    assert_eq!(created.song_count, 4);
    assert_eq!(created.duration, Duration::from_secs(700));

    // Delete t1: both occurrences vanish, visible list is [t2, t3].
    cat.remove("t1");
    let detail = pl.get(&ctx, &alice, &created.id, first(10)).unwrap();
    assert_eq!(detail.entries.items, ["t2", "t3"]);
    assert_eq!(detail.summary.song_count, 2);
    assert_eq!(detail.summary.duration, Duration::from_secs(500));

    // Removing visible index 1 removes t3, clearing comment sets None.
    let updated = pl
        .update(
            &ctx,
            &alice,
            &created.id,
            &UpdatePlaylist {
                name: Some("Roadtrip 2"),
                comment: Some(None),
                public: Some(true),
                replace_tracks: None,
                add_tracks: &["t2"],
                remove_indices: &[1],
            },
        )
        .unwrap();
    assert_eq!(updated.name, "Roadtrip 2");
    assert_eq!(updated.comment, None);
    assert!(updated.public);
    assert_eq!(updated.song_count, 2);
    assert_eq!(updated.duration, Duration::from_secs(400));
    assert_eq!(
        pl.get(&ctx, &alice, &created.id, first(10))
            .unwrap()
            .entries
            .items,
        ["t2", "t2"]
    );

    assert_eq!(
        pl.update(
            &ctx,
            &alice,
            &created.id,
            &UpdatePlaylist {
                remove_indices: &[5],
                ..UpdatePlaylist::default()
            },
        )
        .unwrap_err()
        .code(),
        timbre_playlists::failures::INVALID_INPUT
    );

    let replaced = pl
        .update(
            &ctx,
            &alice,
            &created.id,
            &UpdatePlaylist {
                replace_tracks: Some(&["t3"]),
                ..UpdatePlaylist::default()
            },
        )
        .unwrap();
    assert_eq!(replaced.song_count, 1);
    assert_eq!(replaced.duration, Duration::from_secs(300));
}

#[test]
fn config_validation_and_per_user_caps_are_enforced() {
    let cfg_err = Harness::try_new("badcfg", "[playlists]\nmax_playlists_per_user = 0\n", false)
        .err()
        .unwrap();
    assert_eq!(cfg_err.code(), timbre_playlists::failures::INVALID_CONFIG);
    assert!(
        cfg_err.public_message().contains("max_playlists_per_user"),
        "expected key in public_message, got: {}",
        cfg_err.public_message()
    );
    assert_eq!(
        Harness::try_new("unknowncfg", "[playlists]\nbogus = 1\n", false)
            .err()
            .unwrap()
            .code(),
        timbre_edge_config::failures::INVALID
    );

    let h = Harness::new(
        "caps",
        "[playlists]\nmax_playlists_per_user = 2\nmax_tracks_per_playlist = 2\nmax_total_entries_per_user = 3\n",
        false,
    );
    let ctx = h.ctx();
    let pl = h.port();
    let alice = user("alice");

    let mk = |name, tracks: &[&str]| {
        pl.create(
            &ctx,
            &alice,
            &CreatePlaylist {
                name,
                comment: None,
                public: true,
                tracks,
            },
        )
    };
    for bad in ["bad\u{202e}name", "   ", "bad\nname"] {
        assert_eq!(
            mk(bad, &[]).unwrap_err().code(),
            timbre_playlists::failures::INVALID_INPUT
        );
    }
    assert_eq!(
        mk("Too Big", &["t1", "t2", "t3"]).unwrap_err().code(),
        timbre_playlists::failures::LIMIT_REACHED
    );

    let first_pl = mk("One", &["t1"]).unwrap();
    let appended = pl
        .update(
            &ctx,
            &alice,
            &first_pl.id,
            &UpdatePlaylist {
                add_tracks: &["t2"],
                ..UpdatePlaylist::default()
            },
        )
        .unwrap();
    assert_eq!(appended.song_count, 2);
    assert_eq!(
        pl.get(&ctx, &alice, &first_pl.id, first(10))
            .unwrap()
            .entries
            .items,
        ["t1", "t2"]
    );
    assert_eq!(
        mk("Over Total", &["t3", "t4"]).unwrap_err().code(),
        timbre_playlists::failures::LIMIT_REACHED
    );
}

#[test]
fn user_removed_and_user_added_wipe_playlists_for_that_user_only() {
    let h = Harness::new("user-wipe", "", false);
    let ctx = h.ctx();
    let pl = h.port();
    let alice = user("alice");
    let bob = user("bob");

    let first_pl = pl
        .create(
            &ctx,
            &alice,
            &CreatePlaylist {
                name: "One",
                comment: None,
                public: true,
                tracks: &["t1"],
            },
        )
        .unwrap();
    let bob_pl = pl
        .create(
            &ctx,
            &bob,
            &CreatePlaylist {
                name: "Bob One",
                comment: None,
                public: false,
                tracks: &["t9"],
            },
        )
        .unwrap();

    ctx.publish(&UserRemoved {
        user: alice.clone(),
    });
    assert_eq!(
        pl.get(&ctx, &bob, &first_pl.id, first(10))
            .unwrap_err()
            .code(),
        timbre_playlists::failures::NOT_FOUND
    );
    let orphan = pl
        .create(
            &ctx,
            &alice,
            &CreatePlaylist {
                name: "Orphan",
                comment: None,
                public: false,
                tracks: &["t1"],
            },
        )
        .unwrap();
    ctx.publish(&UserAdded {
        user: alice.clone(),
    });
    assert_eq!(
        pl.get(&ctx, &alice, &orphan.id, first(10))
            .unwrap_err()
            .code(),
        timbre_playlists::failures::NOT_FOUND
    );
    assert_eq!(
        pl.get(&ctx, &bob, &bob_pl.id, first(10))
            .unwrap()
            .entries
            .items,
        ["t9"]
    );
}
