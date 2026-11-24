use std::collections::BTreeMap;
use std::ops::ControlFlow;
use std::sync::Arc;

use crate::failures::BrowseFailure;
use timbre_kernel::folders::{FolderIndex, Folders, IndexedFolder, Versioned};
use timbre_kernel::library_tree::{Folder, FolderEntry, LibraryRoot, LibraryTree};
use timbre_kernel::{Ctx, Cursor, Fallible, MAX_PAGE_LIMIT, Page, PageRequest, Port};

const METADATA: PageRequest<'static> = PageRequest {
    limit: 0,
    cursor: None,
};
const CHUNK: usize = 128;

pub(crate) struct Browse;
impl Port for Browse {}

impl Folders for Browse {
    fn roots(&self, ctx: &Ctx, page: PageRequest<'_>) -> Fallible<Page<LibraryRoot>> {
        within_cap(page)?;
        tree(ctx)?.roots(ctx, page)
    }

    fn indexes(
        &self,
        ctx: &Ctx,
        root: Option<&str>,
        if_modified_since: Option<u64>,
        page: PageRequest<'_>,
    ) -> Fallible<Versioned<Page<FolderIndex>>> {
        within_cap(page)?;
        let tree = tree(ctx)?;
        let version = tree.tree_version(ctx)?;
        let unchanged = if_modified_since == Some(version);
        let mut groups = BTreeMap::<_, Vec<_>>::new();
        let mut found = root.is_none();
        walk(
            |page| tree.roots(ctx, page),
            |item| {
                if root.is_some_and(|id| id != item.id) {
                    return Ok(ControlFlow::Continue(()));
                }
                found = true;
                if !unchanged && !folder(ctx, &*tree, &item.folder_id, METADATA)?.ignored {
                    for_children(ctx, &*tree, &item.folder_id, |entry| {
                        if let FolderEntry::Folder {
                            id,
                            name,
                            ignored: false,
                        } = entry
                            && visible(ctx, &*tree, &id)?
                        {
                            let initial = name
                                .to_string_lossy()
                                .chars()
                                .next()
                                .filter(|letter| letter.is_alphabetic());
                            let letter = initial
                                .and_then(|letter| letter.to_uppercase().next())
                                .unwrap_or('#');
                            groups
                                .entry(letter)
                                .or_default()
                                .push(IndexedFolder { id, name });
                        }
                        Ok(ControlFlow::Continue(()))
                    })?;
                }
                Ok(if root.is_some() {
                    ControlFlow::Break(())
                } else {
                    ControlFlow::Continue(())
                })
            },
        )?;
        if !found {
            return Err(BrowseFailure::NotFound.boxed());
        }
        if unchanged {
            return Ok(Versioned::NotModified);
        }
        let offset = offset(page)?;
        let items = groups
            .into_iter()
            .skip(offset)
            .take(page.limit.saturating_add(1))
            .map(|(letter, folders)| FolderIndex { letter, folders })
            .collect();
        Ok(Versioned::Changed {
            version,
            value: result_page(page, offset, items),
        })
    }

    fn directory(
        &self,
        ctx: &Ctx,
        id: &str,
        if_modified_since: Option<u64>,
        page: PageRequest<'_>,
    ) -> Fallible<Versioned<Folder>> {
        within_cap(page)?;
        let tree = tree(ctx)?;
        let version = tree.tree_version(ctx)?;
        let mut directory = folder(ctx, &*tree, id, METADATA)?;
        if directory.ignored
            || ancestor_ignored(ctx, &*tree, &directory)?
            || !visible(ctx, &*tree, id)?
        {
            return Err(BrowseFailure::NotFound.boxed());
        }
        if if_modified_since == Some(version) {
            return Ok(Versioned::NotModified);
        }
        let offset = offset(page)?;
        let mut remaining = offset;
        let mut items = Vec::new();
        if page.limit != 0 {
            for_children(ctx, &*tree, id, |item| {
                if let FolderEntry::Folder { id, ignored, .. } = &item
                    && (*ignored || !visible(ctx, &*tree, id)?)
                {
                    return Ok(ControlFlow::Continue(()));
                }
                if remaining == 0 {
                    items.push(item);
                } else {
                    remaining -= 1;
                }
                Ok(if items.len() > page.limit {
                    ControlFlow::Break(())
                } else {
                    ControlFlow::Continue(())
                })
            })?;
        }
        directory.children = result_page(page, offset, items);
        Ok(Versioned::Changed {
            version,
            value: directory,
        })
    }
}

fn tree(ctx: &Ctx) -> Fallible<Arc<dyn LibraryTree>> {
    ctx.ports()
        .get::<dyn LibraryTree>()
        .ok_or_else(|| BrowseFailure::NotAvailable.boxed())
}

/// A page larger than the kernel's cap is refused, not quietly shortened: a
/// client that asked for more would read the short page as the end of a list.
fn within_cap(page: PageRequest<'_>) -> Fallible {
    if page.limit > MAX_PAGE_LIMIT {
        return Err(BrowseFailure::PageTooLarge.boxed());
    }
    Ok(())
}

/// A tree provider reports an absent folder with its own `not_found` code;
/// this port answers with `folders.not_found`, as its contract says, instead
/// of leaking the provider's.
fn folder(ctx: &Ctx, tree: &dyn LibraryTree, id: &str, page: PageRequest<'_>) -> Fallible<Folder> {
    tree.folder(ctx, id, page).map_err(|failure| {
        if failure.code().as_str().ends_with(".not_found") {
            BrowseFailure::NotFound.boxed()
        } else {
            failure
        }
    })
}

fn ancestor_ignored(ctx: &Ctx, tree: &dyn LibraryTree, start: &Folder) -> Fallible<bool> {
    let mut ancestor = start.parent_id.clone();
    while let Some(id) = ancestor {
        let parent = folder(ctx, tree, &id, METADATA)?;
        if parent.ignored {
            return Ok(true);
        }
        ancestor = parent.parent_id;
    }
    Ok(false)
}

fn visible(ctx: &Ctx, tree: &dyn LibraryTree, id: &str) -> Fallible<bool> {
    let mut pending = Vec::new();
    let mut current = id;
    let mut next;
    loop {
        if for_children(ctx, tree, current, |entry| {
            match entry {
                FolderEntry::File { .. } => return Ok(ControlFlow::Break(())),
                FolderEntry::Folder {
                    id, ignored: false, ..
                } => pending.push(id),
                FolderEntry::Folder { .. } => {}
            }
            Ok(ControlFlow::Continue(()))
        })? {
            return Ok(true);
        }
        let Some(id) = pending.pop() else {
            return Ok(false);
        };
        next = id;
        current = &next;
    }
}

fn for_children(
    ctx: &Ctx,
    tree: &dyn LibraryTree,
    id: &str,
    visit: impl FnMut(FolderEntry) -> Fallible<ControlFlow<()>>,
) -> Fallible<bool> {
    walk(
        |page| folder(ctx, tree, id, page).map(|folder| folder.children),
        visit,
    )
}

fn walk<T>(
    read: impl Fn(PageRequest<'_>) -> Fallible<Page<T>>,
    mut visit: impl FnMut(T) -> Fallible<ControlFlow<()>>,
) -> Fallible<bool> {
    let mut cursor = None;
    loop {
        let page = read(PageRequest {
            limit: CHUNK,
            cursor: cursor.as_ref(),
        })?;
        for item in page.items {
            if visit(item)?.is_break() {
                return Ok(true);
            }
        }
        cursor = page.next;
        if cursor.is_none() {
            return Ok(false);
        }
    }
}

fn offset(page: PageRequest<'_>) -> Fallible<usize> {
    page.cursor
        .map_or(Ok(0), |cursor| cursor.as_str().parse())
        .map_err(|_| BrowseFailure::InvalidCursor.boxed())
}

fn result_page<T>(request: PageRequest<'_>, offset: usize, mut items: Vec<T>) -> Page<T> {
    let next = (request.limit != 0 && items.len() > request.limit)
        .then(|| Cursor::new(offset.saturating_add(request.limit).to_string()));
    items.truncate(request.limit);
    Page { items, next }
}
