use crate::store::{Window, all, exec};
use timbre_edge_sqlite::rusqlite::{Connection, params};
use timbre_edge_text::{contains_cjk, fold};
use timbre_kernel::catalog::{SearchResults, SearchWindow, SearchWindows};
use timbre_kernel::{Fallible, Page};

/// One searchable kind. The row id of a search entry is derived from the
/// entity id and this code, so one entity's row can be replaced or dropped
/// without scanning the whole index.
#[derive(Clone, Copy)]
pub(crate) enum Entity {
    Artist,
    Album,
    Track,
}

impl Entity {
    const fn code(self) -> i64 {
        match self {
            Self::Artist => 0,
            Self::Album => 1,
            Self::Track => 2,
        }
    }

    /// The one row of this entity, in the index's column order.
    const fn select(self) -> &'static str {
        match self {
            Self::Artist => {
                "SELECT ?1,'artist',id,folded_name,'','' FROM library_artists WHERE id=?2"
            }
            Self::Album => {
                "SELECT ?1,'album',id,folded_name,folded_artist,'' FROM library_albums WHERE id=?2"
            }
            Self::Track => {
                "SELECT ?1,'track',t.id,t.folded_name,t.folded_artist,a.folded_name FROM library_tracks t JOIN library_albums a ON a.id=t.album_id WHERE t.id=?2"
            }
        }
    }

    fn row_id(self, entity: i64) -> i64 {
        entity.saturating_mul(KINDS).saturating_add(self.code())
    }

    /// Every row of this kind, in the index's column order. The row id is
    /// built from the same constants as [`Entity::row_id`].
    fn bulk(self) -> String {
        let (kinds, code) = (KINDS, self.code());
        match self {
            Self::Artist => format!(
                "SELECT (id*{kinds}+{code}),'artist',id,folded_name,'','' FROM library_artists"
            ),
            Self::Album => format!(
                "SELECT (id*{kinds}+{code}),'album',id,folded_name,folded_artist,'' FROM library_albums"
            ),
            Self::Track => format!(
                "SELECT (t.id*{kinds}+{code}),'track',t.id,t.folded_name,t.folded_artist,a.folded_name FROM library_tracks t JOIN library_albums a ON a.id=t.album_id"
            ),
        }
    }
}

/// How many kinds share the row id space; see [`Entity::row_id`].
const KINDS: i64 = 4;

const TABLES: [&str; 2] = ["library_fts", "library_trigram"];

/// Rebuild both indexes from scratch; a full scan touches every row anyway.
pub(crate) fn index(sql: &Connection) -> Fallible {
    let select = [Entity::Artist, Entity::Album, Entity::Track]
        .map(Entity::bulk)
        .join(" UNION ALL ");
    for table in TABLES {
        exec(sql, &format!("DELETE FROM {table}"), [])?;
        exec(
            sql,
            &format!("INSERT INTO {table}(rowid,kind,entity,name,artist,album) {select}"),
            [],
        )?;
    }
    Ok(())
}

/// Replace one entity's index rows with its current contents.
pub(crate) fn refresh(sql: &Connection, entity: Entity, id: i64) -> Fallible {
    let row = entity.row_id(id);
    for table in TABLES {
        exec(sql, &format!("DELETE FROM {table} WHERE rowid=?1"), [row])?;
        exec(
            sql,
            &format!(
                "INSERT INTO {table}(rowid,kind,entity,name,artist,album) {}",
                entity.select()
            ),
            params![row, id],
        )?;
    }
    Ok(())
}

/// Drop the index rows of entities that no longer exist.
pub(crate) fn forget(sql: &Connection, entity: Entity, ids: &[i64]) -> Fallible {
    for &id in ids {
        let row = entity.row_id(id);
        for table in TABLES {
            exec(sql, &format!("DELETE FROM {table} WHERE rowid=?1"), [row])?;
        }
    }
    Ok(())
}

pub(crate) fn find(
    sql: &Connection,
    input: &str,
    counts: SearchWindows,
) -> Fallible<SearchResults> {
    let folded = fold(input);
    let pattern = if input.trim().is_empty() {
        Pattern::All
    } else if folded.is_empty() {
        Pattern::None
    } else if contains_cjk(input) {
        if input.chars().filter(|ch| !ch.is_whitespace()).count() >= 3
            && folded.chars().count() >= 3
        {
            Pattern::Trigram(quoted(&folded))
        } else {
            Pattern::Substring(folded)
        }
    } else {
        Pattern::Prefix(format!("{}*", quoted(&folded)))
    };
    Ok(SearchResults {
        artists: matching(
            sql,
            &pattern,
            "artist",
            counts.artists,
            crate::views::artist,
        )?,
        albums: matching(sql, &pattern, "album", counts.albums, crate::views::album)?,
        tracks: matching(sql, &pattern, "track", counts.tracks, crate::views::track)?,
    })
}

fn quoted(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

enum Pattern {
    All,
    None,
    Prefix(String),
    Trigram(String),
    Substring(String),
}

fn matching<T>(
    sql: &Connection,
    pattern: &Pattern,
    kind: &str,
    page: SearchWindow,
    hydrate: impl Fn(&Connection, i64) -> Fallible<T>,
) -> Fallible<Page<T>> {
    let window = Window {
        count: page.count,
        offset: page.offset,
    };
    if page.count == 0 || matches!(pattern, Pattern::None) {
        return Ok(window.page(Vec::new()));
    }
    let table = match kind {
        "artist" => "library_artists",
        "album" => "library_albums",
        _ => "library_tracks",
    };
    let exact = match pattern {
        Pattern::All | Pattern::None => all(
            sql,
            &format!("SELECT id FROM {table} ORDER BY order_name,id LIMIT ?1 OFFSET ?2"),
            params![window.limit(), window.skip()],
            |row| row.get(0),
        )?,
        Pattern::Prefix(query) => fts(sql, table, "library_fts", kind, query, window)?,
        Pattern::Trigram(query) => fts(sql, table, "library_trigram", kind, query, window)?,
        Pattern::Substring(query) => {
            let statement = format!(
                "SELECT t.id FROM library_fts f JOIN {table} t ON t.id=f.entity WHERE f.kind=?1 AND (instr(f.name,?2)>0 OR instr(f.artist,?2)>0 OR instr(f.album,?2)>0) ORDER BY t.order_name,t.id LIMIT ?3 OFFSET ?4"
            );
            all(
                sql,
                &statement,
                params![kind, query, window.limit(), window.skip()],
                |row| row.get(0),
            )?
        }
    };
    hydrate_page(sql, exact, window, hydrate)
}

fn fts(
    sql: &Connection,
    table: &str,
    index: &str,
    kind: &str,
    query: &str,
    window: Window,
) -> Fallible<Vec<i64>> {
    let statement = format!(
        "SELECT t.id FROM {index} f JOIN {table} t ON t.id=f.entity WHERE {index} MATCH ?1 AND f.kind=?2 ORDER BY t.order_name,t.id LIMIT ?3 OFFSET ?4"
    );
    all(
        sql,
        &statement,
        params![query, kind, window.limit(), window.skip()],
        |row| row.get(0),
    )
}

pub(crate) fn hydrate_page<T>(
    sql: &Connection,
    mut ids: Vec<i64>,
    window: Window,
    hydrate: impl Fn(&Connection, i64) -> Fallible<T>,
) -> Fallible<Page<T>> {
    let extra = ids.len() > window.count;
    ids.truncate(window.count);
    let mut page = window.page(
        ids.into_iter()
            .map(|id| hydrate(sql, id))
            .collect::<Fallible<Vec<_>>>()?,
    );
    if extra && window.count != 0 {
        page.next = Some(timbre_kernel::Cursor::new(
            window.offset.saturating_add(window.count).to_string(),
        ));
    }
    Ok(page)
}
