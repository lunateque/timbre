//! What the walk records about one directory entry.

use std::fs::{self, Metadata};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) enum Kind {
    Audio,
    Image,
    Lyrics,
}

impl Kind {
    pub(super) fn name(self) -> &'static str {
        match self {
            Self::Audio => "audio",
            Self::Image => "image",
            Self::Lyrics => "lyrics",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) struct Stamp {
    pub(super) size: u64,
    pub(super) modified: SystemTime,
}

impl Stamp {
    pub(super) fn mtime(self) -> String {
        match self.modified.duration_since(SystemTime::UNIX_EPOCH) {
            Ok(age) => age.as_nanos().to_string(),
            Err(error) => format!("-{}", error.duration().as_nanos()),
        }
    }

    pub(super) fn from(metadata: &Metadata) -> std::io::Result<Self> {
        Ok(Self {
            size: metadata.len(),
            modified: metadata.modified()?,
        })
    }
}

pub(super) struct File {
    pub(super) relative: PathBuf,
    pub(super) absolute: PathBuf,
    pub(super) canonical: PathBuf,
    pub(super) stamp: Stamp,
    pub(super) kind: Kind,
}

pub(super) struct Folder {
    pub(super) root: usize,
    pub(super) relative: PathBuf,
    pub(super) absolute: PathBuf,
    pub(super) hash: String,
    pub(super) signature: String,
    pub(super) ignored: bool,
    pub(super) files: Vec<File>,
}

pub(super) fn classify(path: &Path) -> Option<Kind> {
    match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "mp3" | "flac" | "ogg" | "oga" | "opus" | "m4a" | "m4b" | "mp4" | "wav" | "aiff"
        | "aif" | "aifc" | "wavpack" | "wv" | "ape" => Some(Kind::Audio),
        "jpg" | "jpeg" | "png" | "webp" => Some(Kind::Image),
        "lrc" | "txt" => Some(Kind::Lyrics),
        _ => None,
    }
}

/// Files whose presence hides a folder from the library.
pub(crate) const IGNORE_FILES: [&str; 2] = [".nomedia", ".timbreignore"];

/// Whether a change to this path can change the catalog. A watcher reports
/// every write inside a root, and a pass triggered by an unrelated file
/// would re-read tags for nothing.
pub(crate) fn watchable(path: &Path) -> bool {
    let name = path.file_name().and_then(|name| name.to_str());
    if classify(path).is_some() || name.is_some_and(|name| IGNORE_FILES.contains(&name)) {
        return true;
    }
    // A directory event names the directory itself, and a vanished path may
    // have been a directory, so only an existing plain file is dropped.
    !path.is_file()
}

pub(crate) fn observe(path: &Path) -> Option<(u64, SystemTime)> {
    fs::metadata(path)
        .ok()
        .and_then(|metadata| Stamp::from(&metadata).ok())
        .map(|stamp| (stamp.size, stamp.modified))
}
