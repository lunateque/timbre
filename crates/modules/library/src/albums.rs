use crate::config::Settings;
use crate::group::{self, AlbumGroup};
use crate::store::{all, exec, one, unix_millis};
use crate::{credits, group::Song};
use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, SystemTime};
use timbre_edge_config::serde;
use timbre_edge_sqlite::rusqlite::{Connection, params};
use timbre_edge_sqlite::{Database, json};
use timbre_edge_tags::{Advisory, Fields};
use timbre_edge_text::{fold, order_name};
use timbre_kernel::Fallible;
use timbre_kernel::catalog::{ExplicitStatus, ItemDate};

#[derive(serde::Deserialize, serde::Serialize)]
#[serde(crate = "timbre_edge_config::serde")]
pub(crate) struct Metadata {
    pub(crate) sort_name: Option<String>,
    pub(crate) release_types: Vec<String>,
    pub(crate) original_date: Option<String>,
    pub(crate) release_date: Option<String>,
    pub(crate) discs: BTreeMap<u32, (String, Option<String>)>,
    pub(crate) labels: Vec<String>,
    pub(crate) moods: Vec<String>,
    pub(crate) genres: Vec<String>,
    pub(crate) advisory: Option<Advisory>,
}

/// What one pass has to regroup: the albums holding tracks of these
/// folders, plus albums that lost tracks. No scope means the whole catalog.
pub(crate) struct Scope {
    pub(crate) folders: BTreeSet<i64>,
    pub(crate) albums: BTreeSet<i64>,
}

/// How many albums share one write transaction.
const BATCH: usize = 64;

pub(crate) fn regroup(
    db: &Database,
    settings: &Settings,
    scope: Option<&Scope>,
    stop: &impl Fn() -> bool,
) -> Fallible<bool> {
    let groups = group::group(db.read(group::songs)?);
    let keys = match scope {
        Some(scope) => db.read(|sql| album_keys(sql, &scope.albums))?,
        None => BTreeSet::new(),
    };
    let selected: Vec<_> = groups
        .into_iter()
        .filter(|(key, group)| scope.is_none_or(|scope| touches(scope, &keys, key, group)))
        .collect();
    let incremental = scope.is_some();
    for batch in selected.chunks(BATCH) {
        if stop() {
            return Ok(false);
        }
        db.write(|sql| {
            for (key, group) in batch {
                write(sql, key, group, settings, incremental)?;
            }
            Ok(())
        })?;
    }
    if stop() {
        return Ok(false);
    }
    db.write(|sql| {
        let albums = all(sql, "DELETE FROM library_albums WHERE NOT EXISTS(SELECT 1 FROM library_tracks WHERE album_id=library_albums.id) RETURNING id", [], |row| row.get(0))?;
        crate::search::forget(sql, crate::search::Entity::Album, &albums)?;
        let artists = all(sql, "DELETE FROM library_artists WHERE NOT EXISTS(SELECT 1 FROM library_credits WHERE artist_id=library_artists.id) RETURNING id", [], |row| row.get(0))?;
        crate::search::forget(sql, crate::search::Entity::Artist, &artists)?;
        exec(sql, "DELETE FROM library_art WHERE NOT EXISTS(SELECT 1 FROM library_folder_art WHERE hash=library_art.hash) AND NOT EXISTS(SELECT 1 FROM library_tracks WHERE art_hash=library_art.hash)", [])?;
        if incremental { Ok(()) } else { crate::search::index(sql) }
    })?;
    Ok(true)
}

/// The keys of albums that lost tracks: their groups must be written even
/// when no song of theirs sits in a changed folder.
fn album_keys(sql: &Connection, albums: &BTreeSet<i64>) -> Fallible<BTreeSet<String>> {
    albums
        .iter()
        .map(|&album| {
            one(
                sql,
                "SELECT album_key FROM library_albums WHERE id=?1",
                [album],
                |row| row.get(0),
            )
        })
        .collect()
}

fn touches(scope: &Scope, keys: &BTreeSet<String>, key: &str, group: &AlbumGroup) -> bool {
    keys.contains(key)
        || group.songs.iter().any(|(song, _)| {
            scope.folders.contains(&song.folder) || scope.folders.contains(&song.parent)
        })
}

fn write(
    sql: &Connection,
    key: &str,
    group: &AlbumGroup,
    settings: &Settings,
    incremental: bool,
) -> Fallible {
    let first = group
        .songs
        .iter()
        .find(|(_, overridden)| overridden.is_none())
        .or_else(|| group.songs.first());
    let Some((first, _)) = first else {
        return Ok(());
    };
    let compilation = group.compilation();
    let fields = &first.source.fields;
    let artist_fields = group
        .songs
        .iter()
        .map(|(song, _)| &song.source.fields)
        .find(|fields| credits::has_album_artist(fields))
        .unwrap_or(fields);
    let album_credits = credits::album(artist_fields, settings, compilation.is_some());
    let display_artist = album_artist(artist_fields, compilation.is_some());
    let artist_sort = credits::display(artist_fields.album_artist_sort.as_ref(), &[]);
    let artist_order = order_name(if artist_sort.is_empty() {
        &display_artist
    } else {
        &artist_sort
    });
    let name = display_name(
        &group.raw.title,
        fields.album_version.as_deref(),
        settings.append_version_to_title,
    );
    let metadata = aggregate(group, fields, compilation.is_some());
    let order = order_name(metadata.sort_name.as_deref().unwrap_or(&name));
    let timestamp = unix_millis(SystemTime::now());
    let duration = group.songs.iter().fold(Duration::ZERO, |sum, (song, _)| {
        sum.saturating_add(song.source.properties.duration)
    });
    let millis = i64::try_from(duration.as_millis()).unwrap_or(i64::MAX);
    let year = metadata
        .release_date
        .as_deref()
        .and_then(item_date)
        .map(|date| date.year);
    let count = i64::try_from(group.songs.len()).unwrap_or(i64::MAX);
    let album: i64 = one(
        sql,
        "INSERT INTO library_albums(album_key,key_source,raw_group,name,order_name,artist_order,display_artist,release_id,release_group_id,version,compilation,year,mature_metadata,song_count,duration_ms,created,folded_name,folded_artist) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18) ON CONFLICT(album_key) DO UPDATE SET key_source=excluded.key_source,raw_group=excluded.raw_group,name=excluded.name,order_name=excluded.order_name,artist_order=excluded.artist_order,display_artist=excluded.display_artist,release_id=excluded.release_id,release_group_id=excluded.release_group_id,version=excluded.version,compilation=excluded.compilation,year=excluded.year,mature_metadata=excluded.mature_metadata,song_count=excluded.song_count,duration_ms=excluded.duration_ms,folded_name=excluded.folded_name,folded_artist=excluded.folded_artist RETURNING id",
        params![
            key,
            group.raw.source(),
            json::encode(&group.raw)?,
            name,
            order,
            artist_order,
            display_artist,
            fields.musicbrainz_album_id,
            fields.musicbrainz_release_group_id,
            fields.album_version,
            compilation,
            year,
            json::encode(&metadata)?,
            count,
            millis,
            timestamp,
            fold(&name),
            fold(&display_artist)
        ],
        |row| row.get(0),
    )?;
    let art_id = write_cover(sql, album, group)?;
    exec(
        sql,
        "DELETE FROM library_credits WHERE album_id=?1",
        [album],
    )?;
    credits::write(sql, None, Some(album), &album_credits)?;
    for (song, overridden) in &group.songs {
        update_song(
            sql,
            song,
            album,
            art_id.as_deref(),
            overridden.as_ref(),
            settings,
        )?;
    }
    if incremental {
        reindex(sql, album, group)?;
    }
    Ok(())
}

/// Refresh the search rows this album owns. A full pass rebuilds the whole
/// index instead, so only an incremental pass pays per entity.
fn reindex(sql: &Connection, album: i64, group: &AlbumGroup) -> Fallible {
    use crate::search::{Entity, refresh};
    for (song, _) in &group.songs {
        refresh(sql, Entity::Track, song.id)?;
    }
    refresh(sql, Entity::Album, album)?;
    let artists = all(
        sql,
        "SELECT DISTINCT c.artist_id FROM library_credits c LEFT JOIN library_tracks t ON t.id=c.track_id WHERE c.album_id=?1 OR t.album_id=?1",
        [album],
        |row| row.get(0),
    )?;
    for artist in artists {
        refresh(sql, Entity::Artist, artist)?;
    }
    Ok(())
}

fn write_cover(sql: &Connection, album: i64, group: &AlbumGroup) -> Fallible<Option<String>> {
    use timbre_edge_sqlite::rusqlite::OptionalExtension;
    let embedded = group
        .songs
        .iter()
        .filter_map(|(song, _)| Some((!song.art_front, song.art.as_deref()?)))
        .min_by_key(|(not_front, _)| *not_front);
    let art_id = if let Some((_, hash)) = embedded {
        Some(format!("al-{album}-{hash}"))
    } else {
        sql.query_row(
            "SELECT hash FROM library_folder_art WHERE folder_id=?1",
            [group.raw.folder],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(timbre_edge_sqlite::failures::query_error)?
        .map(|hash| format!("al-{album}-{hash}"))
    };
    exec(
        sql,
        "UPDATE library_albums SET cover_art=?2 WHERE id=?1",
        params![album, art_id],
    )?;
    Ok(art_id)
}

fn update_song(
    sql: &Connection,
    song: &Song,
    album: i64,
    art: Option<&str>,
    override_key: Option<&group::Raw>,
    settings: &Settings,
) -> Fallible {
    exec(
        sql,
        "UPDATE library_tracks SET album_id=?1,disc=?2,cover_art=?3 WHERE id=?4",
        params![album, song.disc(), art, song.id],
    )?;
    exec(
        sql,
        "DELETE FROM library_diagnostics WHERE track_id=?1 AND kind='overridden_key'",
        [song.id],
    )?;
    if let Some(lost) = override_key {
        exec(
            sql,
            "INSERT INTO library_diagnostics(track_id,kind,detail) VALUES(?1,'overridden_key',?2)",
            params![song.id, json::encode(lost)?],
        )?;
    }
    exec(
        sql,
        "DELETE FROM library_credits WHERE track_id=?1",
        [song.id],
    )?;
    let credits = credits::tracks(&song.source.fields, settings);
    crate::credits::write(sql, Some(song.id), None, &credits)?;
    exec(
        sql,
        "DELETE FROM library_track_genres WHERE track_id=?1",
        [song.id],
    )?;
    for genre in credits::values(song.source.fields.genre.as_ref()) {
        exec(
            sql,
            "INSERT OR IGNORE INTO library_track_genres(track_id,name,folded_name) VALUES(?1,?2,?3)",
            params![song.id, genre, fold(&genre)],
        )?;
    }
    Ok(())
}

fn aggregate(group: &AlbumGroup, representative: &Fields, compilation: bool) -> Metadata {
    let mut data = Metadata {
        sort_name: representative.album_sort.clone(),
        release_types: Vec::new(),
        original_date: representative
            .original_date
            .clone()
            .or_else(|| representative.original_year.map(|year| year.to_string())),
        release_date: representative.date.clone(),
        discs: BTreeMap::new(),
        labels: Vec::new(),
        moods: Vec::new(),
        genres: Vec::new(),
        advisory: None,
    };
    for (song, _) in &group.songs {
        let fields = &song.source.fields;
        if let Some(title) = &fields.disc_subtitle {
            data.discs
                .entry(song.disc().unwrap_or(1))
                .or_insert_with(|| (title.clone(), None));
        }
        extend_unique(
            &mut data.release_types,
            fields.release_types.iter().cloned(),
        );
        extend_unique(&mut data.labels, credits::values(fields.label.as_ref()));
        extend_unique(&mut data.genres, credits::values(fields.genre.as_ref()));
        extend_unique(&mut data.moods, credits::values(fields.mood.as_ref()));
        if data.advisory.is_none() || fields.advisory == Some(Advisory::Explicit) {
            data.advisory = fields.advisory;
        }
    }
    if compilation {
        extend_unique(
            &mut data.release_types,
            std::iter::once("compilation".to_owned()),
        );
    }
    data
}

fn extend_unique(list: &mut Vec<String>, values: impl IntoIterator<Item = String>) {
    for value in values {
        if !list.iter().any(|old| value.eq_ignore_ascii_case(old)) {
            list.push(value);
        }
    }
}

fn display_name(title: &str, version: Option<&str>, append: bool) -> String {
    if let Some(version) = version.filter(|version| append && !version.is_empty()) {
        format!("{title} ({version})")
    } else {
        title.to_owned()
    }
}

pub(crate) fn album_artist(fields: &Fields, compilation: bool) -> String {
    if credits::has_album_artist(fields) {
        credits::display(fields.album_artist.as_ref(), &fields.album_artists)
    } else if compilation {
        "Various Artists".to_owned()
    } else {
        credits::display(fields.artist.as_ref(), &fields.artists)
    }
}

pub(crate) fn item_date(tag: &str) -> Option<ItemDate> {
    let mut values = tag.split(['-', '/']);
    let year = values.next()?.trim().parse().ok()?;
    let month = values
        .next()
        .and_then(|part| part.parse::<u8>().ok())
        .filter(|month| (1..=12).contains(month));
    let day = values
        .next()
        .and_then(|part| part.get(..2)?.parse::<u8>().ok())
        .filter(|day| month.is_some() && (1..=31).contains(day));
    Some(ItemDate { year, month, day })
}

pub(crate) fn advisory(value: Option<Advisory>) -> Option<ExplicitStatus> {
    value.and_then(|value| match value {
        Advisory::None => Some(ExplicitStatus::None),
        Advisory::Explicit => Some(ExplicitStatus::Explicit),
        Advisory::Clean => Some(ExplicitStatus::Clean),
        Advisory::Other(_) => None,
    })
}
