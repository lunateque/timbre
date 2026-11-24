use super::paths::Stamp;
use crate::store::{all, row_path, unsigned};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use timbre_edge_sqlite::Database;
use timbre_kernel::Fallible;

pub(super) type Unreadable = BTreeMap<PathBuf, String>;

struct FailureStamp {
    size: u64,
    mtime: String,
    reason: String,
}

#[derive(Default)]
pub(super) struct Previous(BTreeMap<PathBuf, FailureStamp>);

impl Previous {
    pub(super) fn load(db: &Database, full: bool) -> Fallible<Self> {
        if full {
            return Ok(Self::default());
        }
        db.read(|sql| {
            let found = all(
                sql,
                "SELECT DISTINCT path,size,mtime,reason FROM library_unreadable",
                [],
                |row| {
                    Ok((
                        row_path(row, 0)?,
                        FailureStamp {
                            size: unsigned(row, 1)?,
                            mtime: row.get(2)?,
                            reason: row.get(3)?,
                        },
                    ))
                },
            )?;
            Ok(Self(found.into_iter().collect()))
        })
    }

    pub(super) fn reason(&self, path: &Path, stamp: Stamp) -> Option<&str> {
        let previous = self.0.get(path)?;
        (previous.size == stamp.size && previous.mtime == stamp.mtime()).then_some(&previous.reason)
    }
}
