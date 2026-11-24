#![expect(
    clippy::unwrap_used,
    reason = "fixture setup failures must fail the test"
)]

use std::cell::RefCell;
use std::fmt::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use timbre_edge_config::Config;
use timbre_edge_sqlite::{SharedDatabase, failures::query_error};
use timbre_edge_tags::fixtures::{VorbisComments, WriteOptions, write};
use timbre_kernel::catalog::{Catalog, SearchWindow, SearchWindows, Track};
use timbre_kernel::library_tree::LibraryTree;
use timbre_kernel::testing::Shared;
use timbre_kernel::{
    Ctx, Format, LogFilter, Logger, PageRequest, Registry, Resources, ThreadRuntime, block_on,
};

pub(crate) const FIRST: PageRequest<'static> = PageRequest {
    limit: 50,
    cursor: None,
};
static NEXT: AtomicUsize = AtomicUsize::new(0);

pub(crate) struct Fixture {
    base: PathBuf,
    roots: RefCell<Vec<String>>,
    extra: RefCell<String>,
    sink: Shared,
    output: Shared,
}

impl Fixture {
    pub(crate) fn new(roots: &[&str]) -> Self {
        let base = std::env::temp_dir().join(format!(
            "timbre-library-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("data")).unwrap();
        std::fs::create_dir_all(base.join("music")).unwrap();
        Self {
            base,
            roots: RefCell::new(roots.iter().map(|root| (*root).to_owned()).collect()),
            extra: RefCell::new(String::new()),
            sink: Shared::default(),
            output: Shared::default(),
        }
    }
    pub(crate) fn ctx(&self) -> Ctx {
        self.ctx_with(self.data("timbre.db"), false).0
    }

    pub(crate) fn ctx_with(&self, database: PathBuf, watch: bool) -> (Ctx, Arc<ThreadRuntime>) {
        let path = self.base.join("config.toml");
        let mut config = String::from("[library]\nroots = [");
        for root in self.roots.borrow().iter() {
            write!(config, "'{}',", self.base.join(root).display()).unwrap();
        }
        writeln!(
            config,
            "]\nscan_workers = 2\nwatch = {watch}\nscan_on_start = false"
        )
        .unwrap();
        config.push_str(&self.extra.borrow());
        std::fs::write(&path, config).unwrap();
        let resources = Resources::new()
            .with(Arc::new(Config::load(Some(&path)).unwrap()))
            .unwrap()
            .with(Arc::new(SharedDatabase::new(database, |_, path| {
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                Ok(())
            })))
            .unwrap()
            .with(Arc::new(timbre_kernel::command::Output::new(Box::new(
                self.output.clone(),
            ))))
            .unwrap();
        let runtime = Arc::new(ThreadRuntime::new());
        let kernel = Registry::new()
            .module("library", timbre_library::register)
            .unwrap()
            .finish_with_resources(
                Logger::new(
                    LogFilter::default(),
                    Format::Json,
                    Box::new(self.sink.clone()),
                    std::time::SystemTime::now,
                ),
                runtime.clone(),
                resources,
            );
        kernel.check_config().unwrap();
        (Ctx::root(&kernel), runtime)
    }
    pub(crate) fn path(&self, path: &str) -> PathBuf {
        self.base.join("music").join(path)
    }
    pub(crate) fn data(&self, path: &str) -> PathBuf {
        self.base.join("data").join(path)
    }
    pub(crate) fn change_roots(&self, roots: &[&str]) {
        *self.roots.borrow_mut() = roots.iter().map(|root| (*root).to_owned()).collect();
    }
    pub(crate) fn extra(&self, config: &str) {
        config.clone_into(&mut self.extra.borrow_mut());
    }
    pub(crate) fn log(&self) -> String {
        self.sink.text()
    }
    pub(crate) fn write(&self, path: &str, bytes: &[u8]) -> PathBuf {
        let path = self.path(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, bytes).unwrap();
        path
    }
    pub(crate) fn rename(&self, from: &str, to: &str) {
        std::fs::rename(self.path(from), self.path(to)).unwrap();
    }
    pub(crate) fn remove(&self, path: &str) {
        std::fs::remove_file(self.path(path)).unwrap();
    }
    pub(crate) fn flac(&self, path: &str, tags: &[(&str, &str)]) {
        self.vorbis(path, include_bytes!("../fixtures/tone.flac"), tags);
    }
    pub(crate) fn opus(&self, path: &str, tags: &[(&str, &str)]) {
        self.vorbis(path, include_bytes!("../fixtures/tone.opus"), tags);
    }
    fn vorbis(&self, path: &str, bytes: &[u8], tags: &[(&str, &str)]) {
        let path = self.write(path, bytes);
        let mut tag = VorbisComments::new();
        for &(key, value) in tags {
            tag.push(key.to_owned(), value.to_owned());
        }
        write(&path, &tag, WriteOptions::default()).unwrap();
    }
    pub(crate) fn scan(&self, full: bool) {
        self.command(
            "scan",
            if full {
                vec!["--full".to_owned()]
            } else {
                Vec::new()
            },
        );
    }
    pub(crate) fn explain(&self, path: &str) -> String {
        self.command(
            "library explain",
            vec![self.path(path).to_string_lossy().into_owned()],
        );
        self.output.text().lines().last().unwrap().to_owned()
    }
    /// Everything the commands have written to stdout so far.
    pub(crate) fn said(&self) -> String {
        self.output.text()
    }
    fn command(&self, name: &str, rest: Vec<String>) {
        let ctx = self.ctx();
        let args: Vec<_> = name.split(' ').map(str::to_owned).collect();
        let (handler, suffix) = ctx.kernel().find_command(&args).unwrap();
        assert_eq!(suffix, [] as [String; 0]);
        block_on(handler(ctx.clone(), rest)).unwrap();
        ctx.kernel().flush_log();
    }
    pub(crate) fn catalog(ctx: &Ctx) -> Arc<dyn Catalog> {
        ctx.ports().get::<dyn Catalog>().unwrap()
    }
    pub(crate) fn tree(ctx: &Ctx) -> Arc<dyn LibraryTree> {
        ctx.ports().get::<dyn LibraryTree>().unwrap()
    }
    /// The folder reached by walking these names down from the first root.
    pub(crate) fn folder_at(ctx: &Ctx, names: &[&str]) -> timbre_kernel::library_tree::Folder {
        use timbre_kernel::library_tree::FolderEntry;
        let tree = Self::tree(ctx);
        let roots = tree.roots(ctx, FIRST).unwrap();
        let root = roots.items.first().unwrap();
        let mut folder = tree.folder(ctx, &root.folder_id, FIRST).unwrap();
        for name in names {
            let id = folder
                .children
                .items
                .iter()
                .find_map(|entry| match entry {
                    FolderEntry::Folder {
                        id, name: child, ..
                    } if child == name => Some(id.clone()),
                    _ => None,
                })
                .unwrap();
            folder = tree.folder(ctx, &id, FIRST).unwrap();
        }
        folder
    }
    pub(crate) fn track(&self, title: &str) -> Track {
        let ctx = self.ctx();
        Self::catalog(&ctx)
            .search(
                &ctx,
                title,
                SearchWindows {
                    tracks: SearchWindow {
                        count: 50,
                        offset: 0,
                    },
                    ..SearchWindows::default()
                },
            )
            .unwrap()
            .tracks
            .items
            .into_iter()
            .find(|track| track.title == title)
            .unwrap()
    }
    pub(crate) fn id(&self, title: &str) -> String {
        self.track(title).id
    }
    pub(crate) fn execute(&self, statement: &str) {
        let ctx = self.ctx();
        let shared = ctx.resource::<SharedDatabase>().unwrap();
        let db = shared.get(&ctx).unwrap();
        db.write(|sql| {
            sql.execute_batch(statement).map_err(query_error)?;
            Ok(())
        })
        .unwrap();
    }
    pub(crate) fn count(&self, table: &str) -> i64 {
        let ctx = self.ctx();
        let shared = ctx.resource::<SharedDatabase>().unwrap();
        let db = shared.get(&ctx).unwrap();
        db.read(|sql| {
            sql.query_row(&format!("select count(*) from {table}"), [], |row| {
                row.get(0)
            })
            .map_err(query_error)
        })
        .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(Path::new(&self.base));
    }
}
