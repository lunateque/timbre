use crate::config::{Settings, shared_db};
use crate::failures::LibraryFailure;
use crate::log_events::EXPLAINED;
use crate::scan::{self, Running};
use crate::store::{all, one, path_to_bytes};
use std::path::Path;
use timbre_edge_sqlite::json;
use timbre_edge_sqlite::rusqlite::params;
use timbre_kernel::{BoxFuture, Ctx, Fallible, log};

pub(crate) fn scan(ctx: Ctx, args: Vec<String>) -> BoxFuture<Fallible> {
    Box::pin(async move {
        let full = match args.as_slice() {
            [] => false,
            [arg] if arg == "--full" => true,
            _ => return Err(LibraryFailure::Argument.boxed()),
        };
        let settings = Settings::get(&ctx)?;
        let running = Running::begin(crate::state(&ctx)?)?;
        scan::stoppable(&ctx, move |stop, ctx| {
            scan::run(&ctx, &running, &settings, full, &stop)
        })
        .await
    })
}

pub(crate) fn explain(ctx: Ctx, args: Vec<String>) -> BoxFuture<Fallible> {
    Box::pin(async move {
        let [path] = args.as_slice() else {
            return Err(LibraryFailure::Argument.boxed());
        };
        let path = std::path::absolute(path).map_err(crate::failures::io)?;
        let scoped = ctx.clone();
        ctx.blocking(move || describe(&scoped, &path)).await?
    })
}

fn describe(ctx: &Ctx, path: &Path) -> Fallible {
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    shared_db(ctx)?.get(ctx)?.read(|sql| {
        let item = one(sql, "SELECT t.id,t.fingerprint,a.album_key,a.key_source,a.raw_group,a.compilation,t.cover_art,CASE WHEN EXISTS(SELECT 1 FROM library_tracks art WHERE art.art_hash=substr(t.cover_art,-32)) THEN 'embedded' WHEN t.cover_art IS NOT NULL THEN 'sidecar' END,a.id,(SELECT art.path FROM library_folder_art art WHERE art.hash=substr(t.cover_art,-32) ORDER BY art.folder_id LIMIT 1) FROM library_tracks t JOIN library_albums a ON a.id=t.album_id JOIN library_track_paths p ON p.track_id=t.id JOIN library_roots r ON r.id=p.root_id WHERE p.absolute=?1 OR p.absolute=?2 OR t.canonical=?2 ORDER BY r.ordinal,p.relative LIMIT 1", params![path_to_bytes(path), path_to_bytes(&canonical)], |row| Ok(Explanation {
            track: row.get(0)?, fingerprint: row.get(1)?, key: row.get(2)?, key_source: row.get(3)?, group: row.get(4)?, compilation: row.get(5)?, art: row.get(6)?, art_source: row.get(7)?, album: row.get(8)?, art_path: crate::store::optional_path(row, 9)?,
        }))?;
        let track = item.track;
        let mut credits = crate::credits::read(sql, Some(track), None)?;
        credits.extend(crate::credits::read(sql, None, Some(item.album))?);
        let credits: Vec<_> = credits.iter().map(|credit| (&credit.role, &credit.sub_role, &credit.artist.id, &credit.artist.name, &credit.join_phrase)).collect();
        let lyrics = all(sql, "SELECT source,language,synced,path FROM library_lyrics WHERE track_id=?1 ORDER BY position", [track], |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?, row.get::<_, bool>(2)?, crate::store::optional_path(row, 3)?.map(|path| path.to_string_lossy().into_owned()))))?;
        let diagnostics = all(sql, "SELECT kind,detail FROM library_diagnostics WHERE track_id=?1 ORDER BY kind", [track], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))?;
        log!(ctx, info, EXPLAINED,
            path = path.to_string_lossy().as_ref(), track_id = track, fingerprint = item.fingerprint.as_deref().unwrap_or_default(),
            album_key = item.key, key_source = item.key_source, grouping_inputs = item.group,
            compilation_source = item.compilation.as_deref().unwrap_or_default(), credits = json::encode(&credits)?.as_str(),
            art_id = item.art.as_deref().unwrap_or_default(), art_source = item.art_source.as_deref().unwrap_or_default(),
            art_path = item.art_path.as_deref().map(Path::to_string_lossy).as_deref().unwrap_or_default(),
            lyric_sources = json::encode(&lyrics)?.as_str(), diagnostics = json::encode(&diagnostics)?.as_str(),
        );
        Ok(())
    })
}

struct Explanation {
    track: i64,
    album: i64,
    fingerprint: Option<String>,
    key: String,
    key_source: String,
    group: String,
    compilation: Option<String>,
    art: Option<String>,
    art_source: Option<String>,
    art_path: Option<std::path::PathBuf>,
}
