//! Implementation of the kernel `Activity` port.

use crate::ActivityModule;
use crate::config::shared_db;
use crate::failures::ActivityFailure;
use crate::filter::{filtered_page, live_ids, verify_existing};
use crate::log_events;
use crate::playback;
use crate::store::{self, playback as playback_db};
use crate::validate::{check_actor, check_id, kind_name};
use std::collections::{BTreeSet, HashMap};
use std::time::{Duration, SystemTime};
use timbre_kernel::activity::{
    Activity, AlbumActivityList, AlbumStats, Bookmark, ItemKind, NowPlayingEntry, PlayQueue,
    RatedItem, SaveBookmark, SavePlayQueue, Scrobble, StarredItem, TrackStats,
};
use timbre_kernel::clock::now_unix_millis;
use timbre_kernel::directory::Directory;
use timbre_kernel::identity::UserName;
use timbre_kernel::page::{MAX_PAGE_LIMIT, Page, PageRequest, Window};
use timbre_kernel::{Ctx, Fallible, log};

fn checked_sorted_ids<'a>(ids: &[&'a str]) -> Fallible<Vec<&'a str>> {
    Window::check_limit(ids.len())?;
    let mut set = BTreeSet::new();
    for &id in ids {
        set.insert(check_id(id)?);
    }
    Ok(set.into_iter().collect())
}

/// Filter `unique` against a registered `Catalog`, returning only ids that
/// still exist (or all of `unique` when no `Catalog` is registered).
fn live_batch<'a>(ctx: &Ctx, kind: ItemKind, unique: Vec<&'a str>) -> Fallible<Vec<&'a str>> {
    if unique.is_empty() {
        return Ok(unique);
    }
    let Some(live) = live_ids(ctx, kind, &unique)? else {
        return Ok(unique);
    };
    Ok(unique.into_iter().filter(|&id| live.contains(id)).collect())
}

/// Page over sorted in-memory now-playing entries, filtering vanished tracks
/// and CLI-deleted accounts in bounded chunks rather than hydrating the full
/// table upfront.
fn filter_now_playing(
    ctx: &Ctx,
    entries: &[NowPlayingEntry],
    win: Window,
) -> Fallible<Page<NowPlayingEntry>> {
    if win.count() == 0 || entries.is_empty() {
        return Ok(Page {
            items: Vec::new(),
            next: None,
        });
    }
    let directory = ctx.ports().get::<dyn Directory>();
    let mut user_alive: HashMap<UserName, bool> = HashMap::new();
    let chunk_size = win.lookahead().clamp(64, MAX_PAGE_LIMIT);
    let mut skipped = 0usize;
    let mut kept = Vec::with_capacity(win.lookahead());
    for chunk in entries.chunks(chunk_size) {
        let track_ids: Vec<&str> = chunk.iter().map(|e| e.track.as_str()).collect();
        let live_tracks = live_ids(ctx, ItemKind::Track, &track_ids)?;
        for entry in chunk {
            if let Some(ref live) = live_tracks
                && !live.contains(&entry.track)
            {
                continue;
            }
            if let Some(ref dir) = directory {
                let alive = if let Some(&cached) = user_alive.get(&entry.user) {
                    cached
                } else {
                    let exists = dir.user(ctx, &entry.user)?.is_some();
                    user_alive.insert(entry.user.clone(), exists);
                    exists
                };
                if !alive {
                    continue;
                }
            }
            if skipped < win.offset() {
                skipped += 1;
                continue;
            }
            kept.push(entry.clone());
            if kept.len() >= win.lookahead() {
                return Ok(win.page(kept));
            }
        }
    }
    Ok(win.page(kept))
}

impl Activity for ActivityModule {
    fn star(&self, ctx: &Ctx, user: &UserName, kind: ItemKind, ids: &[&str]) -> Fallible {
        check_actor(ctx, user)?;
        let settings = self.settings()?;
        let unique = checked_sorted_ids(ids)?;
        if unique.is_empty() {
            return Ok(());
        }
        let verified = verify_existing(ctx, kind, &unique)?;
        let now = now_unix_millis();
        let added = shared_db(ctx)?.get(ctx)?.write(|sql| {
            store::star_many(sql, user, kind, &verified, now, settings.max_stars_per_user)
        })?;
        log!(
            ctx,
            info,
            log_events::STARRED,
            user = user.as_str(),
            kind = kind_name(kind),
            requested = verified.len(),
            added = added
        );
        Ok(())
    }

    fn unstar(&self, ctx: &Ctx, user: &UserName, kind: ItemKind, ids: &[&str]) -> Fallible {
        check_actor(ctx, user)?;
        let unique = checked_sorted_ids(ids)?;
        if unique.is_empty() {
            return Ok(());
        }
        let removed = shared_db(ctx)?
            .get(ctx)?
            .write(|sql| store::unstar_many(sql, user, kind, &unique))?;
        log!(
            ctx,
            info,
            log_events::UNSTARRED,
            user = user.as_str(),
            kind = kind_name(kind),
            requested = unique.len(),
            removed = removed
        );
        Ok(())
    }

    fn starred(
        &self,
        ctx: &Ctx,
        user: &UserName,
        kind: ItemKind,
        page: PageRequest<'_>,
    ) -> Fallible<Page<StarredItem>> {
        check_actor(ctx, user)?;
        let win = Window::new(page)?;
        let db = shared_db(ctx)?;
        filtered_page(
            ctx,
            kind,
            win,
            |offset, limit| {
                db.get(ctx)?
                    .read(|sql| store::starred_chunk(sql, user, kind, offset, limit))
            },
            |item| item.id.as_str(),
        )
    }

    fn starred_by_ids(
        &self,
        ctx: &Ctx,
        user: &UserName,
        kind: ItemKind,
        ids: &[&str],
    ) -> Fallible<Vec<StarredItem>> {
        check_actor(ctx, user)?;
        let live = live_batch(ctx, kind, checked_sorted_ids(ids)?)?;
        if live.is_empty() {
            return Ok(Vec::new());
        }
        shared_db(ctx)?
            .get(ctx)?
            .read_transaction(|sql| store::starred_lookup(sql, user, kind, &live))
    }

    fn set_rating(
        &self,
        ctx: &Ctx,
        user: &UserName,
        kind: ItemKind,
        id: &str,
        rating: u8,
    ) -> Fallible {
        check_actor(ctx, user)?;
        let settings = self.settings()?;
        let id = check_id(id)?;
        if rating > 5 {
            return Err(ActivityFailure::InvalidInput("Rating must be between 0 and 5.").boxed());
        }
        let target = if rating > 0 {
            let mut verified = verify_existing(ctx, kind, &[id])?;
            verified.pop().unwrap_or_else(|| id.to_owned())
        } else {
            id.to_owned()
        };
        let now = now_unix_millis();
        shared_db(ctx)?.get(ctx)?.write(|sql| {
            store::set_rating(
                sql,
                user,
                kind,
                &target,
                rating,
                now,
                settings.max_ratings_per_user,
            )
        })?;
        log!(
            ctx,
            info,
            log_events::RATED,
            user = user.as_str(),
            kind = kind_name(kind),
            item = target.as_str(),
            rating = u64::from(rating)
        );
        Ok(())
    }

    fn ratings(
        &self,
        ctx: &Ctx,
        user: &UserName,
        kind: ItemKind,
        ids: &[&str],
    ) -> Fallible<Vec<RatedItem>> {
        check_actor(ctx, user)?;
        let live = live_batch(ctx, kind, checked_sorted_ids(ids)?)?;
        if live.is_empty() {
            return Ok(Vec::new());
        }
        shared_db(ctx)?
            .get(ctx)?
            .read_transaction(|sql| store::ratings_lookup(sql, user, kind, &live))
    }

    fn scrobble(&self, ctx: &Ctx, user: &UserName, entry: &Scrobble<'_>) -> Fallible {
        playback::scrobble(self, ctx, user, entry)
    }

    fn now_playing(
        &self,
        ctx: &Ctx,
        user: Option<&UserName>,
        page: PageRequest<'_>,
    ) -> Fallible<Page<NowPlayingEntry>> {
        if let Some(target) = user {
            check_actor(ctx, target)?;
        }
        let win = Window::new(page)?;
        let settings = self.settings()?;
        let snapshot = self.now_playing.snapshot(
            user,
            SystemTime::now(),
            Duration::from_secs(settings.now_playing_ttl_secs),
        );
        filter_now_playing(ctx, &snapshot, win)
    }

    fn track_stats(&self, ctx: &Ctx, user: &UserName, ids: &[&str]) -> Fallible<Vec<TrackStats>> {
        check_actor(ctx, user)?;
        let live = live_batch(ctx, ItemKind::Track, checked_sorted_ids(ids)?)?;
        if live.is_empty() {
            return Ok(Vec::new());
        }
        shared_db(ctx)?
            .get(ctx)?
            .read_transaction(|sql| playback_db::track_stats_lookup(sql, user, &live))
    }

    fn album_stats(&self, ctx: &Ctx, user: &UserName, ids: &[&str]) -> Fallible<Vec<AlbumStats>> {
        check_actor(ctx, user)?;
        let live = live_batch(ctx, ItemKind::Album, checked_sorted_ids(ids)?)?;
        if live.is_empty() {
            return Ok(Vec::new());
        }
        shared_db(ctx)?
            .get(ctx)?
            .read_transaction(|sql| playback_db::album_stats_lookup(sql, user, &live))
    }

    fn top_tracks(
        &self,
        ctx: &Ctx,
        user: Option<&UserName>,
        page: PageRequest<'_>,
    ) -> Fallible<Page<TrackStats>> {
        if let Some(target) = user {
            check_actor(ctx, target)?;
        }
        let win = Window::new(page)?;
        let db = shared_db(ctx)?;
        filtered_page(
            ctx,
            ItemKind::Track,
            win,
            |offset, limit| {
                db.get(ctx)?
                    .read(|sql| playback_db::top_tracks_chunk(sql, user, offset, limit))
            },
            |item| item.track.as_str(),
        )
    }

    fn albums(
        &self,
        ctx: &Ctx,
        user: &UserName,
        list: AlbumActivityList,
        page: PageRequest<'_>,
    ) -> Fallible<Page<String>> {
        check_actor(ctx, user)?;
        let win = Window::new(page)?;
        let db = shared_db(ctx)?;
        filtered_page(
            ctx,
            ItemKind::Album,
            win,
            |offset, limit| {
                db.get(ctx)?
                    .read(|sql| playback_db::albums_chunk(sql, user, list, offset, limit))
            },
            String::as_str,
        )
    }

    fn play_queue(&self, ctx: &Ctx, user: &UserName) -> Fallible<Option<PlayQueue>> {
        playback::play_queue(ctx, user)
    }

    fn save_play_queue(&self, ctx: &Ctx, user: &UserName, queue: &SavePlayQueue<'_>) -> Fallible {
        playback::save_play_queue(self, ctx, user, queue)
    }

    fn bookmarks(
        &self,
        ctx: &Ctx,
        user: &UserName,
        page: PageRequest<'_>,
    ) -> Fallible<Page<Bookmark>> {
        check_actor(ctx, user)?;
        let win = Window::new(page)?;
        let db = shared_db(ctx)?;
        filtered_page(
            ctx,
            ItemKind::Track,
            win,
            |offset, limit| {
                db.get(ctx)?
                    .read(|sql| playback_db::bookmarks_chunk(sql, user, offset, limit))
            },
            |item| item.track.as_str(),
        )
    }

    fn save_bookmark(&self, ctx: &Ctx, user: &UserName, bookmark: &SaveBookmark<'_>) -> Fallible {
        playback::save_bookmark(self, ctx, user, bookmark)
    }

    fn delete_bookmark(&self, ctx: &Ctx, user: &UserName, track: &str) -> Fallible {
        playback::delete_bookmark(ctx, user, track)
    }
}
