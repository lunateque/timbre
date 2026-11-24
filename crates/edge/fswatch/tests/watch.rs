use std::path::PathBuf;
use std::time::{Duration, Instant};
use timbre_edge_fswatch::{Event, Watcher, failures};

struct TempDir(PathBuf);

impl TempDir {
    #[expect(
        clippy::expect_used,
        reason = "test fixture: a temp directory that cannot be made is a broken environment"
    )]
    fn new() -> Self {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("a clock after the epoch")
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("timbre-watch-{}-{unique}", std::process::id()));
        std::fs::create_dir(&path).expect("a fresh temp directory");
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn watches_the_canonical_root_and_coalesces_repeated_writes() {
    let root = TempDir::new();
    let nested = root.0.join("nested");
    std::fs::create_dir(&nested).unwrap();
    let mut watcher = Watcher::new(&[nested.join("..")], Duration::from_millis(100)).unwrap();
    let file = nested.join("track.mp3");
    for bytes in [b"new".as_slice(), b"updated", b"final"] {
        std::fs::write(&file, bytes).unwrap();
    }
    let canonical = file.canonicalize().unwrap();
    let end = Instant::now() + Duration::from_secs(5);
    loop {
        assert!(Instant::now() < end, "watcher did not report the new track");
        if let Some(Event::Paths(paths)) = watcher.recv_timeout(Duration::from_millis(250)).unwrap()
            && paths.contains(&canonical)
        {
            assert_eq!(paths.iter().filter(|path| *path == &canonical).count(), 1);
            break;
        }
    }
}

#[test]
fn refuses_a_missing_root() {
    let root = TempDir::new();
    let Err(error) = Watcher::new(&[root.0.join("missing")], Duration::ZERO) else {
        panic!("missing root accepted");
    };
    assert_eq!(error.code(), failures::WATCH);
}
