#![expect(
    clippy::unwrap_used,
    reason = "fixture setup failures must fail the test"
)]

//! Shared test harness and fake catalog for `timbre-playlists` tests.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use timbre_edge_config::Config;
use timbre_edge_sqlite::SharedDatabase;
use timbre_kernel::catalog::{
    Album, AlbumDetail, AlbumOrder, Artist, ArtistDetail, Catalog, Genre, Lyrics, MediaSource,
    ReplayGain, ScanStatus, SearchResults, SearchWindows, Track,
};
use timbre_kernel::identity::UserName;
use timbre_kernel::page::{Page, PageRequest, Window};
use timbre_kernel::playlists::Playlists;
use timbre_kernel::testing::Shared;
use timbre_kernel::{
    Ctx, Fallible, Format, Kernel, LogFilter, Logger, Port, Registry, Resources, ThreadRuntime,
};

#[derive(Default)]
pub(crate) struct FakeCatalog {
    tracks: Mutex<HashMap<String, Duration>>,
}

impl FakeCatalog {
    pub(crate) fn set(&self, id: &str, secs: u64) {
        self.tracks
            .lock()
            .unwrap()
            .insert(id.to_owned(), Duration::from_secs(secs));
    }
    pub(crate) fn remove(&self, id: &str) {
        self.tracks.lock().unwrap().remove(id);
    }
}

fn stub_track(id: &str, duration: Duration) -> Track {
    Track {
        id: id.to_owned(),
        title: id.to_owned(),
        sort_name: None,
        album_id: "a1".to_owned(),
        album: "A".to_owned(),
        display_artist: "A".to_owned(),
        display_album_artist: "A".to_owned(),
        display_composer: String::new(),
        credits: Vec::new(),
        track_number: None,
        disc_number: None,
        year: None,
        genres: Vec::new(),
        musicbrainz_id: None,
        isrc: Vec::new(),
        moods: Vec::new(),
        bpm: None,
        comment: None,
        media_type: None,
        explicit_status: None,
        movements: Vec::new(),
        groupings: Vec::new(),
        works: Vec::new(),
        show_movement: None,
        replay_gain: ReplayGain::default(),
        cover_art: None,
        duration,
        size: 1024,
        bitrate_kbps: None,
        sample_rate: None,
        channels: None,
        bit_depth: None,
        suffix: "flac".to_owned(),
        created: UNIX_EPOCH,
    }
}

impl Port for FakeCatalog {}

impl Catalog for FakeCatalog {
    fn artists(&self, _: &Ctx, _: PageRequest<'_>) -> Fallible<Page<Artist>> {
        unreachable!()
    }
    fn artist(&self, _: &Ctx, _: &str, _: PageRequest<'_>) -> Fallible<ArtistDetail> {
        unreachable!()
    }
    fn album(&self, _: &Ctx, _: &str, _: PageRequest<'_>) -> Fallible<AlbumDetail> {
        unreachable!()
    }
    fn track(&self, _: &Ctx, _: &str) -> Fallible<Track> {
        unreachable!()
    }
    fn tracks(&self, _: &Ctx, ids: &[&str], page: PageRequest<'_>) -> Fallible<Page<Track>> {
        Window::check_limit(ids.len())?;
        let win = Window::new(page)?;
        let map = self.tracks.lock().unwrap();
        let items: Vec<Track> = ids
            .iter()
            .filter_map(|&id| map.get(id).map(|&d| stub_track(id, d)))
            .skip(win.offset())
            .take(win.lookahead())
            .collect();
        Ok(win.page(items))
    }
    fn track_durations(&self, _: &Ctx, ids: &[&str]) -> Fallible<Vec<(String, Duration)>> {
        Window::check_limit(ids.len())?;
        let map = self.tracks.lock().unwrap();
        Ok(ids
            .iter()
            .filter_map(|&id| map.get(id).map(|&d| (id.to_owned(), d)))
            .collect())
    }
    fn albums_by_ids(&self, _: &Ctx, _: &[&str], _: PageRequest<'_>) -> Fallible<Page<Album>> {
        unreachable!()
    }
    fn artists_by_ids(&self, _: &Ctx, _: &[&str], _: PageRequest<'_>) -> Fallible<Page<Artist>> {
        unreachable!()
    }
    fn albums(&self, _: &Ctx, _: AlbumOrder<'_>, _: PageRequest<'_>) -> Fallible<Page<Album>> {
        unreachable!()
    }
    fn genres(&self, _: &Ctx, _: PageRequest<'_>) -> Fallible<Page<Genre>> {
        unreachable!()
    }
    fn search(&self, _: &Ctx, _: &str, _: SearchWindows) -> Fallible<SearchResults> {
        unreachable!()
    }
    fn lyrics(&self, _: &Ctx, _: &str, _: PageRequest<'_>) -> Fallible<Page<Lyrics>> {
        unreachable!()
    }
    fn media_source(&self, _: &Ctx, _: &str) -> Fallible<MediaSource> {
        unreachable!()
    }
    fn scan_status(&self, _: &Ctx) -> Fallible<ScanStatus> {
        unreachable!()
    }
    fn start_scan(&self, _: &Ctx, _: bool) -> Fallible<ScanStatus> {
        unreachable!()
    }
}

pub(crate) struct Harness {
    dir: PathBuf,
    pub(crate) kernel: Arc<Kernel>,
    pub(crate) catalog: Option<Arc<FakeCatalog>>,
}

impl Drop for Harness {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

impl Harness {
    pub(crate) fn try_new(tag: &str, toml: &str, with_catalog: bool) -> Fallible<Self> {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("timbre-pl-test-{tag}-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = dir.join("timbre.toml");
        std::fs::write(&cfg, toml).unwrap();
        let resources = Resources::new()
            .with(Arc::new(Config::load(Some(&cfg)).unwrap()))
            .unwrap()
            .with(Arc::new(SharedDatabase::new(
                dir.join("timbre.db"),
                |_, _| Ok(()),
            )))
            .unwrap();
        let mut registry = Registry::new()
            .module("playlists", timbre_playlists::register)
            .unwrap();
        let catalog = with_catalog.then(|| {
            let cat = Arc::new(FakeCatalog::default());
            registry
                .provide::<dyn Catalog>(Arc::clone(&cat) as Arc<dyn Catalog>)
                .unwrap();
            cat
        });
        let logger = Logger::new(
            LogFilter::default(),
            Format::Json,
            Box::new(Shared::default()),
            SystemTime::now,
        );
        let kernel =
            registry.finish_with_resources(logger, Arc::new(ThreadRuntime::new()), resources);
        if let Err(failure) = kernel.check_config() {
            let _ = std::fs::remove_dir_all(&dir);
            return Err(failure);
        }
        Ok(Self {
            dir,
            kernel,
            catalog,
        })
    }

    pub(crate) fn new(tag: &str, toml: &str, with_catalog: bool) -> Self {
        Self::try_new(tag, toml, with_catalog).unwrap()
    }

    pub(crate) fn ctx(&self) -> Ctx {
        Ctx::root(&self.kernel)
    }

    pub(crate) fn port(&self) -> Arc<dyn Playlists> {
        self.ctx().ports().get::<dyn Playlists>().unwrap()
    }
}

pub(crate) fn user(name: &str) -> UserName {
    UserName::normalize(name).unwrap()
}

pub(crate) fn first(limit: usize) -> PageRequest<'static> {
    PageRequest {
        limit,
        cursor: None,
    }
}
