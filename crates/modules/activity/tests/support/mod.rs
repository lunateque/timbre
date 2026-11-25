#![expect(
    clippy::unwrap_used,
    reason = "fixture setup failures must fail the test"
)]

//! Shared test harness and fake catalog for `timbre-activity` integration tests.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use timbre_edge_config::Config;
use timbre_edge_sqlite::SharedDatabase;
use timbre_kernel::activity::Activity;
use timbre_kernel::catalog::{
    Album, AlbumDetail, AlbumOrder, Artist, ArtistDetail, Catalog, Genre, Lyrics, MediaSource,
    ReplayGain, ScanStatus, SearchResults, SearchWindows, Track,
};
use timbre_kernel::identity::UserName;
use timbre_kernel::page::{Page, PageRequest, Window};
use timbre_kernel::signals::Played;
use timbre_kernel::testing::Shared;
use timbre_kernel::{
    Ctx, Failure, FailureCode, Fallible, Format, Kernel, LogFilter, Logger, Port, Registry,
    Resources, ThreadRuntime,
};

#[derive(Debug)]
struct Missing;

impl Failure for Missing {
    fn code(&self) -> FailureCode {
        FailureCode::new("library.not_found")
    }
    fn status(&self) -> u16 {
        404
    }
    fn public_message(&self) -> &'static str {
        "not found"
    }
}

#[derive(Default)]
pub(crate) struct FakeCatalog {
    tracks: Mutex<HashMap<String, String>>,
    albums: Mutex<HashSet<String>>,
    artists: Mutex<HashSet<String>>,
}

impl FakeCatalog {
    pub(crate) fn add_track(&self, track_id: &str, album_id: &str) {
        self.tracks
            .lock()
            .unwrap()
            .insert(track_id.to_owned(), album_id.to_owned());
        self.albums.lock().unwrap().insert(album_id.to_owned());
    }

    pub(crate) fn remove_track(&self, track_id: &str) {
        self.tracks.lock().unwrap().remove(track_id);
    }

    pub(crate) fn remove_album(&self, album_id: &str) {
        self.albums.lock().unwrap().remove(album_id);
    }
}

fn stub_track(id: &str, album_id: &str) -> Track {
    Track {
        id: id.to_owned(),
        title: id.to_owned(),
        sort_name: None,
        album_id: album_id.to_owned(),
        album: album_id.to_owned(),
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
        duration: Duration::from_secs(180),
        size: 1024,
        bitrate_kbps: None,
        sample_rate: None,
        channels: None,
        bit_depth: None,
        suffix: "flac".to_owned(),
        created: UNIX_EPOCH,
    }
}

fn stub_album(id: &str) -> Album {
    Album {
        id: id.to_owned(),
        name: id.to_owned(),
        sort_name: None,
        order_name: id.to_owned(),
        display_artist: "A".to_owned(),
        credits: Vec::new(),
        musicbrainz_id: None,
        release_group_id: None,
        version: None,
        release_types: Vec::new(),
        compilation: None,
        year: None,
        original_release_date: None,
        release_date: None,
        disc_titles: Vec::new(),
        record_labels: Vec::new(),
        moods: Vec::new(),
        genres: Vec::new(),
        explicit_status: None,
        cover_art: None,
        song_count: 1,
        duration: Duration::from_secs(180),
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
    fn track(&self, _: &Ctx, id: &str) -> Fallible<Track> {
        let map = self.tracks.lock().unwrap();
        let album = map
            .get(id)
            .ok_or_else(|| Box::new(Missing) as Box<dyn Failure>)?;
        Ok(stub_track(id, album))
    }
    fn tracks(&self, _: &Ctx, ids: &[&str], page: PageRequest<'_>) -> Fallible<Page<Track>> {
        Window::check_limit(ids.len())?;
        let win = Window::new(page)?;
        let map = self.tracks.lock().unwrap();
        let items: Vec<Track> = ids
            .iter()
            .filter_map(|&id| map.get(id).map(|al| stub_track(id, al)))
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
            .filter(|&&id| map.contains_key(id))
            .map(|&id| (id.to_owned(), Duration::from_secs(180)))
            .collect())
    }
    fn albums_by_ids(&self, _: &Ctx, ids: &[&str], page: PageRequest<'_>) -> Fallible<Page<Album>> {
        let win = Window::new(page)?;
        let set = self.albums.lock().unwrap();
        let items: Vec<Album> = ids
            .iter()
            .filter(|&&id| set.contains(id))
            .map(|&id| stub_album(id))
            .skip(win.offset())
            .take(win.lookahead())
            .collect();
        Ok(win.page(items))
    }
    fn artists_by_ids(
        &self,
        _: &Ctx,
        ids: &[&str],
        page: PageRequest<'_>,
    ) -> Fallible<Page<Artist>> {
        let win = Window::new(page)?;
        let set = self.artists.lock().unwrap();
        let items: Vec<Artist> = ids
            .iter()
            .filter(|&&id| set.contains(id))
            .map(|&id| Artist {
                id: id.to_owned(),
                name: id.to_owned(),
                sort_name: None,
                order_name: id.to_owned(),
                musicbrainz_id: None,
                roles: Vec::new(),
                album_count: 0,
                cover_art: None,
            })
            .skip(win.offset())
            .take(win.lookahead())
            .collect();
        Ok(win.page(items))
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

struct PlayedCollector(Arc<Mutex<Vec<Played>>>);
impl Port for PlayedCollector {}

pub(crate) struct Harness {
    dir: PathBuf,
    pub(crate) kernel: Arc<Kernel>,
    pub(crate) played: Arc<Mutex<Vec<Played>>>,
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
            std::env::temp_dir().join(format!("timbre-act-test-{tag}-{}-{n}", std::process::id()));
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
        let played = Arc::new(Mutex::new(Vec::new()));
        let mut registry = Registry::new()
            .module("activity", timbre_activity::register)
            .unwrap();
        registry
            .provide::<PlayedCollector>(Arc::new(PlayedCollector(Arc::clone(&played))))
            .unwrap();
        registry.on::<Played>(|ctx, signal| {
            let holder = ctx.ports().get::<PlayedCollector>().unwrap();
            holder.0.lock().unwrap().push(signal.clone());
            Ok(())
        });
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
            played,
            catalog,
        })
    }

    pub(crate) fn new(tag: &str, toml: &str, with_catalog: bool) -> Self {
        Self::try_new(tag, toml, with_catalog).unwrap()
    }

    pub(crate) fn ctx(&self) -> Ctx {
        Ctx::root(&self.kernel)
    }

    pub(crate) fn port(&self) -> Arc<dyn Activity> {
        self.ctx().ports().get::<dyn Activity>().unwrap()
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
