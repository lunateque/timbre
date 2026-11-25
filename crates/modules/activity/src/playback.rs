//! Scrobble, play queue and bookmark logic separated from the trait dispatch.

use crate::ActivityModule;
use crate::config::shared_db;
use crate::failures::ActivityFailure;
use crate::filter::live_ids;
use crate::log_events;
use crate::store::playback as db;
use crate::validate::{
    check_actor, check_client, check_comment, check_id, check_position_ms, check_scrobble_time,
};
use std::time::{Duration, SystemTime};
use timbre_kernel::activity::{ItemKind, PlayQueue, SaveBookmark, SavePlayQueue, Scrobble};
use timbre_kernel::catalog::Catalog;
use timbre_kernel::clock::{from_unix_millis, now_unix_millis, unix_millis};
use timbre_kernel::identity::UserName;
use timbre_kernel::signals::Played;
use timbre_kernel::{Ctx, Fallible, log};

pub(crate) fn scrobble(
    module: &ActivityModule,
    ctx: &Ctx,
    user: &UserName,
    entry: &Scrobble<'_>,
) -> Fallible {
    check_actor(ctx, user)?;
    let raw_track = check_id(entry.track)?;
    let client = check_client(entry.client)?;
    let now = SystemTime::now();
    // Resolve against the catalog outside any SQLite write lock so a missing
    // track never records a bogus play or publishes `Played`, and always store
    // the catalog's canonical track id.
    let (track_id, album_id) = if let Some(catalog) = ctx.ports().get::<dyn Catalog>() {
        match catalog.track(ctx, raw_track) {
            Ok(track) => (track.id, Some(track.album_id)),
            Err(failure) if failure.status() == 404 => {
                return Err(
                    ActivityFailure::NotFound("The requested track was not found.").boxed(),
                );
            }
            Err(failure) => return Err(failure),
        }
    } else {
        (raw_track.to_owned(), None)
    };
    if entry.submission {
        let at = match entry.at {
            Some(ts) => check_scrobble_time(ts, now)?,
            None => now,
        };
        let at_ms = unix_millis(at);
        shared_db(ctx)?
            .get(ctx)?
            .write(|sql| db::record_play(sql, user, &track_id, album_id.as_deref(), at_ms))?;
        module.now_playing.clear_completed(user, client, &track_id);
        log!(
            ctx,
            info,
            log_events::PLAYED,
            user = user.as_str(),
            track = track_id.as_str(),
            client = client
        );
        // Publish only after the transaction commits so downstream scrobblers
        // never observe a rolled-back play.
        ctx.publish(&Played {
            user: user.clone(),
            track: track_id,
            at: from_unix_millis(at_ms),
        });
    } else {
        let settings = module.settings()?;
        if let Some(reason) = module.now_playing.record(
            user.clone(),
            client.to_owned(),
            track_id.clone(),
            now,
            Duration::from_secs(settings.now_playing_ttl_secs),
            settings.now_playing_max_entries,
        ) {
            log!(
                ctx,
                info,
                log_events::NOW_PLAYING_EVICTED,
                user = user.as_str(),
                reason = reason
            );
        }
        log!(
            ctx,
            info,
            log_events::NOW_PLAYING,
            user = user.as_str(),
            track = track_id.as_str(),
            client = client
        );
    }
    Ok(())
}

pub(crate) fn play_queue(ctx: &Ctx, user: &UserName) -> Fallible<Option<PlayQueue>> {
    check_actor(ctx, user)?;
    let Some(raw) = shared_db(ctx)?
        .get(ctx)?
        .read_transaction(|sql| db::read_queue(sql, user))?
    else {
        return Ok(None);
    };
    let track_refs: Vec<&str> = raw.tracks.iter().map(String::as_str).collect();
    let (tracks, current_index, position_ms) =
        if let Some(live) = live_ids(ctx, ItemKind::Track, &track_refs)? {
            let mut kept = Vec::with_capacity(raw.tracks.len());
            let mut new_index = None;
            for (old_index, track) in raw.tracks.into_iter().enumerate() {
                if !live.contains(&track) {
                    continue;
                }
                if raw.current_index == Some(old_index) {
                    new_index = Some(kept.len());
                }
                kept.push(track);
            }
            if kept.is_empty() {
                return Ok(None);
            }
            // Reset position when the track that held `current_index` vanished.
            let pos = if raw.current_index.is_some() && new_index.is_none() {
                0
            } else {
                raw.position_ms
            };
            (kept, new_index, pos)
        } else {
            (raw.tracks, raw.current_index, raw.position_ms)
        };
    Ok(Some(PlayQueue {
        tracks,
        current_index,
        position_ms,
        changed_by: raw.changed_by,
        changed_at: from_unix_millis(raw.changed_at),
    }))
}

pub(crate) fn save_play_queue(
    module: &ActivityModule,
    ctx: &Ctx,
    user: &UserName,
    queue: &SavePlayQueue<'_>,
) -> Fallible {
    check_actor(ctx, user)?;
    let settings = module.settings()?;
    if queue.tracks.is_empty() {
        if queue.current_index.is_some() {
            return Err(
                ActivityFailure::InvalidInput("Current play queue index is out of range.").boxed(),
            );
        }
        shared_db(ctx)?
            .get(ctx)?
            .write(|sql| db::save_queue(sql, user, &[], None, 0, "", 0))?;
        log!(ctx, info, log_events::QUEUE_CLEARED, user = user.as_str());
        return Ok(());
    }
    if queue.tracks.len() > settings.max_queue_tracks {
        return Err(ActivityFailure::LimitReached(
            "Play queue exceeds the configured track limit.",
        )
        .boxed());
    }
    if let Some(index) = queue.current_index
        && index >= queue.tracks.len()
    {
        return Err(
            ActivityFailure::InvalidInput("Current play queue index is out of range.").boxed(),
        );
    }
    let position_ms = check_position_ms(queue.position_ms)?;
    let changed_by = check_client(queue.changed_by)?;
    for &track in queue.tracks {
        check_id(track)?;
    }
    let now = now_unix_millis();
    shared_db(ctx)?.get(ctx)?.write(|sql| {
        db::save_queue(
            sql,
            user,
            queue.tracks,
            queue.current_index,
            position_ms,
            changed_by,
            now,
        )
    })?;
    log!(
        ctx,
        info,
        log_events::QUEUE_SAVED,
        user = user.as_str(),
        tracks = queue.tracks.len(),
        client = changed_by
    );
    Ok(())
}

pub(crate) fn save_bookmark(
    module: &ActivityModule,
    ctx: &Ctx,
    user: &UserName,
    bookmark: &SaveBookmark<'_>,
) -> Fallible {
    check_actor(ctx, user)?;
    let settings = module.settings()?;
    let track = check_id(bookmark.track)?;
    let position_ms = check_position_ms(bookmark.position_ms)?;
    let comment = check_comment(bookmark.comment)?;
    let now = now_unix_millis();
    shared_db(ctx)?.get(ctx)?.write(|sql| {
        db::upsert_bookmark(
            sql,
            user,
            track,
            position_ms,
            comment,
            now,
            settings.max_bookmarks_per_user,
        )
    })?;
    log!(
        ctx,
        info,
        log_events::BOOKMARK_SAVED,
        user = user.as_str(),
        track = track,
        position_ms = bookmark.position_ms
    );
    Ok(())
}

pub(crate) fn delete_bookmark(ctx: &Ctx, user: &UserName, track: &str) -> Fallible {
    check_actor(ctx, user)?;
    let track = check_id(track)?;
    shared_db(ctx)?
        .get(ctx)?
        .write(|sql| db::delete_bookmark(sql, user, track))?;
    log!(
        ctx,
        info,
        log_events::BOOKMARK_DELETED,
        user = user.as_str(),
        track = track
    );
    Ok(())
}
