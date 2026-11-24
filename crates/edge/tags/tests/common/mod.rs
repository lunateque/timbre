use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use timbre_edge_tags::fixtures::{Accessor, Tag, TagType, WriteOptions, write};

static NEXT: AtomicU64 = AtomicU64::new(0);

pub(crate) struct Sample {
    pub(crate) path: PathBuf,
    dir: PathBuf,
}

impl Sample {
    pub(crate) fn new(name: &str) -> Self {
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("timbre-tags-{}-{id}", std::process::id()));
        std::fs::create_dir(&dir).unwrap();
        let path = dir.join(name);
        let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name);
        std::fs::copy(fixture, &path).unwrap();
        Self { path, dir }
    }

    pub(crate) fn set_title(&self, kind: TagType, title: &str) {
        let mut tag = Tag::new(kind);
        tag.set_title(title.to_owned());
        write(&self.path, &tag, WriteOptions::default()).unwrap();
    }
}

impl Drop for Sample {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.dir).unwrap();
    }
}
