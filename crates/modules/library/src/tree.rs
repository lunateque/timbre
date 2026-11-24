use crate::Library;
use crate::config::shared_db;
use crate::store::{Window, all, id, one, row_path, unsigned, window};
use timbre_edge_sqlite::rusqlite::types::Type;
use timbre_edge_sqlite::rusqlite::{Connection, Error, params};
use timbre_kernel::library_tree::{Folder, FolderEntry, LibraryFile, LibraryRoot, LibraryTree};
use timbre_kernel::{Ctx, Fallible, Page, PageRequest};

impl LibraryTree for Library {
    fn roots(&self, ctx: &Ctx, page: PageRequest<'_>) -> Fallible<Page<LibraryRoot>> {
        let win = window(page)?;
        shared_db(ctx)?.get(ctx)?.read(|sql| {
            let roots = all(sql, "SELECT r.id,f.id,r.path FROM library_roots r JOIN library_folders f ON f.root_id=r.id WHERE f.parent_id IS NULL ORDER BY r.ordinal,r.id LIMIT ?1 OFFSET ?2", params![win.limit(), win.skip()], |row| Ok(LibraryRoot { id: row.get::<_, i64>(0)?.to_string(), folder_id: row.get::<_, i64>(1)?.to_string(), path: row_path(row, 2)? }))?;
            Ok(win.page(roots))
        })
    }

    fn folder(&self, ctx: &Ctx, folder: &str, page: PageRequest<'_>) -> Fallible<Folder> {
        let win = window(page)?;
        let folder = id(folder)?;
        shared_db(ctx)?
            .get(ctx)?
            .read(|sql| read_folder(sql, folder, win))
    }

    fn tree_version(&self, ctx: &Ctx) -> Fallible<u64> {
        shared_db(ctx)?.get(ctx)?.read(|sql| {
            one(
                sql,
                "SELECT tree_version FROM library_state WHERE id=1",
                [],
                |row| unsigned(row, 0),
            )
        })
    }
}

fn read_folder(sql: &Connection, id: i64, win: Window) -> Fallible<Folder> {
    let mut folder = one(
        sql,
        "SELECT root_id,parent_id,name,relative,ignored FROM library_folders WHERE id=?1",
        [id],
        |row| {
            Ok(Folder {
                id: id.to_string(),
                root_id: row.get::<_, i64>(0)?.to_string(),
                parent_id: row.get::<_, Option<i64>>(1)?.map(|id| id.to_string()),
                name: row_path(row, 2)?.into_os_string(),
                relative_path: row_path(row, 3)?,
                ignored: row.get(4)?,
                children: win.page(Vec::new()),
            })
        },
    )?;
    let children = all(
        sql,
        "SELECT 0 AS entry,sort_name,name,id,ignored,'' AS kind FROM library_folders WHERE parent_id=?1 UNION ALL SELECT 1,sort_name,name,track_id,0,kind FROM library_files WHERE folder_id=?1 ORDER BY entry,sort_name,name,id LIMIT ?2 OFFSET ?3",
        params![id, win.limit(), win.skip()],
        |row| {
            let name = row_path(row, 2)?.into_os_string();
            if row.get::<_, i64>(0)? == 0 {
                return Ok(FolderEntry::Folder {
                    id: row.get::<_, i64>(3)?.to_string(),
                    name,
                    ignored: row.get(4)?,
                });
            }
            let kind: String = row.get(5)?;
            let kind = match kind.as_str() {
                "audio" => LibraryFile::Audio {
                    track_id: row.get::<_, i64>(3)?.to_string(),
                },
                "image" => LibraryFile::Image,
                "lyrics" => LibraryFile::Lyrics,
                // The scan writes no other kind, so the row is corrupt.
                other => {
                    return Err(Error::FromSqlConversionFailure(
                        5,
                        Type::Text,
                        Box::new(std::io::Error::other(format!("unknown file kind {other}"))),
                    ));
                }
            };
            Ok(FolderEntry::File { name, kind })
        },
    )?;
    folder.children = win.page(children);
    Ok(folder)
}
