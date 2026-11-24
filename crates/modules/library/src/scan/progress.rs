use crate::LibraryState;
use crate::log_events::{SCAN_FINISHED, SCAN_PROGRESS};
use crate::store::{all, exec, integer, one, unix_millis, unsigned};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::mpsc::{RecvTimeoutError, Sender, channel};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime};
use timbre_edge_sqlite::Database;
use timbre_edge_sqlite::rusqlite::params;
use timbre_kernel::{Ctx, Fallible, log};

const INTERVAL: Duration = Duration::from_secs(5);

#[derive(Default)]
struct Counts {
    folders: AtomicU64,
    tags: AtomicU64,
    unreadable: AtomicU64,
}

pub(super) struct Progress<'a> {
    ctx: &'a Ctx,
    start: Instant,
    counts: Arc<Counts>,
    stop: Sender<()>,
    reporter: Option<JoinHandle<()>>,
}

impl<'a> Progress<'a> {
    pub(super) fn new(ctx: &'a Ctx, library: &Arc<dyn LibraryState>) -> Fallible<Self> {
        let start = Instant::now();
        let (stop, receiver) = channel();
        let counts = Arc::new(Counts::default());
        let (job_ctx, library, current) = (ctx.clone(), library.clone(), counts.clone());
        let reporter = thread::Builder::new()
            .name("library-progress".into())
            .spawn(move || {
                while receiver.recv_timeout(INTERVAL) == Err(RecvTimeoutError::Timeout) {
                    let folders = current.folders.load(Relaxed);
                    let tag_reads = current.tags.load(Relaxed);
                    let unreadable = current.unreadable.load(Relaxed);
                    {
                        let mut scan = library
                            .scan()
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        scan.folders = folders;
                        scan.tag_reads = tag_reads;
                        scan.unreadable = unreadable;
                    }
                    log!(
                        job_ctx,
                        info,
                        SCAN_PROGRESS,
                        folders = folders,
                        tag_reads = tag_reads,
                        unreadable = unreadable,
                        duration = start.elapsed()
                    );
                }
            })
            .map_err(crate::failures::io)?;
        Ok(Self {
            ctx,
            start,
            counts,
            stop,
            reporter: Some(reporter),
        })
    }

    pub(super) fn folders(&self, count: usize) {
        self.counts
            .folders
            .store(u64::try_from(count).unwrap_or(u64::MAX), Relaxed);
    }

    pub(super) fn tag(&self, failed: bool) {
        self.counts.tags.fetch_add(1, Relaxed);
        if failed {
            self.counts.unreadable.fetch_add(1, Relaxed);
        }
    }

    fn stop_reporter(&mut self) {
        if let Some(thread) = self.reporter.take() {
            let _ = self.stop.send(());
            let _ = thread.join();
        }
    }

    pub(super) fn finish(mut self, db: &Database, unreadable_folders: Option<u64>) -> Fallible {
        self.stop_reporter();
        let folders = self.counts.folders.load(Relaxed);
        let tag_reads = self.counts.tags.load(Relaxed);
        let (tracks, albums, artists, compilations, files) = db.read(|sql| one(sql, "SELECT (SELECT count(*) FROM library_tracks),(SELECT count(*) FROM library_albums),(SELECT count(*) FROM library_artists),(SELECT count(*) FROM library_albums WHERE compilation IS NOT NULL),(SELECT count(*) FROM library_unreadable)", [], |row| Ok((unsigned(row, 0)?, unsigned(row, 1)?, unsigned(row, 2)?, unsigned(row, 3)?, unsigned(row, 4)?))))?;
        let diagnostics = db.read(|sql| {
            all(
                sql,
                "SELECT kind,count(*) FROM library_diagnostics GROUP BY kind ORDER BY kind",
                [],
                |row| Ok((row.get::<_, String>(0)?, unsigned(row, 1)?)),
            )
        })?;
        let unreadable = files.saturating_add(unreadable_folders.unwrap_or_default());
        let now = unix_millis(SystemTime::now());
        if unreadable_folders.is_some() {
            db.write(|sql| { exec(sql, "UPDATE library_state SET unreadable=?1,scanned_folders=?2,tag_reads=?3,finished=?4 WHERE id=1", params![integer(unreadable), integer(folders), integer(tag_reads), now])?; Ok(()) })?;
        }
        log!(
            self.ctx,
            info,
            SCAN_FINISHED,
            tracks = tracks,
            folders = folders,
            albums = albums,
            artists = artists,
            compilations = compilations,
            unreadable = unreadable,
            tag_reads = tag_reads,
            diagnostics = timbre_edge_sqlite::json::encode(&diagnostics)?.as_str(),
            duration = self.start.elapsed(),
            cancelled = unreadable_folders.is_none()
        );
        Ok(())
    }
}

impl Drop for Progress<'_> {
    fn drop(&mut self) {
        self.stop_reporter();
    }
}
