#[expect(
    dead_code,
    reason = "each test binary uses a subset of the shared fixture"
)]
mod support;

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::thread;
use std::time::{Duration, Instant};
use support::Fixture;
use timbre_edge_tags::fixtures::{OggPictureStorage, Picture, VorbisComments, WriteOptions, write};
use timbre_kernel::block_on;

const EMBEDDED: &[u8] = include_bytes!("fixtures/embedded.png");

#[test]
fn the_scan_rejects_a_database_beneath_a_music_root_before_creating_it() {
    let fixture = Fixture::new(&["music"]);
    let forbidden = fixture.path("not-created/timbre.db");
    let (ctx, _) = fixture.ctx_with(forbidden.clone(), false);
    let args = [String::from("scan")];
    let (handler, _) = ctx.kernel().find_command(&args).unwrap();
    let error = block_on(handler(ctx, Vec::new())).unwrap_err();
    assert_eq!(error.code().as_str(), "library.root_invalid");
    assert!(!forbidden.parent().unwrap().exists());
}

#[test]
fn the_watcher_waits_for_stability_and_preserves_untouched_subtrees() {
    let fixture = Fixture::new(&["music"]);
    fixture.flac("unchanged/disc/keep.flac", &[("TITLE", "Keep")]);
    fs::create_dir_all(fixture.path("receiving")).unwrap();
    fixture.scan(false);
    let existing = fixture.id("Keep");
    let (ctx, runtime) = fixture.ctx_with(fixture.data("timbre.db"), true);
    let port = Fixture::catalog(&ctx);
    ctx.kernel().start_jobs(&ctx);
    fixture.flac("receiving/arriving.flac", &[("TITLE", "Arriving")]);
    let arrived = fixture.path("receiving/arriving.flac");
    for _ in 0..8 {
        let mut file = OpenOptions::new().append(true).open(&arrived).unwrap();
        file.write_all(b"\0").unwrap();
        drop(file);
        thread::sleep(Duration::from_millis(350));
        assert_eq!(fixture.count("library_tracks"), 1);
    }
    let until = Instant::now() + Duration::from_secs(12);
    while fixture.count("library_tracks") != 2 && Instant::now() < until {
        thread::sleep(Duration::from_millis(80));
    }
    runtime.trigger_shutdown();
    assert!(ctx.kernel().wait_jobs(Duration::from_secs(8)));
    assert_eq!(fixture.count("library_tracks"), 2);
    assert_eq!(fixture.id("Keep"), existing);
    assert_eq!(fixture.track("Arriving").title, "Arriving");
    assert_eq!(port.scan_status(&ctx).unwrap().folders, 2);
}

#[test]
fn a_tree_deeper_than_the_limit_is_cut_and_reported() {
    let fixture = Fixture::new(&["music"]);
    let mut deep = fixture.path("");
    for _ in 0..300 {
        deep = deep.join("d");
    }
    fs::create_dir_all(&deep).unwrap();
    fixture.scan(false);
    let folders = fixture.count("library_folders");
    assert!(
        (2..=258).contains(&folders),
        "the walk indexed {folders} folders"
    );
    assert!(fixture.log().contains("directory nesting is too deep"));
}

#[cfg(unix)]
#[test]
fn a_directory_reached_twice_through_a_symlink_is_walked_once() {
    let fixture = Fixture::new(&["music"]);
    fixture.flac("real/track.flac", &[("TITLE", "Once")]);
    std::os::unix::fs::symlink(fixture.path("real"), fixture.path("link")).unwrap();
    std::os::unix::fs::symlink(fixture.path("real"), fixture.path("other")).unwrap();
    fixture.scan(false);
    assert_eq!(fixture.count("library_folders"), 2);
    assert_eq!(fixture.count("library_tracks"), 1);
}

#[test]
fn a_track_that_cannot_be_written_is_recorded_and_the_scan_goes_on() {
    let fixture = Fixture::new(&["music"]);
    fixture.flac("album/first.flac", &[("TITLE", "First")]);
    fixture.scan(false);
    fixture.execute(
        "CREATE TRIGGER reject BEFORE INSERT ON library_tracks WHEN new.title='Broken'
         BEGIN SELECT RAISE(ABORT,'the disk said no'); END",
    );
    fixture.flac("album/broken.flac", &[("TITLE", "Broken")]);
    fixture.flac("album/third.flac", &[("TITLE", "Third")]);
    fixture.scan(false);
    assert_eq!(fixture.count("library_tracks"), 2);
    assert_eq!(fixture.count("library_unreadable"), 1);
    assert_eq!(fixture.track("Third").title, "Third");
}

#[test]
fn a_track_whose_write_fails_halfway_leaves_no_rows_behind() {
    let fixture = Fixture::new(&["music"]);
    fixture.flac("album/first.flac", &[("TITLE", "First")]);
    fixture.flac("album/broken.flac", &[("TITLE", "Broken")]);
    let mut tag = VorbisComments::new();
    tag.push("TITLE".into(), "Broken".into());
    tag.push("LYRICS".into(), "a poison line".into());
    tag.insert_picture(Picture::from_reader(&mut &*EMBEDDED).unwrap(), None)
        .unwrap();
    write(
        &fixture.path("album/broken.flac"),
        &tag,
        WriteOptions::default(),
    )
    .unwrap();
    // The art row goes in before the track row. A later track in the same
    // batch must not see it: without a savepoint per track, the rows of a
    // file that failed stay in the transaction that is about to commit.
    fixture.execute(
        "CREATE TABLE probe(art INTEGER);
         CREATE TRIGGER reject_lyric BEFORE INSERT ON library_lyrics WHEN new.body LIKE '%poison%'
         BEGIN SELECT RAISE(ABORT,'the disk said no'); END;
         CREATE TRIGGER observe AFTER INSERT ON library_tracks WHEN new.title='First'
         BEGIN INSERT INTO probe(art) SELECT count(*) FROM library_art; END",
    );
    fixture.scan(false);
    assert_eq!(fixture.count("library_unreadable"), 1);
    assert_eq!(fixture.count("library_tracks"), 1);
    assert_eq!(fixture.count("probe"), 1, "the observing track was written");
    assert_eq!(
        fixture.count("probe WHERE art = 0"),
        1,
        "the failed track left its art row behind"
    );
    assert_eq!(fixture.track("First").title, "First");
}

#[test]
fn a_scan_stopped_by_shutdown_says_so() {
    let fixture = Fixture::new(&["music"]);
    fixture.flac("album/first.flac", &[("TITLE", "First")]);
    let (ctx, runtime) = fixture.ctx_with(fixture.data("timbre.db"), false);
    // Shutdown before the pass begins: the scan stops at its first check.
    runtime.trigger_shutdown();
    let args = [String::from("scan")];
    let (handler, _) = ctx.kernel().find_command(&args).unwrap();
    block_on(handler(ctx, Vec::new())).unwrap();
    assert_eq!(fixture.said().trim_end(), "Library scan stopped.");
}
