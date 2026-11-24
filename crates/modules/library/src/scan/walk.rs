use super::paths::{File, Folder, Stamp, classify};
use super::tree::KnownFolder;
use crate::digest::Digest;
use crate::log_events::FILE_UNREADABLE;
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use timbre_kernel::{Ctx, Fallible, log};

/// Directories below this depth are a loop or an attack, not a music library.
const MAX_DEPTH: usize = 256;

pub(super) type Forest = BTreeMap<(usize, PathBuf), Folder>;

pub(super) struct Inventory {
    pub(super) forest: Forest,
    pub(super) retained: HashSet<i64>,
    pub(super) unreadable: u64,
}

pub(super) fn inventory(
    ctx: &Ctx,
    roots: &[PathBuf],
    indexed: &BTreeMap<PathBuf, (i64, i64)>,
    previous: &[KnownFolder],
    scope: Option<&HashSet<PathBuf>>,
    stop: &impl Fn() -> bool,
) -> Fallible<Inventory> {
    let old = previous
        .iter()
        .map(|folder| ((folder.root, folder.relative.as_path()), folder))
        .collect();
    let mut walker = Walker {
        ctx,
        seen: HashSet::new(),
        folders: BTreeMap::new(),
        retained: HashSet::new(),
        cutoff: HashSet::new(),
        warnings: super::warnings::Warnings::default(),
        old,
        scope,
        stop,
    };
    for (ordinal, root) in roots.iter().enumerate() {
        // Each root is walked on its own; within one root a directory is
        // entered once, however many symlinks point at it.
        walker.seen.clear();
        let root_id = indexed.get(root).map_or(0, |(id, _)| *id);
        walker.visit(ordinal, root_id, root, Path::new(""), root)?;
        for old in previous.iter().filter(|folder| folder.root == root_id) {
            if old
                .relative
                .ancestors()
                .any(|ancestor| walker.cutoff.contains(&(ordinal, ancestor.to_path_buf())))
            {
                walker.retained.insert(old.id);
                walker
                    .folders
                    .entry((ordinal, old.relative.clone()))
                    .or_insert_with(|| remembered(ordinal, root, old));
            }
        }
    }
    walker.warnings.summarize(ctx, "entry");
    Ok(Inventory {
        forest: walker.folders,
        retained: walker.retained,
        unreadable: walker.warnings.total(),
    })
}

fn remembered(ordinal: usize, root: &Path, old: &KnownFolder) -> Folder {
    Folder {
        root: ordinal,
        relative: old.relative.clone(),
        absolute: root.join(&old.relative),
        hash: old.hash.clone(),
        signature: old.signature.clone(),
        ignored: old.ignored,
        files: Vec::new(),
    }
}

struct Walker<'a, F> {
    ctx: &'a Ctx,
    seen: HashSet<PathBuf>,
    folders: Forest,
    warnings: super::warnings::Warnings,
    retained: HashSet<i64>,
    cutoff: HashSet<(usize, PathBuf)>,
    old: BTreeMap<(i64, &'a Path), &'a KnownFolder>,
    scope: Option<&'a HashSet<PathBuf>>,
    stop: &'a F,
}

struct Entries {
    children: Vec<(PathBuf, PathBuf)>,
    files: Vec<File>,
    ignored: bool,
}

impl<F: Fn() -> bool> Walker<'_, F> {
    fn report(&mut self, path: &Path, reason: &std::io::Error) {
        if !self.warnings.allow() {
            return;
        }
        log!(
            self.ctx,
            warn,
            FILE_UNREADABLE,
            path = path.to_string_lossy().as_ref(),
            reason = reason.to_string().as_str()
        );
    }

    /// Walk one root depth first with an explicit stack: a deep or
    /// symlink-looped tree must not grow the call stack.
    fn visit(
        &mut self,
        ordinal: usize,
        root_id: i64,
        root: &Path,
        relative: &Path,
        canonical: &Path,
    ) -> Fallible {
        let Some(frame) = self.open(ordinal, root_id, root, relative, canonical, 0)? else {
            return Ok(());
        };
        let mut stack = vec![frame];
        while let Some(top) = stack.last_mut() {
            if (self.stop)() {
                return Ok(());
            }
            if let Some((child, target)) = top.children.get(top.next).cloned() {
                top.next += 1;
                let depth = top.depth + 1;
                if let Some(name) = child.file_name() {
                    top.signature.path(Path::new(name));
                }
                match self.open(ordinal, root_id, root, &child, &target, depth)? {
                    Some(frame) => stack.push(frame),
                    None => self.carry(&mut stack, ordinal, &child),
                }
                continue;
            }
            let Some(frame) = stack.pop() else { break };
            let relative = frame.relative.clone();
            let folder = close(frame);
            self.folders.insert((ordinal, relative.clone()), folder);
            self.carry(&mut stack, ordinal, &relative);
        }
        Ok(())
    }

    /// Fold a finished or remembered child's signature into its parent.
    fn carry(&self, stack: &mut [Frame], ordinal: usize, relative: &Path) {
        if let Some(parent) = stack.last_mut()
            && let Some(child) = self.folders.get(&(ordinal, relative.to_path_buf()))
        {
            parent.signature.text(&child.signature);
        }
    }

    fn open(
        &mut self,
        ordinal: usize,
        root_id: i64,
        root: &Path,
        relative: &Path,
        canonical: &Path,
        depth: usize,
    ) -> Fallible<Option<Frame>> {
        let absolute = root.join(relative);
        if !self.seen.insert(canonical.to_path_buf()) {
            return Ok(None);
        }
        if depth > MAX_DEPTH {
            self.report(
                &absolute,
                &std::io::Error::other("directory nesting is too deep"),
            );
            return Ok(None);
        }
        if let Some(scope) = self.scope
            && let Some(old) = self.old.get(&(root_id, relative))
            && !scope.iter().any(|path| {
                path.starts_with(&absolute)
                    || path.starts_with(canonical)
                    || absolute.starts_with(path)
                    || canonical.starts_with(path)
            })
        {
            self.cutoff.insert((ordinal, relative.to_path_buf()));
            self.folders.insert(
                (ordinal, relative.to_path_buf()),
                remembered(ordinal, root, old),
            );
            return Ok(None);
        }
        let entries = match fs::read_dir(canonical) {
            Ok(entries) => entries,
            Err(error) if !relative.as_os_str().is_empty() => {
                self.report(&absolute, &error);
                if let Some(old) = self.old.get(&(root_id, relative)) {
                    self.cutoff.insert((ordinal, relative.to_path_buf()));
                    self.folders.insert(
                        (ordinal, relative.to_path_buf()),
                        remembered(ordinal, root, old),
                    );
                }
                return Ok(None);
            }
            Err(error) => return Err(crate::failures::io(error)),
        };
        let Entries {
            mut children,
            mut files,
            ignored,
        } = self.entries(entries, root, relative, &absolute);
        files.sort_unstable_by(|a, b| a.relative.cmp(&b.relative));
        children.sort_unstable_by(|a, b| a.0.cmp(&b.0));
        Ok(Some(Frame {
            ordinal,
            relative: relative.to_path_buf(),
            absolute,
            depth,
            children,
            next: 0,
            files,
            ignored,
            signature: Digest::default(),
        }))
    }
    fn entries(
        &mut self,
        entries: fs::ReadDir,
        root: &Path,
        relative: &Path,
        absolute: &Path,
    ) -> Entries {
        let mut children = Vec::new();
        let mut files = Vec::new();
        let mut ignored = false;
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    self.report(absolute, &error);
                    continue;
                }
            };
            let name = entry.file_name();
            if super::paths::IGNORE_FILES
                .iter()
                .any(|known| name == *known)
            {
                ignored = true;
            }
            let source = entry.path();
            let kind = match entry.file_type() {
                Ok(kind) => kind,
                Err(error) => {
                    self.report(&source, &error);
                    continue;
                }
            };
            let target = if kind.is_symlink() {
                match source.canonicalize() {
                    Ok(path) if path.starts_with(root) => path,
                    Ok(_) => continue,
                    Err(error) => {
                        self.report(&source, &error);
                        continue;
                    }
                }
            } else {
                source
            };
            if kind.is_dir() || (kind.is_symlink() && target.is_dir()) {
                if !self.seen.contains(&target) {
                    children.push((relative.join(&name), target));
                }
                continue;
            }
            let Some(kind) = classify(Path::new(&name)) else {
                continue;
            };
            match fs::metadata(&target).and_then(|metadata| {
                if metadata.is_file() {
                    Stamp::from(&metadata).map(Some)
                } else {
                    Ok(None)
                }
            }) {
                Ok(Some(stamp)) => files.push(File {
                    relative: relative.join(&name),
                    absolute: absolute.join(&name),
                    canonical: target,
                    stamp,
                    kind,
                }),
                Ok(None) => {}
                Err(error) => self.report(&target, &error),
            }
        }
        Entries {
            children,
            files,
            ignored,
        }
    }
}

/// One directory being walked: its pending children and the digests that
/// close over them.
struct Frame {
    ordinal: usize,
    relative: PathBuf,
    absolute: PathBuf,
    depth: usize,
    children: Vec<(PathBuf, PathBuf)>,
    next: usize,
    files: Vec<File>,
    ignored: bool,
    signature: Digest,
}

fn close(frame: Frame) -> Folder {
    let Frame {
        ordinal,
        relative,
        absolute,
        files,
        ignored,
        mut signature,
        ..
    } = frame;
    let mut hash = Digest::default();
    hash.number(u64::from(ignored));
    for file in &files {
        if let Some(name) = file.relative.file_name() {
            signature.path(Path::new(name));
            hash.path(Path::new(name));
        }
        signature.text(file.kind.name());
        hash.path(&file.canonical);
        hash.number(file.stamp.size);
        hash.text(&file.stamp.mtime());
        hash.text(file.kind.name());
    }
    let signature = signature.finish();
    hash.text(&signature);
    Folder {
        root: ordinal,
        relative,
        absolute,
        hash: hash.finish(),
        signature,
        ignored,
        files,
    }
}
