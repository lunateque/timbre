#[expect(
    dead_code,
    reason = "each test binary uses a subset of the shared fixture"
)]
mod support;

use support::{FIRST, Fixture};
use timbre_kernel::catalog::{AlbumOrder, CompilationSource, SearchWindow, SearchWindows};
use timbre_kernel::library_tree::FolderEntry;

#[test]
fn rename_retag_upgrade_and_overlapping_roots_preserve_track_identity() {
    let fixture = Fixture::new(&["music", "music/box"]);
    let tags = [
        ("TITLE", "original"),
        ("MUSICBRAINZ_TRACKID", "ff01-recording"),
    ];
    fixture.flac("box/old.flac", &tags);
    fixture.scan(false);
    let id = fixture.id("original");
    fixture.rename("box/old.flac", "box/new.flac");
    fixture.scan(false);
    assert_eq!(fixture.id("original"), id);
    fixture.flac("box/new.flac", &[("TITLE", "changed title"), tags[1]]);
    fixture.scan(false);
    assert_eq!(fixture.id("changed title"), id);
    assert_eq!(fixture.count("library_track_paths"), 2);
    fixture.change_roots(&["music"]);
    fixture.scan(false);
    assert_eq!(fixture.id("changed title"), id);
    assert_eq!(fixture.count("library_track_paths"), 1);
    fixture.remove("box/new.flac");
    fixture.opus("box/new.opus", &[("TITLE", "after upgrade"), tags[1]]);
    fixture.scan(false);
    assert_eq!(fixture.id("after upgrade"), id);
}

#[test]
fn mp3_replaced_with_flac_in_the_same_folder_keeps_the_recording_id() {
    use timbre_edge_tags::fixtures::{
        Accessor, Frame, Id3v2Tag, UniqueFileIdentifierFrame, WriteOptions, write,
    };

    let fixture = Fixture::new(&["music"]);
    let recording = "72d7ea2f-e795-4774-b519-7a544ca40f52";
    let path = fixture.write(
        "album/one.mp3",
        include_bytes!("../../../edge/tags/tests/fixtures/tone.mp3"),
    );
    let mut tag = Id3v2Tag::default();
    tag.set_title("MP3".to_owned());
    tag.insert(Frame::UniqueFileIdentifier(UniqueFileIdentifierFrame::new(
        "http://musicbrainz.org",
        recording.as_bytes(),
    )));
    write(&path, &tag, WriteOptions::default()).unwrap();
    fixture.scan(false);
    let id = fixture.id("MP3");
    fixture.remove("album/one.mp3");
    fixture.flac(
        "album/one.flac",
        &[("TITLE", "Lossless"), ("MUSICBRAINZ_TRACKID", recording)],
    );
    fixture.scan(false);
    assert_eq!(fixture.id("Lossless"), id);
}

#[test]
fn identical_copies_do_not_merge_or_carry_ambiguous_ids() {
    let fixture = Fixture::new(&["music"]);
    fixture.flac("old/a.flac", &[("TITLE", "alpha")]);
    fixture.flac("old/b.flac", &[("TITLE", "beta")]);
    fixture.scan(false);
    let a = fixture.id("alpha");
    let b = fixture.id("beta");
    assert_ne!(a, b);
    assert!(
        !fixture
            .log()
            .contains("\"event\":\"library.relink_ambiguous\"")
    );
    assert_eq!(fixture.count("library_diagnostics"), 0);
    fixture.rename("old/a.flac", "old/moved.flac");
    fixture.scan(false);
    assert_eq!(fixture.id("alpha"), a);
    assert_eq!(fixture.id("beta"), b);
    assert_eq!(fixture.count("library_diagnostics"), 0);
    fixture.rename("old", "new");
    fixture.scan(false);
    assert_ne!(fixture.id("alpha"), a);
    assert_ne!(fixture.id("beta"), b);
    assert!(
        fixture
            .log()
            .contains("\"event\":\"library.relink_ambiguous\"")
    );
    assert_eq!(fixture.count("library_diagnostics"), 2);
    fixture.scan(false);
    assert_eq!(fixture.count("library_diagnostics"), 0);
}

#[test]
fn releases_join_discs_but_not_distinct_folders_or_editions() {
    let fixture = Fixture::new(&["music"]);
    fixture.flac(
        "main/CD1/a.flac",
        &[
            ("TITLE", "one"),
            ("ALBUM", "The Album"),
            ("ALBUMARTIST", "A"),
            ("DATE", "2003"),
        ],
    );
    fixture.flac(
        "main/CD2/b.flac",
        &[
            ("TITLE", "two"),
            ("ALBUM", "The Album"),
            ("ALBUMARTIST", "A"),
            ("DATE", "2004"),
        ],
    );
    fixture.flac(
        "main/CD2/c.flac",
        &[
            ("TITLE", "three"),
            ("ALBUM", "Different Album"),
            ("ALBUMARTIST", "A"),
        ],
    );
    fixture.flac(
        "other/d.flac",
        &[
            ("TITLE", "four"),
            ("ALBUM", "The Album"),
            ("ALBUMARTIST", "A"),
        ],
    );
    fixture.flac(
        "other/e.flac",
        &[
            ("TITLE", "five"),
            ("ALBUM", "The Album"),
            ("ALBUMARTIST", "A"),
            ("MUSICBRAINZ_ALBUMCOMMENT", "Anniversary"),
        ],
    );
    fixture.scan(false);
    let first = fixture.track("one");
    assert_eq!(fixture.track("two").album_id, first.album_id);
    assert_eq!(fixture.track("three").album_id, first.album_id);
    assert_ne!(fixture.track("four").album_id, first.album_id);
    assert_ne!(
        fixture.track("four").album_id,
        fixture.track("five").album_id
    );
    assert_eq!(fixture.track("two").disc_number, Some(2));
    assert!(
        fixture
            .explain("main/CD2/c.flac")
            .contains("overridden_key")
    );
}

#[test]
fn display_credits_and_links_have_independent_policies() {
    let fixture = Fixture::new(&["music"]);
    for (path, title, artist) in [
        ("1.flac", "feature", "A feat. B"),
        ("2.flac", "slash", "AC/DC"),
        ("3.flac", "ampersand", "Simon & Garfunkel"),
    ] {
        fixture.flac(path, &[("TITLE", title), ("ARTIST", artist)]);
    }
    fixture.flac(
        "4.flac",
        &[
            ("TITLE", "explicit"),
            ("ARTIST", "A feat. B"),
            ("ARTISTS", "Different"),
        ],
    );
    fixture.flac(
        "5.flac",
        &[
            ("TITLE", "paired"),
            ("ARTIST", "A feat. B"),
            ("MUSICBRAINZ_ARTISTID", "paired-id"),
        ],
    );
    fixture.scan(false);
    let linked = |title| {
        fixture
            .track(title)
            .credits
            .into_iter()
            .filter(|credit| credit.role == "artist")
            .map(|credit| credit.artist.name)
            .collect::<Vec<_>>()
    };
    assert_eq!(fixture.track("feature").display_artist, "A feat. B");
    assert_eq!(linked("feature"), ["A", "B"]);
    assert_eq!(linked("slash"), ["AC/DC"]);
    assert_eq!(linked("ampersand"), ["Simon & Garfunkel"]);
    assert_eq!(linked("explicit"), ["Different"]);
    assert_eq!(linked("paired"), ["A feat. B"]);
    let id = fixture.id("ampersand");
    fixture.extra("[library.tags]\nartist = [' & ']\n");
    fixture.scan(false);
    assert_eq!(fixture.id("ampersand"), id);
    assert_eq!(linked("ampersand"), ["Simon", "Garfunkel"]);
}

#[test]
fn compilation_source_and_replaygain_are_exact() {
    let fixture = Fixture::new(&["music"]);
    fixture.flac(
        "tag/a.flac",
        &[
            ("TITLE", "tagged"),
            ("ALBUM", "Tagged"),
            ("ARTIST", "A"),
            ("COMPILATION", "1"),
        ],
    );
    fixture.flac(
        "inferred/a.flac",
        &[("TITLE", "first"), ("ALBUM", "Inferred"), ("ARTIST", "A")],
    );
    fixture.flac(
        "inferred/b.flac",
        &[("TITLE", "second"), ("ALBUM", "Inferred"), ("ARTIST", "B")],
    );
    fixture.flac(
        "inferred/c.flac",
        &[("TITLE", "third"), ("ALBUM", "Inferred"), ("ARTIST", "C")],
    );
    fixture.opus(
        "guests/a.opus",
        &[
            ("TITLE", "gain"),
            ("ALBUM", "Guests"),
            ("ARTIST", "A feat. B"),
            ("ALBUMARTIST", "A"),
            ("R128_TRACK_GAIN", "-3264"),
        ],
    );
    fixture.flac(
        "guests/b.flac",
        &[
            ("TITLE", "host"),
            ("ALBUM", "Guests"),
            ("ARTIST", "A"),
            ("ALBUMARTIST", "A"),
        ],
    );
    fixture.scan(false);
    let ctx = fixture.ctx();
    let port = Fixture::catalog(&ctx);
    let albums = port.albums(&ctx, AlbumOrder::Name, FIRST).unwrap().items;
    let album = |name| albums.iter().find(|album| album.name == name).unwrap();
    assert_eq!(
        album("Inferred").compilation,
        Some(CompilationSource::Derived)
    );
    assert_eq!(album("Tagged").compilation, Some(CompilationSource::Tag));
    assert!(album("Guests").compilation.is_none());
    assert_eq!(
        album("Tagged").credits[0].artist.id,
        album("Inferred").credits[0].artist.id
    );
    assert_eq!(
        port.artist(&ctx, &album("Tagged").credits[0].artist.id, FIRST)
            .unwrap()
            .artist
            .musicbrainz_id
            .as_deref(),
        Some("89ad4ac3-39f7-470e-963a-56509c546377")
    );
    assert_eq!(fixture.track("gain").replay_gain.track_gain, Some(-7.75));
}

#[test]
fn folded_query_is_literal_and_handles_short_and_long_cjk() {
    let fixture = Fixture::new(&["music"]);
    for (path, title) in [
        ("1.flac", "Déjà Vu"),
        ("2.flac", "Ёлка"),
        ("3.flac", "R.E.M."),
        ("4.flac", "a-ha"),
        ("5.flac", "猫の音楽"),
        ("6.flac", "one OR two"),
        ("7.flac", "one"),
    ] {
        fixture.flac(path, &[("TITLE", title)]);
    }
    fixture.scan(false);
    for (query, title) in [
        ("deja vu", "Déjà Vu"),
        ("елка", "Ёлка"),
        ("rem", "R.E.M."),
        ("aha", "a-ha"),
        ("の音楽", "猫の音楽"),
        ("猫の", "猫の音楽"),
        ("one OR two", "one OR two"),
    ] {
        let ctx = fixture.ctx();
        let result = Fixture::catalog(&ctx)
            .search(
                &ctx,
                query,
                SearchWindows {
                    tracks: SearchWindow {
                        count: 10,
                        offset: 0,
                    },
                    ..SearchWindows::default()
                },
            )
            .unwrap();
        assert_eq!(
            result
                .tracks
                .items
                .iter()
                .map(|track| track.title.as_str())
                .collect::<Vec<_>>(),
            [title]
        );
    }
}

#[test]
fn second_scan_opens_no_tags_and_folder_ignore_is_a_flag() {
    let fixture = Fixture::new(&["music"]);
    fixture.flac("hidden/song.flac", &[("TITLE", "song")]);
    fixture.write("hidden/.nomedia", b"");
    fixture.scan(false);
    let ctx = fixture.ctx();
    let tree = Fixture::tree(&ctx);
    let before = tree.tree_version(&ctx).unwrap();
    let hidden = Fixture::folder_at(&ctx, &["hidden"]);
    assert!(hidden.ignored);
    assert!(matches!(hidden.children.items[0], FolderEntry::File { .. }));
    fixture.scan(false);
    assert!(fixture.log().contains("\"tag_reads\":0"));
    assert_eq!(tree.tree_version(&ctx).unwrap(), before);
    fixture.rename("hidden", "renamed");
    fixture.scan(false);
    assert_eq!(Fixture::folder_at(&ctx, &["renamed"]).id, hidden.id);
    assert!(tree.tree_version(&ctx).unwrap() > before);
}

#[test]
fn unrelated_edits_do_not_reopen_unchanged_unreadable_inputs() {
    let fixture = Fixture::new(&["music"]);
    fixture.write("corrupt/broken.flac", b"not an audio file");
    fixture.flac("healthy/a.flac", &[("TITLE", "First")]);
    fixture.scan(false);
    fixture.flac("healthy/b.flac", &[("TITLE", "Second")]);
    fixture.scan(false);
    let ctx = fixture.ctx();
    let status = Fixture::catalog(&ctx).scan_status(&ctx).unwrap();
    assert_eq!(status.tag_reads, 1);
    assert_eq!(status.unreadable, 1);
    assert_eq!(status.tracks, 2);
    fixture.flac("corrupt/broken.flac", &[("TITLE", "Repaired")]);
    fixture.scan(false);
    let status = Fixture::catalog(&ctx).scan_status(&ctx).unwrap();
    assert_eq!(
        (status.tag_reads, status.unreadable, status.tracks),
        (1, 0, 3)
    );
}
