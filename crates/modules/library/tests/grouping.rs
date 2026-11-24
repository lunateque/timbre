#[expect(
    dead_code,
    reason = "each test binary uses a subset of the shared fixture"
)]
mod support;

use support::{FIRST, Fixture};
use timbre_kernel::catalog::{AlbumOrder, CompilationSource, SearchWindow, SearchWindows};

#[test]
fn a_few_guest_artists_do_not_make_a_compilation() {
    let fixture = Fixture::new(&["music"]);
    for (name, artist) in [("a", "A"), ("b", "A"), ("c", "A"), ("d", "B"), ("e", "C")] {
        fixture.flac(
            &format!("mostly/{name}.flac"),
            &[("TITLE", name), ("ALBUM", "Mostly"), ("ARTIST", artist)],
        );
    }
    for (name, artist) in [("a", "A"), ("b", "B"), ("c", "C"), ("d", "D")] {
        fixture.flac(
            &format!("various/{name}.flac"),
            &[("TITLE", name), ("ALBUM", "Various"), ("ARTIST", artist)],
        );
    }
    fixture.scan(false);
    let ctx = fixture.ctx();
    let albums = Fixture::catalog(&ctx)
        .albums(&ctx, AlbumOrder::Name, FIRST)
        .unwrap()
        .items;
    let album = |name| albums.iter().find(|album| album.name == name).unwrap();
    assert_eq!(album("Mostly").compilation, None);
    assert_eq!(
        album("Various").compilation,
        Some(CompilationSource::Derived)
    );
    assert_eq!(album("Mostly").display_artist, "A");
    assert_eq!(album("Various").display_artist, "Various Artists");
}

#[test]
fn a_blank_album_artist_tag_reads_as_absent() {
    let fixture = Fixture::new(&["music"]);
    for (name, artist) in [("a", "A"), ("b", "B"), ("c", "C"), ("d", "D")] {
        fixture.flac(
            &format!("blank/{name}.flac"),
            &[
                ("TITLE", name),
                ("ALBUM", "Blank"),
                ("ARTIST", artist),
                ("ALBUMARTIST", "   "),
            ],
        );
    }
    fixture.scan(false);
    let ctx = fixture.ctx();
    let albums = Fixture::catalog(&ctx)
        .albums(&ctx, AlbumOrder::Name, FIRST)
        .unwrap()
        .items;
    let album = albums.iter().find(|album| album.name == "Blank").unwrap();
    assert_eq!(album.compilation, Some(CompilationSource::Derived));
    assert_eq!(album.display_artist, "Various Artists");
    let detail = Fixture::catalog(&ctx)
        .album(&ctx, &album.id, FIRST)
        .unwrap();
    assert!(
        detail
            .album
            .credits
            .iter()
            .any(|credit| credit.role == "albumartist" && credit.artist.name == "Various Artists"),
        "the Various Artists credit must be linked: {:?}",
        detail.album.credits
    );
}

#[test]
fn a_rescan_keeps_album_ids_and_updates_the_changed_album() {
    let fixture = Fixture::new(&["music"]);
    for name in ["one", "two"] {
        fixture.flac(
            &format!("{name}/a.flac"),
            &[("TITLE", &format!("{name} a")), ("ALBUM", name)],
        );
    }
    fixture.scan(false);
    let ctx = fixture.ctx();
    let before: Vec<_> = Fixture::catalog(&ctx)
        .albums(&ctx, AlbumOrder::Name, FIRST)
        .unwrap()
        .items
        .iter()
        .map(|album| (album.name.clone(), album.id.clone(), album.song_count))
        .collect();
    fixture.flac("two/b.flac", &[("TITLE", "two b"), ("ALBUM", "two")]);
    fixture.scan(false);
    let after: Vec<_> = Fixture::catalog(&ctx)
        .albums(&ctx, AlbumOrder::Name, FIRST)
        .unwrap()
        .items
        .iter()
        .map(|album| (album.name.clone(), album.id.clone(), album.song_count))
        .collect();
    assert_eq!(
        before
            .iter()
            .map(|(name, id, _)| (name, id))
            .collect::<Vec<_>>(),
        after
            .iter()
            .map(|(name, id, _)| (name, id))
            .collect::<Vec<_>>()
    );
    assert_eq!(
        after.iter().map(|(_, _, count)| *count).collect::<Vec<_>>(),
        vec![1, 2]
    );
    assert_eq!(
        Fixture::catalog(&ctx)
            .search(
                &ctx,
                "two b",
                SearchWindows {
                    artists: SearchWindow {
                        count: 0,
                        offset: 0
                    },
                    albums: SearchWindow {
                        count: 0,
                        offset: 0
                    },
                    tracks: SearchWindow {
                        count: 5,
                        offset: 0
                    },
                },
            )
            .unwrap()
            .tracks
            .items
            .len(),
        1
    );
}

#[test]
fn an_incremental_rescan_reindexes_renamed_tracks_and_albums() {
    let fixture = Fixture::new(&["music"]);
    fixture.flac(
        "set/a.flac",
        &[("TITLE", "oldsong"), ("ALBUM", "oldrelease")],
    );
    fixture.scan(false);
    let ctx = fixture.ctx();
    let found = |query: &str| {
        let results = Fixture::catalog(&ctx)
            .search(
                &ctx,
                query,
                SearchWindows {
                    artists: SearchWindow {
                        count: 0,
                        offset: 0,
                    },
                    albums: SearchWindow {
                        count: 5,
                        offset: 0,
                    },
                    tracks: SearchWindow {
                        count: 5,
                        offset: 0,
                    },
                },
            )
            .unwrap();
        (results.tracks.items.len(), results.albums.items.len())
    };
    assert_eq!(found("oldsong"), (1, 0));
    assert_eq!(found("oldrelease"), (1, 1));
    fixture.flac(
        "set/a.flac",
        &[("TITLE", "newsong"), ("ALBUM", "newrelease")],
    );
    fixture.scan(false);
    assert_eq!(found("newsong"), (1, 0));
    assert_eq!(found("newrelease"), (1, 1));
    assert_eq!(found("oldsong"), (0, 0));
    assert_eq!(found("oldrelease"), (0, 0));
}

#[test]
fn wav_and_aiff_report_canonical_container_and_codec_names() {
    let fixture = Fixture::new(&["music"]);
    fixture.write("pcm/wave.wav", include_bytes!("fixtures/tone.wav"));
    fixture.write("pcm/apple.aiff", include_bytes!("fixtures/tone.aiff"));
    fixture.scan(false);
    let ctx = fixture.ctx();
    let port = Fixture::catalog(&ctx);
    let source = |title: &str| port.media_source(&ctx, &fixture.id(title)).unwrap();
    let wave = source("wave");
    assert_eq!(
        (wave.container.as_str(), wave.codec.as_str()),
        ("wav", "pcm")
    );
    let apple = source("apple");
    assert_eq!(
        (apple.container.as_str(), apple.codec.as_str()),
        ("aiff", "pcm")
    );
}

#[test]
fn a_track_leaving_its_album_shrinks_that_album_in_the_same_pass() {
    let fixture = Fixture::new(&["music"]);
    // One release across two folders: editing the tag in one of them leaves
    // the other folder untouched, so the old album is regrouped only if the
    // scope follows the track that left.
    for (folder, title) in [("one", "first"), ("two", "second")] {
        fixture.flac(
            &format!("{folder}/a.flac"),
            &[
                ("TITLE", title),
                ("ALBUM", "Shared"),
                ("ARTIST", "A"),
                (
                    "MUSICBRAINZ_ALBUMID",
                    "11111111-1111-1111-1111-111111111111",
                ),
            ],
        );
    }
    fixture.scan(false);
    let ctx = fixture.ctx();
    let albums = || {
        Fixture::catalog(&ctx)
            .albums(&ctx, AlbumOrder::Name, FIRST)
            .unwrap()
            .items
            .into_iter()
            .map(|album| (album.name, album.song_count))
            .collect::<Vec<_>>()
    };
    assert_eq!(albums(), vec![("Shared".to_owned(), 2)]);
    fixture.flac(
        "two/a.flac",
        &[("TITLE", "second"), ("ALBUM", "Moved"), ("ARTIST", "A")],
    );
    fixture.scan(false);
    assert_eq!(
        albums(),
        vec![("Moved".to_owned(), 1), ("Shared".to_owned(), 1)]
    );
}
