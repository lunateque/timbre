use crate::failures::LibraryFailure;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use timbre_edge_config::{Config, serde};
use timbre_edge_sqlite::SharedDatabase;
use timbre_kernel::{Ctx, Fallible};

#[derive(serde::Deserialize)]
#[serde(crate = "timbre_edge_config::serde", default)]
pub(crate) struct Settings {
    pub(crate) roots: Vec<PathBuf>,
    pub(crate) scan_on_start: bool,
    pub(crate) watch: bool,
    pub(crate) scan_workers: usize,
    pub(crate) art_cache_mb: u64,
    pub(crate) append_version_to_title: bool,
    pub(crate) tags: BTreeMap<String, Vec<String>>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            roots: Vec::new(),
            scan_on_start: true,
            watch: true,
            scan_workers: std::thread::available_parallelism().map_or(1, std::num::NonZero::get),
            art_cache_mb: 1024,
            append_version_to_title: false,
            tags: BTreeMap::new(),
        }
    }
}

impl Settings {
    pub(crate) fn get(ctx: &Ctx) -> Fallible<Self> {
        let config = ctx
            .resource::<Config>()
            .ok_or_else(|| LibraryFailure::NotConfigured.boxed())?;
        let settings: Self = config.section("library")?;
        if settings.scan_workers == 0 || settings.tags.values().flatten().any(String::is_empty) {
            return Err(LibraryFailure::Argument.boxed());
        }
        Ok(settings)
    }

    /// The configured roots, canonicalized. A root that overlaps the data
    /// directory is refused: the scan would index the server's own files,
    /// including the database and the art cache, and rewrite what it reads.
    pub(crate) fn canonical_roots(&self, db_path: &Path) -> Fallible<Vec<PathBuf>> {
        let db_path = projected_path(&std::path::absolute(db_path).map_err(crate::failures::io)?)?;
        let data = db_path.parent().unwrap_or(db_path.as_path());
        let overlaps = |root: &Path| data.starts_with(root) || root.starts_with(data);
        let mut canonical = Vec::with_capacity(self.roots.len());
        for path in &self.roots {
            let root = path.canonicalize().map_err(|error| {
                LibraryFailure::RootInvalid(format!("{}: {error}", path.display())).boxed()
            })?;
            if !root.is_dir() || overlaps(&root) {
                return Err(LibraryFailure::RootInvalid(format!(
                    "{} is not a directory separate from the data directory {}",
                    path.display(),
                    data.display()
                ))
                .boxed());
            }
            if !canonical.contains(&root) {
                canonical.push(root);
            }
        }
        Ok(canonical)
    }

    pub(crate) fn policy_key(&self) -> Fallible<String> {
        timbre_edge_sqlite::json::encode(&(self.append_version_to_title, &self.tags))
    }
}

pub(crate) fn shared_db(ctx: &Ctx) -> Fallible<Arc<SharedDatabase>> {
    ctx.resource::<SharedDatabase>()
        .ok_or_else(|| LibraryFailure::NotConfigured.boxed())
}

fn projected_path(path: &Path) -> Fallible<PathBuf> {
    match path.canonicalize() {
        Ok(path) => Ok(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let parent = path
                .parent()
                .ok_or_else(|| LibraryFailure::NotConfigured.boxed())?;
            let name = path
                .file_name()
                .ok_or_else(|| LibraryFailure::NotConfigured.boxed())?;
            // Resolve the existing parent first, including reparse points, before any writes.
            if path.symlink_metadata().is_ok() {
                return Err(crate::failures::io(error));
            }
            Ok(projected_path(parent)?.join(name))
        }
        Err(error) => Err(crate::failures::io(error)),
    }
}

#[cfg(test)]
mod tests {
    use super::Settings;
    use std::path::PathBuf;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("timbre-config-{name}-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("data/art")).unwrap();
        std::fs::create_dir_all(dir.join("music")).unwrap();
        dir
    }

    fn refused(roots: Vec<PathBuf>, db: &std::path::Path) -> String {
        let settings = Settings {
            roots,
            ..Settings::default()
        };
        let Err(failure) = settings.canonical_roots(db) else {
            panic!("the root must be refused")
        };
        failure.code().as_str().to_owned()
    }

    #[test]
    fn a_root_overlapping_the_data_directory_is_refused() {
        let base = temp("overlap");
        let db = base.join("data/timbre.db");
        assert_eq!(refused(vec![base.clone()], &db), "library.root_invalid");
        assert_eq!(
            refused(vec![base.join("data")], &db),
            "library.root_invalid"
        );
        assert_eq!(
            refused(vec![base.join("data/art")], &db),
            "library.root_invalid"
        );
        let settings = Settings {
            roots: vec![base.join("music")],
            ..Settings::default()
        };
        assert_eq!(settings.canonical_roots(&db).unwrap().len(), 1);
        std::fs::remove_dir_all(&base).unwrap();
    }
}
