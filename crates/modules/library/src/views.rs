use crate::albums::{self, Metadata};
use crate::scan::Source;
use crate::store::{all, integer, one, unsigned};
use std::time::Duration;
use timbre_edge_sqlite::json;
use timbre_edge_sqlite::rusqlite::{Connection, params};
use timbre_kernel::Fallible;
use timbre_kernel::catalog::{
    Album, Artist, CompilationSource, DiscTitle, Movement, ReplayGain, Track,
};
use timbre_kernel::clock::from_unix_millis;

pub(crate) fn artist(sql: &Connection, id: i64) -> Fallible<Artist> {
    let (name, sort_name, order_name, musicbrainz_id, cover_art) = one(
        sql,
        "SELECT name,sort_name,order_name,mbid,cover_art FROM library_artists WHERE id=?1",
        [id],
        |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ))
        },
    )?;
    let roles = all(
        sql,
        "SELECT DISTINCT role FROM library_credits WHERE artist_id=?1 ORDER BY role",
        [id],
        |row| row.get(0),
    )?;
    let album_count = one(
        sql,
        "SELECT count(DISTINCT coalesce(c.album_id,t.album_id)) FROM library_credits c LEFT JOIN library_tracks t ON t.id=c.track_id WHERE c.artist_id=?1",
        [id],
        |row| unsigned(row, 0),
    )?;
    Ok(Artist {
        id: id.to_string(),
        name,
        sort_name,
        order_name,
        musicbrainz_id,
        roles,
        album_count,
        cover_art,
    })
}

pub(crate) const ARTIST_ALBUMS: &str = "SELECT a.id FROM library_albums a WHERE EXISTS(SELECT 1 FROM library_credits c LEFT JOIN library_tracks t ON t.id=c.track_id WHERE c.artist_id=?1 AND coalesce(c.album_id,t.album_id)=a.id) ORDER BY a.order_name,a.id LIMIT ?2 OFFSET ?3";

pub(crate) fn album(sql: &Connection, id: i64) -> Fallible<Album> {
    let row = one(
        sql,
        "SELECT name,order_name,display_artist,release_id,release_group_id,version,compilation,year,mature_metadata,cover_art,song_count,duration_ms,created FROM library_albums WHERE id=?1",
        [id],
        |row| {
            Ok(AlbumRow {
                name: row.get(0)?,
                order: row.get(1)?,
                artist: row.get(2)?,
                release: row.get(3)?,
                group: row.get(4)?,
                version: row.get(5)?,
                compilation: row.get(6)?,
                year: row.get(7)?,
                metadata: row.get(8)?,
                art: row.get(9)?,
                count: unsigned(row, 10)?,
                duration: unsigned(row, 11)?,
                created: unsigned(row, 12)?,
            })
        },
    )?;
    let metadata: Metadata = json::decode(&row.metadata)?;
    Ok(Album {
        id: id.to_string(),
        name: row.name,
        sort_name: metadata.sort_name,
        order_name: row.order,
        display_artist: row.artist,
        credits: crate::credits::read(sql, None, Some(id))?,
        musicbrainz_id: row.release,
        release_group_id: row.group,
        version: row.version,
        release_types: metadata.release_types,
        compilation: match row.compilation.as_deref() {
            Some("tag") => Some(CompilationSource::Tag),
            Some("derived") => Some(CompilationSource::Derived),
            _ => None,
        },
        year: row.year,
        original_release_date: metadata
            .original_date
            .as_deref()
            .and_then(albums::item_date),
        release_date: metadata.release_date.as_deref().and_then(albums::item_date),
        disc_titles: metadata
            .discs
            .into_iter()
            .map(|(disc, (title, cover_art))| DiscTitle {
                disc,
                title,
                cover_art,
            })
            .collect(),
        record_labels: metadata.labels,
        moods: metadata.moods,
        genres: metadata.genres,
        explicit_status: albums::advisory(metadata.advisory),
        cover_art: row.art,
        song_count: row.count,
        duration: Duration::from_millis(row.duration),
        created: from_unix_millis(integer(row.created)),
    })
}

struct AlbumRow {
    name: String,
    order: String,
    artist: String,
    release: Option<String>,
    group: Option<String>,
    version: Option<String>,
    compilation: Option<String>,
    year: Option<u32>,
    metadata: String,
    art: Option<String>,
    count: u64,
    duration: u64,
    created: u64,
}

pub(crate) fn track(sql: &Connection, id: i64) -> Fallible<Track> {
    let row = one(
        sql,
        "SELECT t.title,t.track_number,t.disc,t.album_id,a.name,a.display_artist,t.source,t.cover_art,t.size,t.created FROM library_tracks t JOIN library_albums a ON a.id=t.album_id WHERE t.id=?1",
        [id],
        |row| {
            Ok(TrackRow {
                title: row.get(0)?,
                number: row.get(1)?,
                disc: row.get(2)?,
                album_id: row.get(3)?,
                album: row.get(4)?,
                album_artist: row.get(5)?,
                source: row.get(6)?,
                art: row.get(7)?,
                size: unsigned(row, 8)?,
                created: unsigned(row, 9)?,
            })
        },
    )?;
    let source: Source = json::decode(&row.source)?;
    let fields = source.fields;
    let properties = source.properties;
    let movement = (fields.movement.is_some()
        || fields.movement_number.is_some()
        || fields.movement_total.is_some())
    .then_some(Movement {
        name: fields.movement,
        number: fields.movement_number,
        total: fields.movement_total,
    });
    let mut credits = crate::credits::read(sql, Some(id), None)?;
    if !credits.iter().any(|credit| credit.role == "albumartist") {
        credits.extend(crate::credits::read(sql, None, Some(row.album_id))?);
    }
    let raw = fields.replay_gain;
    let convert = |gain| f64::from(gain) / 256.0 + 5.0;
    let replay_gain = ReplayGain {
        track_gain: raw.track_gain.or_else(|| raw.r128_track_gain.map(convert)),
        track_peak: raw.track_peak,
        album_gain: raw.album_gain.or_else(|| raw.r128_album_gain.map(convert)),
        album_peak: raw.album_peak,
    };
    Ok(Track {
        id: id.to_string(),
        title: row.title,
        sort_name: fields.title_sort,
        album_id: row.album_id.to_string(),
        album: row.album,
        display_artist: crate::credits::display(fields.artist.as_ref(), &fields.artists),
        display_album_artist: row.album_artist,
        display_composer: crate::credits::display(fields.composer.as_ref(), &[]),
        credits,
        track_number: row.number,
        disc_number: row.disc,
        year: fields
            .date
            .as_deref()
            .and_then(albums::item_date)
            .map(|date| date.year),
        genres: crate::credits::values(fields.genre.as_ref()),
        musicbrainz_id: fields.musicbrainz_recording_id,
        isrc: fields.isrc,
        moods: crate::credits::values(fields.mood.as_ref()),
        bpm: fields.bpm,
        comment: fields.comment,
        explicit_status: albums::advisory(fields.advisory),
        media_type: Some("song".to_owned()),
        movements: movement.into_iter().collect(),
        groupings: crate::credits::values(fields.grouping.as_ref()),
        works: fields.work.into_iter().collect(),
        show_movement: fields.show_movement,
        replay_gain,
        cover_art: row.art,
        duration: properties.duration,
        size: row.size,
        bitrate_kbps: properties.bitrate_kbps,
        sample_rate: properties.sample_rate,
        channels: properties.channels,
        bit_depth: properties.bit_depth,
        suffix: source.suffix,
        created: from_unix_millis(integer(row.created)),
    })
}

struct TrackRow {
    title: String,
    number: Option<u32>,
    disc: Option<u32>,
    album_id: i64,
    album: String,
    album_artist: String,
    source: String,
    art: Option<String>,
    size: u64,
    created: u64,
}

pub(crate) fn source(sql: &Connection, id: i64) -> Fallible<Source> {
    let encoded: String = one(
        sql,
        "SELECT source FROM library_tracks WHERE id=?1",
        params![id],
        |row| row.get(0),
    )?;
    json::decode(&encoded)
}
