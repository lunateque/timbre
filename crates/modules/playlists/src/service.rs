//! Implementation of the kernel `Playlists` port.

use crate::PlaylistsModule;
use crate::config::shared_db;
use crate::edits;
use crate::failures::PlaylistsFailure;
use crate::log_events;
use crate::store::{self, NewPlaylist, PlaylistRow};
use crate::validate::{
    check_actor, check_comment, check_name, check_playlist_id, check_track_id, new_playlist_id,
    stored_owner,
};
use std::collections::{HashMap, HashSet};
use std::time::Duration;
use timbre_kernel::catalog::Catalog;
use timbre_kernel::clock::{from_unix_millis, now_unix_millis};
use timbre_kernel::identity::UserName;
use timbre_kernel::page::{MAX_PAGE_LIMIT, Page, PageRequest, Window};
use timbre_kernel::playlists::{
    CreatePlaylist, PlaylistDetail, PlaylistSummary, Playlists, UpdatePlaylist,
};
use timbre_kernel::{Ctx, Fallible, log};

/// Resolve track durations for `ids` via lightweight `Catalog::track_durations`,
/// or return `None` when no `Catalog` is registered.
pub(crate) fn catalog_durations(
    ctx: &Ctx,
    ids: &[&str],
) -> Fallible<Option<HashMap<String, Duration>>> {
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
    let mut durations = HashMap::with_capacity(unique.len());
    for chunk in unique.chunks(MAX_PAGE_LIMIT) {
        for (id, dur) in catalog.track_durations(ctx, chunk)? {
            durations.insert(id, dur);
        }
    }
    Ok(Some(durations))
}

/// Compute `(song_count, duration)` from deduplicated `(track_id, count)` rows.
pub(crate) fn summary_from_counts(
    ctx: &Ctx,
    counts: &[(String, u64)],
) -> Fallible<(u64, Duration)> {
    let refs: Vec<&str> = counts.iter().map(|(id, _)| id.as_str()).collect();
    let Some(map) = catalog_durations(ctx, &refs)? else {
        let total = counts
            .iter()
            .fold(0u64, |acc, (_, c)| acc.saturating_add(*c));
        return Ok((total, Duration::ZERO));
    };
    Ok(tally_counts(counts, &map))
}

fn tally_counts(counts: &[(String, u64)], map: &HashMap<String, Duration>) -> (u64, Duration) {
    let mut live_count = 0u64;
    let mut total = Duration::ZERO;
    for (id, occurrences) in counts {
        if let Some(&dur) = map.get(id) {
            live_count = live_count.saturating_add(*occurrences);
            let repeats = u32::try_from(*occurrences).unwrap_or(u32::MAX);
            total = total.saturating_add(dur.saturating_mul(repeats));
        }
    }
    (live_count, total)
}

/// Filter `raw_entries` against the catalog when present, returning the
/// visible track list, live count and combined duration.
fn visible_entries(ctx: &Ctx, raw_entries: Vec<String>) -> Fallible<(Vec<String>, u64, Duration)> {
    let refs: Vec<&str> = raw_entries.iter().map(String::as_str).collect();
    let Some(durations) = catalog_durations(ctx, &refs)? else {
        let count = u64::try_from(raw_entries.len()).unwrap_or(u64::MAX);
        return Ok((raw_entries, count, Duration::ZERO));
    };
    let mut kept = Vec::with_capacity(raw_entries.len());
    let mut total = Duration::ZERO;
    for id in raw_entries {
        if let Some(&dur) = durations.get(&id) {
            total = total.saturating_add(dur);
            kept.push(id);
        }
    }
    let count = u64::try_from(kept.len()).unwrap_or(u64::MAX);
    Ok((kept, count, total))
}

pub(crate) fn to_summary(
    row: PlaylistRow,
    song_count: u64,
    duration: Duration,
) -> Fallible<PlaylistSummary> {
    Ok(PlaylistSummary {
        id: row.id,
        name: row.name,
        comment: row.comment,
        owner: stored_owner(&row.owner)?,
        public: row.public,
        song_count,
        duration,
        created_at: from_unix_millis(row.created_at),
        updated_at: from_unix_millis(row.updated_at),
    })
}

/// Enforce read visibility: another user's private playlist answers `NotFound`
/// so callers cannot probe whether a playlist id exists.
fn check_readable(row: &PlaylistRow, actor: &UserName) -> Fallible {
    if row.owner != actor.as_str() && !row.public {
        return Err(PlaylistsFailure::NotFound.boxed());
    }
    Ok(())
}

/// Enforce ownership for mutations: private playlists of other users answer
/// `NotFound`; public playlists of other users answer `Forbidden`.
pub(crate) fn check_owner(row: &PlaylistRow, actor: &UserName) -> Fallible {
    if row.owner == actor.as_str() {
        return Ok(());
    }
    if row.public {
        Err(PlaylistsFailure::Forbidden.boxed())
    } else {
        Err(PlaylistsFailure::NotFound.boxed())
    }
}

impl Playlists for PlaylistsModule {
    fn list(
        &self,
        ctx: &Ctx,
        actor: &UserName,
        owner: Option<&UserName>,
        page: PageRequest<'_>,
    ) -> Fallible<Page<PlaylistSummary>> {
        check_actor(ctx, actor)?;
        let mut win = Window::new(page)?;
        let (rows, has_more, counts_by_list) =
            shared_db(ctx)?.get(ctx)?.read_transaction(|sql| {
                let mut rows = store::list_visible(sql, actor, owner, win)?;
                // Truncate the lookahead row before loading entry counts.
                let more = win.advance(rows.len());
                rows.truncate(win.count());
                let mut counts = Vec::with_capacity(rows.len());
                for row in &rows {
                    counts.push(store::entry_counts_of(sql, &row.id)?);
                }
                Ok((rows, more, counts))
            })?;
        // Collect borrowed `&str` refs across the page without cloning Strings.
        let flat_refs: Vec<&str> = counts_by_list
            .iter()
            .flat_map(|list| list.iter().map(|(id, _)| id.as_str()))
            .collect();
        let durations = catalog_durations(ctx, &flat_refs)?;
        let mut summaries = Vec::with_capacity(rows.len());
        for (row, counts) in rows.into_iter().zip(counts_by_list) {
            let (count, duration) = if let Some(ref map) = durations {
                tally_counts(&counts, map)
            } else {
                let total = counts
                    .iter()
                    .fold(0u64, |acc, (_, c)| acc.saturating_add(*c));
                (total, Duration::ZERO)
            };
            match to_summary(row, count, duration) {
                Ok(summary) => summaries.push(summary),
                Err(failure) if failure.code() == crate::failures::STORED_OWNER_INVALID => {
                    log!(ctx, warn, log_events::STORED_OWNER_INVALID);
                }
                Err(failure) => return Err(failure),
            }
        }
        let next = has_more.then(|| timbre_kernel::Cursor::new(win.offset().to_string()));
        Ok(Page {
            items: summaries,
            next,
        })
    }

    fn get(
        &self,
        ctx: &Ctx,
        actor: &UserName,
        id: &str,
        entries: PageRequest<'_>,
    ) -> Fallible<PlaylistDetail> {
        check_actor(ctx, actor)?;
        let id = check_playlist_id(id)?;
        let win = Window::new(entries)?;
        let (row, raw_entries) = shared_db(ctx)?.get(ctx)?.read_transaction(|sql| {
            let row = store::by_id(sql, id)?.ok_or_else(|| PlaylistsFailure::NotFound.boxed())?;
            check_readable(&row, actor)?;
            let tracks = store::entries_of(sql, &row.id)?;
            Ok((row, tracks))
        })?;
        let (kept, song_count, duration) = visible_entries(ctx, raw_entries)?;
        let page_slice: Vec<String> = kept
            .into_iter()
            .skip(win.offset())
            .take(win.lookahead())
            .collect();
        Ok(PlaylistDetail {
            summary: to_summary(row, song_count, duration)?,
            entries: win.page(page_slice),
        })
    }

    fn create(
        &self,
        ctx: &Ctx,
        actor: &UserName,
        input: &CreatePlaylist<'_>,
    ) -> Fallible<PlaylistSummary> {
        check_actor(ctx, actor)?;
        let settings = self.settings()?;
        let name = check_name(input.name)?;
        let comment = check_comment(input.comment)?;
        if input.tracks.len() > settings.max_tracks_per_playlist {
            return Err(PlaylistsFailure::LimitReached(
                "Playlist exceeds the configured track limit.",
            )
            .boxed());
        }
        for &track in input.tracks {
            check_track_id(track)?;
        }
        let id = new_playlist_id()?;
        let now = now_unix_millis();
        shared_db(ctx)?.get(ctx)?.write(|sql| {
            store::insert(
                sql,
                NewPlaylist {
                    id: &id,
                    owner: actor,
                    name,
                    comment,
                    public: input.public,
                    now,
                    tracks: input.tracks,
                    max_per_user: settings.max_playlists_per_user,
                    max_total_entries: settings.max_total_entries_per_user,
                },
            )
        })?;
        let raw_entries: Vec<String> = input.tracks.iter().map(|&s| s.to_owned()).collect();
        let (_, song_count, duration) = visible_entries(ctx, raw_entries)?;
        log!(
            ctx,
            info,
            log_events::CREATED,
            id = id.as_str(),
            owner = actor.as_str(),
            public = input.public,
            tracks = song_count
        );
        Ok(PlaylistSummary {
            id,
            name: name.to_owned(),
            comment: comment.map(str::to_owned),
            owner: actor.clone(),
            public: input.public,
            song_count,
            duration,
            created_at: from_unix_millis(now),
            updated_at: from_unix_millis(now),
        })
    }

    fn update(
        &self,
        ctx: &Ctx,
        actor: &UserName,
        id: &str,
        changes: &UpdatePlaylist<'_>,
    ) -> Fallible<PlaylistSummary> {
        let settings = self.settings()?;
        edits::run_update(ctx, actor, id, changes, settings)
    }

    fn delete(&self, ctx: &Ctx, actor: &UserName, id: &str) -> Fallible {
        check_actor(ctx, actor)?;
        let id = check_playlist_id(id)?;
        shared_db(ctx)?.get(ctx)?.write(|sql| {
            let row = store::by_id(sql, id)?.ok_or_else(|| PlaylistsFailure::NotFound.boxed())?;
            check_owner(&row, actor)?;
            store::delete_by_id(sql, id)
        })?;
        log!(
            ctx,
            info,
            log_events::DELETED,
            id = id,
            owner = actor.as_str()
        );
        Ok(())
    }
}
