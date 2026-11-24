use crate::config::Settings;
use crate::store::{all, one};
use timbre_edge_sqlite::rusqlite::{Connection, params};
use timbre_edge_tags::{Fields, TextValues};
use timbre_edge_text::{fold, order_name};
use timbre_kernel::Fallible;
use timbre_kernel::catalog::{ArtistRef, Credit};

pub(crate) struct Named {
    pub(crate) name: String,
    id: Option<String>,
    sort: Option<String>,
    role: &'static str,
    sub_role: Option<String>,
    join: String,
}

pub(crate) fn display(raw: Option<&TextValues>, explicit: &[String]) -> String {
    match raw {
        Some(TextValues::Raw(value)) => value.clone(),
        Some(TextValues::List(values)) => values.join(", "),
        None => explicit.join(", "),
    }
}

pub(crate) fn values(raw: Option<&TextValues>) -> Vec<String> {
    match raw {
        Some(TextValues::Raw(value)) => vec![value.clone()],
        Some(TextValues::List(values)) => values.clone(),
        None => Vec::new(),
    }
}

/// Whether the tags name an album artist at all. A blank or whitespace-only
/// tag counts as absent everywhere, so the displayed album artist and the
/// linked credit cannot disagree about Various Artists.
pub(crate) fn has_album_artist(fields: &Fields) -> bool {
    !display(fields.album_artist.as_ref(), &fields.album_artists)
        .trim()
        .is_empty()
}

pub(crate) fn tracks(fields: &Fields, settings: &Settings) -> Vec<Named> {
    let mut out = named(
        "artist",
        fields.artist.as_ref(),
        &fields.artists,
        &fields.musicbrainz_artist_ids,
        fields.artist_sort.as_ref(),
        settings,
    );
    out.extend(named(
        "albumartist",
        fields.album_artist.as_ref(),
        &fields.album_artists,
        &fields.musicbrainz_album_artist_ids,
        fields.album_artist_sort.as_ref(),
        settings,
    ));
    for (role, raw, sort) in [
        ("composer", &fields.composer, fields.composer_sort.as_ref()),
        ("conductor", &fields.conductor, None),
        ("lyricist", &fields.lyricist, None),
        ("writer", &fields.writer, None),
        ("remixer", &fields.remixer, None),
        ("arranger", &fields.arranger, None),
        ("engineer", &fields.engineer, None),
        ("mixer", &fields.mixer, None),
        ("djmixer", &fields.dj_mixer, None),
    ] {
        out.extend(named(role, raw.as_ref(), &[], &[], sort, settings));
    }
    out.extend(fields.performers.iter().map(|credit| Named {
        name: credit.name.clone(),
        id: None,
        sort: None,
        role: "performer",
        sub_role: credit.instrument.clone(),
        join: String::new(),
    }));
    out
}

pub(crate) fn album(fields: &Fields, settings: &Settings, compilation: bool) -> Vec<Named> {
    if has_album_artist(fields) {
        return named(
            "albumartist",
            fields.album_artist.as_ref(),
            &fields.album_artists,
            &fields.musicbrainz_album_artist_ids,
            fields.album_artist_sort.as_ref(),
            settings,
        );
    }
    if compilation {
        return vec![Named {
            name: "Various Artists".to_owned(),
            id: Some("89ad4ac3-39f7-470e-963a-56509c546377".to_owned()),
            sort: None,
            role: "albumartist",
            sub_role: None,
            join: String::new(),
        }];
    }
    named(
        "albumartist",
        fields.artist.as_ref(),
        &fields.artists,
        &fields.musicbrainz_artist_ids,
        fields.artist_sort.as_ref(),
        settings,
    )
}

fn named(
    role: &'static str,
    raw: Option<&TextValues>,
    explicit: &[String],
    ids: &[String],
    sorts: Option<&TextValues>,
    settings: &Settings,
) -> Vec<Named> {
    let sorts = values(sorts);
    let mut names: Vec<_> = match (raw, explicit, ids) {
        (_, [_, ..], _) => explicit.iter().map(|name| (name.as_str(), ", ")).collect(),
        (Some(TextValues::List(names)), _, _) => {
            names.iter().map(|name| (name.as_str(), ", ")).collect()
        }
        (Some(TextValues::Raw(name)), _, []) => split(name, role, settings),
        (Some(TextValues::Raw(name)), _, _) => vec![(name.as_str(), "")],
        _ => Vec::new(),
    };
    if let Some(last) = names.last_mut() {
        last.1 = "";
    }
    names
        .into_iter()
        .enumerate()
        .filter_map(|(position, (name, join))| {
            let name = name.trim();
            if name.is_empty() {
                return None;
            }
            Some(Named {
                name: name.to_owned(),
                id: ids
                    .get(position)
                    .filter(|id| !id.trim().is_empty())
                    .map(|id| id.trim().to_ascii_lowercase()),
                sort: sorts.get(position).cloned(),
                role,
                sub_role: None,
                join: join.to_owned(),
            })
        })
        .collect()
}

fn split<'a>(name: &'a str, role: &str, settings: &Settings) -> Vec<(&'a str, &'a str)> {
    let lower = name.to_ascii_lowercase();
    let custom = settings.tags.get(role);
    let separators: Vec<String> = custom.map_or_else(
        || vec![" feat. ".to_owned(), " ft. ".to_owned()],
        |tags| tags.iter().map(|tag| tag.to_ascii_lowercase()).collect(),
    );
    let mut out = Vec::new();
    let mut start = 0;
    while let Some(rest) = lower.get(start..) {
        let Some((at, len)) = separators
            .iter()
            .filter(|sep| !sep.is_empty())
            .filter_map(|sep| rest.find(sep).map(|at| (at, sep.len())))
            .min()
        else {
            break;
        };
        let end = start + at;
        let next = end + len;
        if let (Some(value), Some(join)) = (name.get(start..end), name.get(end..next)) {
            out.push((value, join));
        }
        start = next;
    }
    if let Some(rest) = name.get(start..) {
        out.push((rest, ""));
    }
    out
}

pub(crate) fn write(
    sql: &Connection,
    track: Option<i64>,
    album: Option<i64>,
    credits: &[Named],
) -> Fallible {
    for (position, credit) in credits.iter().enumerate() {
        let normalized = fold(&credit.name);
        let key = credit
            .id
            .as_ref()
            .map_or_else(|| format!("name:{normalized}"), |id| format!("mb:{id}"));
        let order = order_name(credit.sort.as_deref().unwrap_or(&credit.name));
        let artist: i64 = one(
            sql,
            "INSERT INTO library_artists(artist_key,mbid,name,sort_name,order_name,folded_name) VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(artist_key) DO UPDATE SET sort_name=coalesce(excluded.sort_name,library_artists.sort_name),order_name=CASE WHEN excluded.sort_name IS NULL THEN library_artists.order_name ELSE excluded.order_name END RETURNING id",
            params![key, credit.id, credit.name, credit.sort, order, normalized],
            |row| row.get(0),
        )?;
        let position = i64::try_from(position).unwrap_or(i64::MAX);
        crate::store::exec(
            sql,
            "INSERT INTO library_credits(track_id,album_id,position,artist_id,credit_name,role,sub_role,join_phrase) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
            params![
                track,
                album,
                position,
                artist,
                credit.name,
                credit.role,
                credit.sub_role,
                credit.join
            ],
        )?;
    }
    Ok(())
}

pub(crate) fn read(
    sql: &Connection,
    track: Option<i64>,
    album: Option<i64>,
) -> Fallible<Vec<Credit>> {
    all(
        sql,
        "SELECT artist_id,credit_name,role,sub_role,join_phrase FROM library_credits WHERE track_id=?1 OR album_id=?2 ORDER BY position",
        params![track, album],
        |row| {
            Ok(Credit {
                artist: ArtistRef {
                    id: row.get::<_, i64>(0)?.to_string(),
                    name: row.get(1)?,
                },
                role: row.get(2)?,
                sub_role: row.get(3)?,
                join_phrase: row.get(4)?,
            })
        },
    )
}
