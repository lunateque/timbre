//! Catalog tolerance helpers: skip deleted catalog items without N+1 queries
//! while bounding per-request scan work.

use crate::failures::ActivityFailure;
use std::collections::HashSet;
use timbre_kernel::activity::ItemKind;
use timbre_kernel::catalog::Catalog;
use timbre_kernel::page::{Cursor, MAX_PAGE_LIMIT, Page, PageRequest, Window};
use timbre_kernel::{Ctx, Fallible};

/// Maximum SQL/catalog chunks scanned in a single `filtered_page` call.
/// Bounds CPU and reader-lease time even when thousands of catalog rows
/// disappeared between scans; a caller continues through the returned cursor.
const MAX_SCAN_BATCHES: usize = 16;

/// Return the subset of `ids` that currently exist in a registered `Catalog`,
/// or `None` when no `Catalog` port is registered.
pub(crate) fn live_ids(
    ctx: &Ctx,
    kind: ItemKind,
    ids: &[&str],
) -> Fallible<Option<HashSet<String>>> {
    let Some(catalog) = ctx.ports().get::<dyn Catalog>() else {
        return Ok(None);
    };
    let mut unique: Vec<&str> = Vec::with_capacity(ids.len());
    let mut seen: HashSet<&str> = HashSet::with_capacity(ids.len());
    for &id in ids {
        if seen.insert(id) {
            unique.push(id);
        }
    }
    let mut live = HashSet::with_capacity(unique.len());
    let first_page = PageRequest {
        limit: MAX_PAGE_LIMIT,
        cursor: None,
    };
    for chunk in unique.chunks(MAX_PAGE_LIMIT) {
        match kind {
            ItemKind::Track => {
                for (id, _) in catalog.track_durations(ctx, chunk)? {
                    live.insert(id);
                }
            }
            ItemKind::Album => {
                let page = catalog.albums_by_ids(ctx, chunk, first_page)?;
                for album in page.items {
                    live.insert(album.id);
                }
            }
            ItemKind::Artist => {
                let page = catalog.artists_by_ids(ctx, chunk, first_page)?;
                for artist in page.items {
                    live.insert(artist.id);
                }
            }
        }
    }
    Ok(Some(live))
}

/// Verify every id in `unique_ids` exists in a registered `Catalog` and return
/// their canonical catalog spellings. When no `Catalog` is registered, returns
/// the validated inputs verbatim.
pub(crate) fn verify_existing(
    ctx: &Ctx,
    kind: ItemKind,
    unique_ids: &[&str],
) -> Fallible<Vec<String>> {
    let Some(live) = live_ids(ctx, kind, unique_ids)? else {
        return Ok(unique_ids.iter().map(|&id| id.to_owned()).collect());
    };
    // Because `library::store::id` rejects non-canonical integer spellings,
    // every existing id matches its canonical string in `live`.
    if live.len() != unique_ids.len() {
        return Err(ActivityFailure::NotFound("The requested catalog item was not found.").boxed());
    }
    let mut canonical: Vec<String> = live.into_iter().collect();
    canonical.sort();
    Ok(canonical)
}

/// Page over stored rows, skipping any whose catalog id disappeared when a
/// `Catalog` port is registered. Cursors encode the next raw SQL offset so
/// following `next` neither skips nor duplicates live rows.
pub(crate) fn filtered_page<T>(
    ctx: &Ctx,
    kind: ItemKind,
    win: Window,
    mut fetch: impl FnMut(usize, usize) -> Fallible<Vec<T>>,
    id_of: impl Fn(&T) -> &str,
) -> Fallible<Page<T>> {
    if win.count() == 0 {
        return Ok(Page {
            items: Vec::new(),
            next: None,
        });
    }
    if ctx.ports().get::<dyn Catalog>().is_none() {
        let rows = fetch(win.offset(), win.lookahead())?;
        return Ok(win.page(rows));
    }
    let batch = win.lookahead().clamp(64, MAX_PAGE_LIMIT);
    let mut sql_offset = win.offset();
    let mut collected = Vec::with_capacity(win.count());
    let mut batches = 0usize;
    loop {
        if batches >= MAX_SCAN_BATCHES {
            return Ok(Page {
                items: collected,
                next: Some(Cursor::new(sql_offset.to_string())),
            });
        }
        batches += 1;
        let rows = fetch(sql_offset, batch)?;
        if rows.is_empty() {
            break;
        }
        let fetched = rows.len();
        let candidate_ids: Vec<&str> = rows.iter().map(&id_of).collect();
        let Some(live) = live_ids(ctx, kind, &candidate_ids)? else {
            return Ok(win.page(rows));
        };
        for (index, row) in rows.into_iter().enumerate() {
            if !live.contains(id_of(&row)) {
                continue;
            }
            if collected.len() == win.count() {
                let next_offset = sql_offset.saturating_add(index);
                return Ok(Page {
                    items: collected,
                    next: Some(Cursor::new(next_offset.to_string())),
                });
            }
            collected.push(row);
        }
        if fetched < batch {
            break;
        }
        sql_offset = sql_offset.saturating_add(fetched);
    }
    Ok(Page {
        items: collected,
        next: None,
    })
}
