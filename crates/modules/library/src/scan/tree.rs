use super::paths::Folder;
use super::walk::Inventory;
use crate::store::{all, exec, one, path_to_bytes, row_path};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use timbre_edge_sqlite::Database;
use timbre_edge_sqlite::rusqlite::params;
use timbre_kernel::Fallible;

pub(super) struct KnownFolder {
    pub(super) id: i64,
    pub(super) root: i64,
    pub(super) relative: PathBuf,
    pub(super) hash: String,
    pub(super) signature: String,
    pub(super) ignored: bool,
}

pub(super) struct Tree {
    pub(super) roots: Vec<i64>,
    pub(super) folders: BTreeMap<(usize, PathBuf), i64>,
    pub(super) changed: BTreeSet<(usize, PathBuf)>,
    pub(super) removed: Vec<i64>,
    pub(super) removed_roots: Vec<i64>,
}

pub(super) type Roots = BTreeMap<PathBuf, (i64, i64)>;

pub(super) fn snapshot(db: &Database) -> Fallible<(Roots, Vec<KnownFolder>)> {
    db.read(|sql| {
        let roots = all(
            sql,
            "SELECT path,id,ordinal FROM library_roots",
            [],
            |row| Ok((row_path(row, 0)?, (row.get(1)?, row.get(2)?))),
        )?;
        let folders = all(
            sql,
            "SELECT id,root_id,relative,folder_hash,child_signature,ignored FROM library_folders",
            [],
            |row| {
                Ok(KnownFolder {
                    id: row.get(0)?,
                    root: row.get(1)?,
                    relative: row_path(row, 2)?,
                    hash: row.get(3)?,
                    signature: row.get(4)?,
                    ignored: row.get(5)?,
                })
            },
        )?;
        Ok((roots.into_iter().collect(), folders))
    })
}

impl Tree {
    pub(super) fn apply(
        db: &Database,
        roots: &[PathBuf],
        inventory: &Inventory,
        indexed: &BTreeMap<PathBuf, (i64, i64)>,
        previous: &[KnownFolder],
        full: bool,
    ) -> Fallible<Self> {
        let forest = &inventory.forest;
        let root_ids = sync_roots(db, roots, indexed)?;
        let removed_roots: Vec<_> = indexed
            .values()
            .map(|(id, _)| *id)
            .filter(|id| !root_ids.contains(id))
            .collect();
        let old: BTreeMap<_, _> = previous
            .iter()
            .map(|folder| ((folder.root, folder.relative.as_path()), folder))
            .collect();
        let mut missing: BTreeMap<_, Vec<_>> = BTreeMap::new();
        let mut removed = BTreeSet::new();
        for folder in previous {
            if root_ids
                .iter()
                .position(|&id| id == folder.root)
                .is_none_or(|ordinal| !forest.contains_key(&(ordinal, folder.relative.clone())))
            {
                if !folder.relative.as_os_str().is_empty() && !removed_roots.contains(&folder.root)
                {
                    missing
                        .entry((folder.root, folder.signature.as_str()))
                        .or_default()
                        .push(folder.id);
                }
                removed.insert(folder.id);
            }
        }
        let mut added = BTreeMap::<_, usize>::new();
        for ((ordinal, relative), folder) in forest {
            if let Some(&root_id) = root_ids.get(*ordinal)
                && !old.contains_key(&(root_id, relative.as_path()))
                && !relative.as_os_str().is_empty()
            {
                *added
                    .entry((root_id, folder.signature.as_str()))
                    .or_default() += 1;
            }
        }
        let mut folders = BTreeMap::new();
        let mut changed = BTreeSet::new();
        for (key, folder) in forest {
            let Some(&root_id) = root_ids.get(folder.root) else {
                continue;
            };
            let parent = folder
                .relative
                .parent()
                .and_then(|parent| folders.get(&(folder.root, parent.to_path_buf())).copied());
            let existing = old.get(&(root_id, folder.relative.as_path()));
            let relink = (existing.is_none()
                && added.get(&(root_id, folder.signature.as_str())) == Some(&1))
            .then(|| {
                match missing
                    .get(&(root_id, folder.signature.as_str()))
                    .map(Vec::as_slice)
                {
                    Some([id]) => Some(*id),
                    _ => None,
                }
            })
            .flatten();
            let id = if let Some(known) = existing.filter(|known| {
                inventory.retained.contains(&known.id) || (!full && known.hash == folder.hash)
            }) {
                known.id
            } else {
                changed.insert(key.clone());
                write_folder(
                    db,
                    folder,
                    root_id,
                    parent,
                    existing.map(|known| known.id).or(relink),
                )?
            };
            removed.remove(&id);
            folders.insert(key.clone(), id);
        }
        Ok(Self {
            roots: root_ids,
            folders,
            changed,
            removed: removed.into_iter().collect(),
            removed_roots,
        })
    }

    pub(super) fn erase_missing(&self, db: &Database) -> Fallible {
        if !self.removed.is_empty() {
            db.write(|sql| {
                for &id in &self.removed {
                    exec(sql, "DELETE FROM library_folders WHERE id=?1", [id])?;
                }
                Ok(())
            })?;
        }
        if !self.removed_roots.is_empty() {
            db.write(|sql| {
                for &id in &self.removed_roots {
                    exec(sql, "DELETE FROM library_roots WHERE id=?1", [id])?;
                }
                Ok(())
            })?;
        }
        Ok(())
    }
}

fn sync_roots(
    db: &Database,
    roots: &[PathBuf],
    indexed: &BTreeMap<PathBuf, (i64, i64)>,
) -> Fallible<Vec<i64>> {
    roots.iter().enumerate().map(|(ordinal, root)| {
        let order = i64::try_from(ordinal).unwrap_or(i64::MAX);
        if let Some(&(id, previous_order)) = indexed.get(root) && previous_order == order { return Ok(id); }
        db.write(|sql| one(sql, "INSERT INTO library_roots(path,ordinal) VALUES(?1,?2) ON CONFLICT(path) DO UPDATE SET ordinal=excluded.ordinal RETURNING id", params![path_to_bytes(root), order], |row| row.get(0)))
    }).collect()
}

fn write_folder(
    db: &Database,
    folder: &Folder,
    root: i64,
    parent: Option<i64>,
    id: Option<i64>,
) -> Fallible<i64> {
    let name = folder
        .absolute
        .file_name()
        .unwrap_or(folder.absolute.as_os_str());
    let sort_name = name.to_string_lossy();
    db.write(|sql| one(sql, "INSERT INTO library_folders(id,root_id,relative,absolute,parent_id,name,sort_name,folder_hash,child_signature,ignored) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10) ON CONFLICT(id) DO UPDATE SET root_id=excluded.root_id,relative=excluded.relative,absolute=excluded.absolute,parent_id=excluded.parent_id,name=excluded.name,sort_name=excluded.sort_name,folder_hash=excluded.folder_hash,child_signature=excluded.child_signature,ignored=excluded.ignored RETURNING id", params![id, root, path_to_bytes(&folder.relative), path_to_bytes(&folder.absolute), parent, path_to_bytes(Path::new(name)), sort_name.as_ref(), folder.hash, folder.signature, folder.ignored], |row| row.get(0)))
}
