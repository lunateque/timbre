use super::paths::Stamp;
use crate::media::{self, EmbeddedArt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use timbre_edge_config::serde;
use timbre_edge_tags::{self as tags, DurationSource, Fields, Lyric, Properties};
use timbre_kernel::Fallible;

/// The tag snapshot kept as JSON beside the track. Lyric bodies stay out of
/// it: they live in `library_lyrics`, and a long one would be stored twice.
#[derive(serde::Deserialize, serde::Serialize)]
#[serde(crate = "timbre_edge_config::serde")]
pub(crate) struct Source {
    pub(crate) fields: Fields,
    pub(crate) properties: Properties,
    pub(crate) suffix: String,
}

pub(super) struct Need {
    pub(super) canonical: PathBuf,
    pub(super) root: PathBuf,
    pub(super) stamp: Stamp,
    pub(super) fingerprint: bool,
}

pub(super) struct Read {
    pub(super) source: Source,
    pub(super) lyrics: Vec<Lyric>,
    pub(super) fingerprint: Option<String>,
    pub(super) art: Option<EmbeddedArt>,
    /// The file's tag was skipped, so the fields above may be empty.
    pub(super) rejected: bool,
}

type Answer = (usize, Fallible<Read>);

fn read(need: &Need) -> Fallible<Read> {
    let path = crate::store::contained(&need.canonical, &need.root)?;
    let tags = tags::read(&path)?;
    let fingerprint =
        if need.fingerprint && tags.properties.duration_source != DurationSource::Approximate {
            let fp = tags::fingerprint(&path, &tags)?;
            Some(format!(
                "{:032x}:{}:{}",
                fp.hash,
                fp.payload_size,
                fp.duration.as_nanos()
            ))
        } else {
            None
        };
    let art = media::embedded_art(&path, &tags);
    let source = Source {
        fields: tags.fields,
        properties: tags.properties,
        suffix: extension(&need.canonical),
    };
    Ok(Read {
        source,
        lyrics: tags.lyrics.into_iter().filter(media::lyric_fits).collect(),
        fingerprint,
        art,
        rejected: tags.rejected_tags,
    })
}

fn extension(path: &Path) -> String {
    path.extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
}

fn worker(
    pending: &[Need],
    next: &AtomicUsize,
    sender: &SyncSender<Answer>,
    stop: &(impl Fn() -> bool + Sync),
) {
    while !stop() {
        let index = next.fetch_add(1, Ordering::Relaxed);
        let Some(task) = pending.get(index) else {
            break;
        };
        if sender.send((index, read(task))).is_err() {
            break;
        }
    }
}

pub(super) fn reads<T>(
    pending: &[Need],
    workers: usize,
    stop: &(impl Fn() -> bool + Sync),
    collect: impl FnOnce(Receiver<Answer>) -> Fallible<T>,
) -> Fallible<T> {
    let next = AtomicUsize::new(0);
    let count = workers.min(pending.len());
    let (sender, receiver) = sync_channel(count.saturating_mul(2));
    std::thread::scope(|scope| {
        for _ in 0..count {
            let sender = sender.clone();
            let next = &next;
            std::thread::Builder::new()
                .name("timbre-library-read".to_owned())
                .spawn_scoped(scope, move || worker(pending, next, &sender, stop))
                .map_err(crate::failures::io)?;
        }
        drop(sender);
        collect(receiver)
    })
}
