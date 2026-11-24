use crate::config::shared_db;
use crate::failures::LibraryFailure;
use crate::search::hydrate_page;
use crate::store::{Window, all, contained, id, one, row_path, unsigned, window};
use crate::{Library, media, scan, search, views};
use timbre_edge_sqlite::rusqlite::{Connection, Params, params};
use timbre_edge_tags::{Codec, Container};
use timbre_edge_text::fold;
use timbre_kernel::catalog::{
    Album, AlbumDetail, AlbumOrder, Artist, ArtistDetail, Catalog, Genre, Lyrics, MediaSource,
    ScanStatus, SearchResults, SearchWindows, Track,
};
use timbre_kernel::{Ctx, Fallible, Page, PageRequest};

impl Catalog for Library {
    fn artists(&self, ctx: &Ctx, page: PageRequest<'_>) -> Fallible<Page<Artist>> {
        let win = window(page)?;
        shared_db(ctx)?.get(ctx)?.read(|sql| {
            ids(
                sql,
                "SELECT id FROM library_artists ORDER BY order_name,id LIMIT ?1 OFFSET ?2",
                params![win.limit(), win.skip()],
                win,
                views::artist,
            )
        })
    }

    fn artist(&self, ctx: &Ctx, artist: &str, albums: PageRequest<'_>) -> Fallible<ArtistDetail> {
        let win = window(albums)?;
        let artist = id(artist)?;
        shared_db(ctx)?.get(ctx)?.read(|sql| {
            Ok(ArtistDetail {
                artist: views::artist(sql, artist)?,
                albums: ids(
                    sql,
                    views::ARTIST_ALBUMS,
                    params![artist, win.limit(), win.skip()],
                    win,
                    views::album,
                )?,
            })
        })
    }

    fn album(&self, ctx: &Ctx, album: &str, tracks: PageRequest<'_>) -> Fallible<AlbumDetail> {
        let win = window(tracks)?;
        let album = id(album)?;
        shared_db(ctx)?.get(ctx)?.read(|sql| Ok(AlbumDetail {
            album: views::album(sql, album)?,
            tracks: ids(sql, "SELECT id FROM library_tracks WHERE album_id=?1 ORDER BY disc,track_number,id LIMIT ?2 OFFSET ?3", params![album, win.limit(), win.skip()], win, views::track)?,
        }))
    }

    fn track(&self, ctx: &Ctx, track: &str) -> Fallible<Track> {
        let track = id(track)?;
        shared_db(ctx)?
            .get(ctx)?
            .read(|sql| views::track(sql, track))
    }

    fn tracks(&self, ctx: &Ctx, tracks: &[&str], page: PageRequest<'_>) -> Fallible<Page<Track>> {
        let win = window(page)?;
        shared_db(ctx)?.get(ctx)?.read(|sql| {
            let mut matching = Vec::new();
            let mut skip = win.offset;
            for input in tracks {
                let Ok(track) = id(input) else { continue };
                let exists: bool = one(sql, "SELECT EXISTS(SELECT 1 FROM library_tracks WHERE id=?1 AND album_id IS NOT NULL)", [track], |row| row.get(0))?;
                if !exists { continue; }
                if skip != 0 { skip -= 1; continue; }
                matching.push(track);
                if matching.len() > win.count { break; }
            }
            hydrate_page(sql, matching, win, views::track)
        })
    }

    fn albums(
        &self,
        ctx: &Ctx,
        order: AlbumOrder<'_>,
        page: PageRequest<'_>,
    ) -> Fallible<Page<Album>> {
        let win = window(page)?;
        shared_db(ctx)?
            .get(ctx)?
            .read(|sql| browse(sql, &order, win))
    }

    fn genres(&self, ctx: &Ctx, page: PageRequest<'_>) -> Fallible<Page<Genre>> {
        let win = window(page)?;
        shared_db(ctx)?.get(ctx)?.read(|sql| {
            let genres = all(sql, "SELECT g.name,count(DISTINCT t.album_id),count(*) FROM library_track_genres g JOIN library_tracks t ON t.id=g.track_id GROUP BY g.folded_name,g.name ORDER BY g.folded_name,g.name LIMIT ?1 OFFSET ?2", params![win.limit(), win.skip()], |row| Ok(Genre { name: row.get(0)?, album_count: unsigned(row, 1)?, track_count: unsigned(row, 2)? }))?;
            Ok(win.page(genres))
        })
    }

    fn search(&self, ctx: &Ctx, query: &str, windows: SearchWindows) -> Fallible<SearchResults> {
        for window in [windows.artists, windows.albums, windows.tracks] {
            if window.count > timbre_kernel::MAX_PAGE_LIMIT {
                return Err(LibraryFailure::Argument.boxed());
            }
        }
        shared_db(ctx)?
            .get(ctx)?
            .read(|sql| search::find(sql, query, windows))
    }

    fn lyrics(&self, ctx: &Ctx, track: &str, page: PageRequest<'_>) -> Fallible<Page<Lyrics>> {
        let track = id(track)?;
        let win = window(page)?;
        shared_db(ctx)?
            .get(ctx)?
            .read(|sql| media::lyrics(sql, track, views::source(sql, track)?.properties, win))
    }

    fn media_source(&self, ctx: &Ctx, track: &str) -> Fallible<MediaSource> {
        let track = id(track)?;
        shared_db(ctx)?.get(ctx)?.read(|sql| {
            let properties = views::source(sql, track)?.properties;
            let paths = all(sql, "SELECT t.canonical,r.path,p.size FROM library_tracks t JOIN library_track_paths p ON t.id=p.track_id JOIN library_roots r ON r.id=p.root_id WHERE t.id=?1 ORDER BY r.ordinal,p.relative", [track], |row| Ok((row_path(row, 0)?, row_path(row, 1)?, unsigned(row, 2)?)))?;
            for (path, root, size) in paths {
                if let Ok(path) = contained(&path, &root) {
                    return Ok(MediaSource { path, container: container_name(properties.container).to_owned(), codec: codec_name(properties.codec), bitrate_kbps: properties.bitrate_kbps, duration: properties.duration, size });
                }
            }
            Err(LibraryFailure::NotFound.boxed())
        })
    }

    fn scan_status(&self, ctx: &Ctx) -> Fallible<ScanStatus> {
        scan::status(self, ctx)
    }

    fn start_scan(&self, ctx: &Ctx, full: bool) -> Fallible<ScanStatus> {
        if !ctx
            .principal()
            .is_some_and(|actor| actor.has(crate::SCAN_PERMISSION))
        {
            return Err(LibraryFailure::Forbidden.boxed());
        }
        scan::start(ctx, self, full)
    }
}

/// Canonical container name. The tag crate's `Debug` spelling is a Rust
/// detail; clients read these names, so the mapping is written out.
fn container_name(container: Container) -> &'static str {
    match container {
        Container::Mpeg => "mp3",
        Container::Flac => "flac",
        Container::Ogg => "ogg",
        Container::Mp4 => "mp4",
        Container::Wav => "wav",
        Container::Aiff => "aiff",
        Container::WavPack => "wavpack",
        Container::Ape => "ape",
    }
}

/// Canonical codec name, including the numeric WAVE tags and AIFF-C
/// four-character codes that carry no name of their own.
fn codec_name(codec: Codec) -> String {
    match codec {
        Codec::Mp3 => "mp3".to_owned(),
        Codec::Flac => "flac".to_owned(),
        Codec::Vorbis => "vorbis".to_owned(),
        Codec::Opus => "opus".to_owned(),
        Codec::Aac => "aac".to_owned(),
        Codec::Alac => "alac".to_owned(),
        Codec::Pcm => "pcm".to_owned(),
        Codec::WavPack => "wavpack".to_owned(),
        Codec::MonkeyAudio => "ape".to_owned(),
        Codec::Wav(tag) => match tag {
            0x0001 | 0x0003 | 0xfffe => "pcm".to_owned(),
            0x0002 | 0x0011 => "adpcm".to_owned(),
            0x0006 => "alaw".to_owned(),
            0x0007 => "mulaw".to_owned(),
            0x0055 => "mp3".to_owned(),
            0x2000 => "ac3".to_owned(),
            other => format!("wav_{other:04x}"),
        },
        Codec::Aiff(code) => aiff_codec(code),
    }
}

fn aiff_codec(code: [u8; 4]) -> String {
    let name = String::from_utf8_lossy(&code).trim().to_ascii_lowercase();
    match name.as_str() {
        "none" | "sowt" | "twos" | "raw" | "in24" | "in32" | "fl32" | "fl64" => "pcm".to_owned(),
        "alac" => "alac".to_owned(),
        ".mp3" | "mp3" => "mp3".to_owned(),
        "ima4" => "adpcm".to_owned(),
        "ulaw" => "mulaw".to_owned(),
        "alaw" => "alaw".to_owned(),
        other => other.chars().filter(char::is_ascii_alphanumeric).collect(),
    }
}

fn ids<T>(
    sql: &Connection,
    query: &str,
    values: impl Params,
    win: Window,
    read: impl Fn(&Connection, i64) -> Fallible<T>,
) -> Fallible<Page<T>> {
    let matches = all(sql, query, values, |row| row.get(0))?;
    hydrate_page(sql, matches, win, read)
}

fn browse(sql: &Connection, order: &AlbumOrder<'_>, win: Window) -> Fallible<Page<Album>> {
    let page = params![win.limit(), win.skip()];
    let matches = match order {
        AlbumOrder::Newest => all(
            sql,
            "SELECT id FROM library_albums ORDER BY created DESC,id LIMIT ?1 OFFSET ?2",
            page,
            |row| row.get(0),
        ),
        AlbumOrder::Name => all(
            sql,
            "SELECT id FROM library_albums ORDER BY order_name,id LIMIT ?1 OFFSET ?2",
            page,
            |row| row.get(0),
        ),
        AlbumOrder::Artist => all(
            sql,
            "SELECT id FROM library_albums ORDER BY artist_order,order_name,id LIMIT ?1 OFFSET ?2",
            page,
            |row| row.get(0),
        ),
        AlbumOrder::Random => all(
            sql,
            "SELECT a.id FROM library_albums a CROSS JOIN library_state s ORDER BY ((a.id * 1103515245 + s.tree_version * 12345) & 2147483647),a.id LIMIT ?1 OFFSET ?2",
            page,
            |row| row.get(0),
        ),
        AlbumOrder::Genre(name) => all(
            sql,
            "SELECT a.id FROM library_albums a WHERE EXISTS(SELECT 1 FROM library_tracks t JOIN library_track_genres g ON g.track_id=t.id WHERE t.album_id=a.id AND g.folded_name=?1) ORDER BY a.order_name,a.id LIMIT ?2 OFFSET ?3",
            params![fold(name), win.limit(), win.skip()],
            |row| row.get(0),
        ),
        AlbumOrder::Year { from, to } => {
            let order = if from <= to { "ASC" } else { "DESC" };
            let query = format!(
                "SELECT id FROM library_albums WHERE year BETWEEN ?1 AND ?2 ORDER BY year {order},id LIMIT ?3 OFFSET ?4"
            );
            all(
                sql,
                &query,
                params![from.min(to), from.max(to), win.limit(), win.skip()],
                |row| row.get(0),
            )
        }
    }?;
    hydrate_page(sql, matches, win, views::album)
}
