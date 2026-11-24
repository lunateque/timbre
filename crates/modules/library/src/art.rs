use crate::Library;
use crate::config::{Settings, shared_db};
use crate::failures::{LibraryFailure, io};
use crate::log_events::ART_SOURCE_STALE;
use crate::media::{IMAGE_LIMITS, image_failure, read_bounded};
use crate::store::{all, contained, one, row_path};
use std::fs::{self, File, OpenOptions};
use std::io::{Read as _, Seek as _, SeekFrom, Write as _};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;
use timbre_edge_image::{content_hash, thumbnail};
use timbre_edge_sqlite::Database;
use timbre_edge_sqlite::rusqlite::{Connection, params};
use timbre_kernel::artwork::{Artwork, Image, ImageBody};
use timbre_kernel::{Ctx, Fallible, log};

/// The thumbnail cache's bookkeeping: a sequence for temporary file names and
/// the directory's size, measured once and carried forward.
#[derive(Default)]
pub(crate) struct ArtCache {
    sequence: AtomicU64,
    total: Mutex<Option<u64>>,
}

impl Artwork for Library {
    fn image(&self, ctx: &Ctx, id: &str, size: Option<u32>) -> Fallible<Image> {
        let (id, hash) = parse_id(id).ok_or_else(|| LibraryFailure::NotFound.boxed())?;
        let shared = shared_db(ctx)?;
        let db = shared.get(ctx)?;
        db.read(|sql| exists(sql, id, hash))?;
        let Some(size) = size else {
            let (mime, bytes) = stored_art(ctx, db, hash)?;
            return Ok(Image {
                body: ImageBody::Bytes(bytes),
                mime,
                etag: hash.to_owned(),
            });
        };
        if size == 0 || size > IMAGE_LIMITS.max_dimension {
            return Err(LibraryFailure::ImageTooLarge.boxed());
        }
        let cap = Settings::get(ctx)?.art_cache_mb.saturating_mul(1024 * 1024);
        let name = format!("{hash}-{size}.jpg");
        let dir = match cap {
            0 => None,
            _ => Some(cache_dir(db.path())?),
        };
        if let Some(dir) = &dir {
            let path = dir.join(&name);
            if let Some((file, bytes)) = cached(&path)? {
                return Ok(jpeg(ImageBody::CacheFile(file), &bytes));
            }
        }
        // Decoding and the original's file reads stay outside the cache lock:
        // one slow source must not hold up every other thumbnail request.
        let (_, original) = stored_art(ctx, db, hash)?;
        let bytes = thumbnail(&original, size, IMAGE_LIMITS)
            .map_err(|error| image_failure(error.as_ref()))?;
        let len = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        if let Some(dir) = dir.filter(|_| len <= cap) {
            let path = dir.join(&name);
            self.art.store(&dir, &path, &bytes, cap)?;
        }
        let etag = format!("{:032x}", content_hash(&bytes));
        Ok(Image {
            body: ImageBody::Bytes(bytes),
            mime: "image/jpeg".to_owned(),
            etag,
        })
    }
}

impl ArtCache {
    /// Write one thumbnail and keep the directory under `cap`. The lock
    /// covers the write and the running total only; eviction runs here,
    /// after a store, instead of listing the directory on every cache hit.
    fn store(&self, dir: &Path, path: &Path, bytes: &[u8], cap: u64) -> Fallible {
        let sequence = self.sequence.fetch_add(1, Ordering::Relaxed);
        let mut total = self
            .total
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let known = match *total {
            Some(known) => known,
            None => measure(dir)?,
        };
        // A write that fails halfway leaves the total unknown, not wrong.
        *total = None;
        write_entry(dir, path, bytes, sequence)?;
        let len = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        let grown = known.saturating_add(len);
        *total = Some(if grown > cap {
            evict(dir, cap, path)?
        } else {
            grown
        });
        Ok(())
    }
}

enum Locator {
    Sidecar,
    Span(i64, i64),
    Picture(i64),
}

fn stored_art(ctx: &Ctx, db: &Database, hash: &str) -> Fallible<(String, Vec<u8>)> {
    let (mime, sources) = db.read(|sql| {
        let mime = one(sql, "SELECT mime FROM library_art WHERE hash=?1", [hash], |row| row.get(0))?;
        let sources = all(sql,
            "SELECT p.absolute,r.path,e.offset,e.length,e.picture FROM library_tracks t JOIN library_embedded_art e ON e.track_id=t.id JOIN library_track_paths p ON p.track_id=t.id JOIN library_roots r ON r.id=p.root_id WHERE t.art_hash=?1 UNION ALL SELECT a.path,r.path,NULL,NULL,NULL FROM library_folder_art a JOIN library_folders f ON f.id=a.folder_id JOIN library_roots r ON r.id=f.root_id WHERE a.hash=?1",
            [hash], |row| {
                let location = match (row.get(2)?, row.get(3)?, row.get(4)?) {
                    (Some(offset), Some(length), _) => Locator::Span(offset, length),
                    (_, _, Some(index)) => Locator::Picture(index),
                    _ => Locator::Sidecar,
                };
                Ok((row_path(row, 0)?, row_path(row, 1)?, location))
            })?;
        Ok((mime, sources))
    })?;
    for (path, root, locator) in sources {
        let bytes = contained(&path, &root)
            .ok()
            .and_then(|path| locator.load(&path));
        if let Some(bytes) = bytes.filter(|bytes| format!("{:032x}", content_hash(bytes)) == hash) {
            return Ok((mime, bytes));
        }
        log!(
            ctx,
            warn,
            ART_SOURCE_STALE,
            hash = hash,
            path = path.to_string_lossy().as_ref()
        );
    }
    Err(LibraryFailure::NotFound.boxed())
}

impl Locator {
    fn load(&self, path: &Path) -> Option<Vec<u8>> {
        match *self {
            Self::Sidecar => read_bounded(path, IMAGE_LIMITS.max_bytes).ok().flatten(),
            Self::Picture(index) => {
                timbre_edge_tags::read_picture(path, usize::try_from(index).ok()?)
                    .ok()
                    .flatten()
                    .map(|picture| picture.bytes)
                    .filter(|bytes| bytes.len() <= IMAGE_LIMITS.max_bytes)
            }
            Self::Span(offset, length) => {
                let offset = u64::try_from(offset).ok()?;
                let length = usize::try_from(length)
                    .ok()
                    .filter(|len| *len <= IMAGE_LIMITS.max_bytes)?;
                let mut file = File::open(path).ok()?;
                file.seek(SeekFrom::Start(offset)).ok()?;
                let mut bytes = vec![0; length];
                file.read_exact(&mut bytes).ok()?;
                Some(bytes)
            }
        }
    }
}

fn jpeg(body: ImageBody, bytes: &[u8]) -> Image {
    Image {
        body,
        mime: "image/jpeg".to_owned(),
        etag: format!("{:032x}", content_hash(bytes)),
    }
}

fn parse_id(value: &str) -> Option<(i64, &str)> {
    let (kind, rest) = value.split_once('-')?;
    let (id, hash) = rest.split_once('-')?;
    if kind != "al" || !valid_hash(hash) {
        return None;
    }
    Some((id.parse().ok()?, hash))
}

fn valid_hash(hash: &str) -> bool {
    hash.len() == 32
        && hash
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

fn exists(sql: &Connection, id: i64, hash: &str) -> Fallible {
    let valid: bool = one(
        sql,
        "SELECT EXISTS(SELECT 1 FROM library_albums WHERE id=?1 AND cover_art=?2)",
        params![id, format!("al-{id}-{hash}")],
        |row| row.get(0),
    )?;
    if valid {
        Ok(())
    } else {
        Err(LibraryFailure::NotFound.boxed())
    }
}

fn cache_dir(db: &Path) -> Fallible<PathBuf> {
    let parent = db
        .canonicalize()
        .map_err(io)?
        .parent()
        .ok_or_else(|| LibraryFailure::NotConfigured.boxed())?
        .to_path_buf();
    let dir = parent.join("art");
    let created = match fs::create_dir(&dir) {
        Ok(()) => true,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => false,
        Err(error) => return Err(io(error)),
    };
    if !dir.symlink_metadata().map_err(io)?.file_type().is_dir()
        || dir.canonicalize().map_err(io)? != dir
    {
        return Err(LibraryFailure::NotConfigured.boxed());
    }
    // The cache holds the same pictures the database protects, so it is
    // created owner-only. A directory the operator already had keeps its
    // mode: this is their data directory, not ours to tighten on every hit.
    #[cfg(unix)]
    if created {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).map_err(io)?;
    }
    #[cfg(not(unix))]
    let _ = created;
    Ok(dir)
}

/// Serve an entry from the cache, as the handle that was checked plus the
/// bytes read through it.
fn cached(path: &Path) -> Fallible<Option<(std::fs::File, Vec<u8>)>> {
    let mut file = match OpenOptions::new().read(true).write(true).open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(io(error)),
    };
    // The handle decides, not the name: an entry swapped for a symlink or a
    // directory between the lookup and the read is refused, and the caller
    // reads from this handle, so nothing can be switched underneath it.
    if !file.metadata().map_err(io)?.file_type().is_file()
        || path
            .symlink_metadata()
            .map_err(io)?
            .file_type()
            .is_symlink()
    {
        return Err(LibraryFailure::NotConfigured.boxed());
    }
    file.set_modified(SystemTime::now()).map_err(io)?;
    let cap = u64::try_from(IMAGE_LIMITS.max_bytes).unwrap_or(u64::MAX);
    let mut bytes = Vec::new();
    (&file)
        .take(cap.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(io)?;
    if bytes.len() > IMAGE_LIMITS.max_bytes {
        return Ok(None);
    }
    file.seek(std::io::SeekFrom::Start(0)).map_err(io)?;
    Ok(Some((file, bytes)))
}

fn write_entry(dir: &Path, path: &Path, bytes: &[u8], sequence: u64) -> Fallible {
    let temp = dir.join(format!(".{}-{sequence}.tmp", std::process::id()));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)
        .map_err(io)?;
    let result = file.write_all(bytes);
    drop(file);
    let result = result.and_then(|()| fs::rename(&temp, path));
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result.map_err(io)
}

/// Whether the entry is a thumbnail this cache wrote.
fn is_entry(path: &Path) -> bool {
    let Some(name) = path.file_stem().and_then(|name| name.to_str()) else {
        return false;
    };
    path.extension().and_then(|ext| ext.to_str()) == Some("jpg")
        && name
            .split_once('-')
            .is_some_and(|(hash, size)| valid_hash(hash) && size.parse::<u32>().is_ok())
}

/// The cache's size on disk, read once per process. Temporary files a killed
/// process left behind are removed here; one that resists removal keeps
/// counting against the cap instead of being forgotten.
fn measure(dir: &Path) -> Fallible<u64> {
    let mut total = 0u64;
    for item in fs::read_dir(dir).map_err(io)? {
        let item = item.map_err(io)?;
        if !item.file_type().map_err(io)?.is_file() {
            continue;
        }
        let path = item.path();
        let len = item.metadata().map_err(io)?.len();
        // A leftover temporary file still occupies the cache when it cannot
        // be removed, so it counts towards the total either way.
        let leftover = path.extension().and_then(|ext| ext.to_str()) == Some("tmp")
            && fs::remove_file(&path).is_err();
        if is_entry(&path) || leftover {
            total = total.saturating_add(len);
        }
    }
    Ok(total)
}

/// Delete the least recently used entries until the cache fits in `cap`,
/// and return what is left on disk.
fn evict(dir: &Path, cap: u64, keep: &Path) -> Fallible<u64> {
    let mut total = 0u64;
    let mut eviction = Vec::new();
    for item in fs::read_dir(dir).map_err(io)? {
        let item = item.map_err(io)?;
        if !item.file_type().map_err(io)?.is_file() {
            continue;
        }
        let path = item.path();
        if !is_entry(&path) {
            continue;
        }
        let metadata = item.metadata().map_err(io)?;
        let len = metadata.len();
        total = total.saturating_add(len);
        if path != keep {
            eviction.push((metadata.modified().map_err(io)?, path, len));
        }
    }
    eviction.sort_unstable();
    for (_, path, bytes) in eviction {
        if total <= cap {
            break;
        }
        fs::remove_file(path).map_err(io)?;
        total = total.saturating_sub(bytes);
    }
    Ok(total)
}
