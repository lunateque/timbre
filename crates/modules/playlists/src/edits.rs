//! Playlist mutation logic (`update`), separated from read/create dispatch to
//! keep concurrency handling, O(N) compaction and cap enforcement concise.

use crate::config::{Settings, shared_db};
use crate::failures::PlaylistsFailure;
use crate::log_events;
use crate::service::{catalog_durations, check_owner, summary_from_counts, to_summary};
use crate::store::{self, PlaylistRow};
use crate::validate::{check_actor, check_comment, check_name, check_playlist_id, check_track_id};
use std::collections::HashSet;
use timbre_kernel::clock::now_unix_millis;
use timbre_kernel::identity::UserName;
use timbre_kernel::playlists::{PlaylistSummary, UpdatePlaylist};
use timbre_kernel::{Ctx, Fallible, log};

/// Maximum retries when an index-based removal races with a concurrent writer.
const MAX_RACE_RETRIES: usize = 3;

/// Compact `stored` in a single O(N) pass after applying replacement, catalog
/// liveness pruning, index removals and appended tracks.
fn apply_entry_edits(
    stored: Vec<String>,
    live_before: Option<&HashSet<String>>,
    changes: &UpdatePlaylist<'_>,
    max_tracks: usize,
) -> Fallible<Vec<String>> {
    let base: Vec<String> = if let Some(replaced) = changes.replace_tracks {
        replaced.iter().map(|&s| s.to_owned()).collect()
    } else if let Some(live) = live_before {
        stored.into_iter().filter(|t| live.contains(t)).collect()
    } else {
        stored
    };
    let mut list = if changes.remove_indices.is_empty() {
        base
    } else {
        let mut to_remove = HashSet::with_capacity(changes.remove_indices.len());
        for &index in changes.remove_indices {
            if index >= base.len() {
                return Err(PlaylistsFailure::InvalidInput(
                    "Playlist track index to remove is out of range.",
                )
                .boxed());
            }
            to_remove.insert(index);
        }
        let mut kept = Vec::with_capacity(base.len().saturating_sub(to_remove.len()));
        for (index, track) in base.into_iter().enumerate() {
            if !to_remove.contains(&index) {
                kept.push(track);
            }
        }
        kept
    };
    if list.len().saturating_add(changes.add_tracks.len()) > max_tracks {
        return Err(
            PlaylistsFailure::LimitReached("Playlist exceeds the configured track limit.").boxed(),
        );
    }
    for &track in changes.add_tracks {
        list.push(track.to_owned());
    }
    Ok(list)
}

enum CommentEdit<'a> {
    Keep,
    Set(Option<&'a str>),
}

fn validate_update_inputs<'a>(
    changes: &UpdatePlaylist<'a>,
    max_tracks: usize,
) -> Fallible<(Option<&'a str>, CommentEdit<'a>)> {
    let new_name = changes.name.map(check_name).transpose()?;
    let new_comment = match changes.comment {
        None => CommentEdit::Keep,
        Some(inner) => CommentEdit::Set(check_comment(inner)?),
    };
    // Bound every input slice before inspecting items or acquiring the write lock.
    if let Some(tracks) = changes.replace_tracks {
        if tracks.len() > max_tracks {
            return Err(PlaylistsFailure::LimitReached(
                "Playlist exceeds the configured track limit.",
            )
            .boxed());
        }
        for &track in tracks {
            check_track_id(track)?;
        }
    }
    if changes.add_tracks.len() > max_tracks {
        return Err(
            PlaylistsFailure::LimitReached("Playlist exceeds the configured track limit.").boxed(),
        );
    }
    if changes.remove_indices.len() > max_tracks {
        return Err(PlaylistsFailure::InvalidInput(
            "Playlist track index to remove is out of range.",
        )
        .boxed());
    }
    for &track in changes.add_tracks {
        check_track_id(track)?;
    }
    Ok((new_name, new_comment))
}

enum WriteAttempt {
    Done(PlaylistRow),
    Retry,
}

#[expect(
    clippy::too_many_arguments,
    reason = "internal helper carries pre-validated update fields into the single write transaction"
)]
fn write_update(
    sql: &timbre_edge_sqlite::rusqlite::Connection,
    actor: &UserName,
    id: &str,
    new_name: Option<&str>,
    new_comment: &CommentEdit<'_>,
    changes: &UpdatePlaylist<'_>,
    snapshot: Option<&(Vec<String>, Option<HashSet<String>>)>,
    settings: Settings,
    now: i64,
) -> Fallible<WriteAttempt> {
    let row = store::by_id(sql, id)?.ok_or_else(|| PlaylistsFailure::NotFound.boxed())?;
    check_owner(&row, actor)?;
    let name = new_name.map_or(row.name, str::to_owned);
    let comment = match new_comment {
        CommentEdit::Keep => row.comment,
        CommentEdit::Set(inner) => inner.map(str::to_owned),
    };
    let public = changes.public.unwrap_or(row.public);
    let entries_touched = changes.replace_tracks.is_some()
        || !changes.remove_indices.is_empty()
        || !changes.add_tracks.is_empty();

    if entries_touched {
        let old_len = store::entry_count_of(sql, id)?;
        let total_before = store::total_entries_for_user(sql, actor)?;
        let other_entries = total_before.saturating_sub(old_len);

        // Append-only fast path avoids deleting and rewriting existing rows.
        if changes.replace_tracks.is_none() && changes.remove_indices.is_empty() {
            let new_len = old_len.saturating_add(changes.add_tracks.len());
            if new_len > settings.max_tracks_per_playlist {
                return Err(PlaylistsFailure::LimitReached(
                    "Playlist exceeds the configured track limit.",
                )
                .boxed());
            }
            if other_entries.saturating_add(new_len) > settings.max_total_entries_per_user {
                return Err(PlaylistsFailure::LimitReached(
                    "Total playlist entry limit for this user has been reached.",
                )
                .boxed());
            }
            store::append_entries(sql, id, changes.add_tracks)?;
        } else {
            let stored = if changes.replace_tracks.is_some() {
                Vec::new()
            } else {
                store::entries_of(sql, id)?
            };
            // If `live_before` was resolved outside the write lock, verify no
            // concurrent edit changed `stored` in between.
            let live_before = if let Some((expected, live)) = snapshot {
                if stored != *expected {
                    return Ok(WriteAttempt::Retry);
                }
                live.as_ref()
            } else {
                None
            };
            let list = apply_entry_edits(
                stored,
                live_before,
                changes,
                settings.max_tracks_per_playlist,
            )?;
            if other_entries.saturating_add(list.len()) > settings.max_total_entries_per_user {
                return Err(PlaylistsFailure::LimitReached(
                    "Total playlist entry limit for this user has been reached.",
                )
                .boxed());
            }
            store::replace_entries(sql, id, &list)?;
        }
    }
    store::update_header(sql, id, &name, comment.as_deref(), public, now)?;
    Ok(WriteAttempt::Done(PlaylistRow {
        id: row.id,
        owner: row.owner,
        name,
        comment,
        public,
        created_at: row.created_at,
        updated_at: now,
    }))
}

pub(crate) fn run_update(
    ctx: &Ctx,
    actor: &UserName,
    id: &str,
    changes: &UpdatePlaylist<'_>,
    settings: Settings,
) -> Fallible<PlaylistSummary> {
    check_actor(ctx, actor)?;
    let id = check_playlist_id(id)?;
    let (new_name, new_comment) =
        validate_update_inputs(changes, settings.max_tracks_per_playlist)?;
    let needs_live_snapshot =
        changes.replace_tracks.is_none() && !changes.remove_indices.is_empty();
    let db = shared_db(ctx)?;
    let mut updated_row = None;

    for _ in 0..MAX_RACE_RETRIES {
        let snapshot = if needs_live_snapshot {
            let existing = db.get(ctx)?.read_transaction(|sql| {
                let row =
                    store::by_id(sql, id)?.ok_or_else(|| PlaylistsFailure::NotFound.boxed())?;
                check_owner(&row, actor)?;
                store::entries_of(sql, id)
            })?;
            let refs: Vec<&str> = existing.iter().map(String::as_str).collect();
            let live = catalog_durations(ctx, &refs)?
                .map(|map| map.into_keys().collect::<HashSet<String>>());
            Some((existing, live))
        } else {
            None
        };
        let now = now_unix_millis();
        match db.get(ctx)?.write(|sql| {
            write_update(
                sql,
                actor,
                id,
                new_name,
                &new_comment,
                changes,
                snapshot.as_ref(),
                settings,
                now,
            )
        })? {
            WriteAttempt::Done(row) => {
                updated_row = Some(row);
                break;
            }
            WriteAttempt::Retry => {}
        }
    }

    let row = updated_row.ok_or_else(|| PlaylistsFailure::Conflict.boxed())?;
    let counts = db.get(ctx)?.read(|sql| store::entry_counts_of(sql, id))?;
    let (song_count, duration) = summary_from_counts(ctx, &counts)?;
    log!(
        ctx,
        info,
        log_events::UPDATED,
        id = id,
        owner = actor.as_str(),
        public = row.public,
        tracks = song_count
    );
    to_summary(row, song_count, duration)
}
