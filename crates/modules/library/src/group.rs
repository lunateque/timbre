use crate::credits;
use crate::scan::Source;
use crate::store::{all, row_path};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use timbre_edge_config::serde;
use timbre_edge_sqlite::json;
use timbre_edge_sqlite::rusqlite::Connection;
use timbre_edge_text::fold;
use timbre_kernel::Fallible;

pub(crate) struct Song {
    pub(crate) id: i64,
    pub(crate) source: Source,
    pub(crate) art: Option<String>,
    pub(crate) art_front: bool,
    pub(crate) parent: i64,
    pub(crate) folder: i64,
    pub(crate) directory: PathBuf,
}

impl Song {
    pub(crate) fn disc(&self) -> Option<u32> {
        self.source
            .fields
            .disc_number
            .or_else(|| disc_name(&self.directory))
    }

    fn raw(&self, album_folder: i64) -> Raw {
        let fields = &self.source.fields;
        let fallback = if self.folder == album_folder {
            Some(self.directory.as_path())
        } else {
            self.directory.parent()
        };
        let directory_name = fallback
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            .unwrap_or("Unknown Album");
        Raw {
            folder: album_folder,
            release: fields
                .musicbrainz_album_id
                .as_deref()
                .map(str::trim)
                .filter(|id| !id.is_empty())
                .map(str::to_ascii_lowercase),
            artist: credits::display(fields.album_artist.as_ref(), &fields.album_artists),
            title: fields
                .album
                .clone()
                .unwrap_or_else(|| directory_name.to_owned()),
            version: fields.album_version.clone(),
        }
    }
}

#[derive(Clone, serde::Serialize)]
#[serde(crate = "timbre_edge_config::serde")]
pub(crate) struct Raw {
    pub(crate) folder: i64,
    release: Option<String>,
    artist: String,
    pub(crate) title: String,
    version: Option<String>,
}

impl Raw {
    pub(crate) fn source(&self) -> &'static str {
        if self.release.is_some() {
            "musicbrainz_release"
        } else {
            "folder"
        }
    }

    fn key(&self) -> String {
        if let Some(id) = &self.release {
            return format!("release:{id}");
        }
        // Length prefixes keep actual field boundaries even when tags contain separators.
        let artist = fold(&self.artist);
        let title = fold(&self.title);
        let version = self.version.as_deref().map_or_else(String::new, fold);
        format!(
            "folder:{}:{}:{artist}{}:{title}{}:{version}",
            self.folder,
            artist.len(),
            title.len(),
            version.len()
        )
    }

    fn partition(&self) -> Option<(i64, Option<String>)> {
        self.release
            .is_none()
            .then(|| (self.folder, self.version.as_deref().map(fold)))
    }
}

pub(crate) struct AlbumGroup {
    pub(crate) raw: Raw,
    pub(crate) songs: Vec<(Song, Option<Raw>)>,
}

impl AlbumGroup {
    pub(crate) fn compilation(&self) -> Option<&'static str> {
        if self
            .songs
            .iter()
            .any(|(song, _)| song.source.fields.compilation == Some(true))
        {
            return Some("tag");
        }
        if self.songs.iter().any(|(song, _)| album_artist(song)) {
            return None;
        }
        // A couple of guest tracks does not make a compilation: most of the
        // album has to disagree with the artist the album is mostly by.
        let mut counts: BTreeMap<String, usize> = BTreeMap::new();
        for (song, _) in &self.songs {
            let fields = &song.source.fields;
            let name = fold(&credits::display(fields.artist.as_ref(), &fields.artists));
            if !name.is_empty() {
                *counts.entry(name).or_default() += 1;
            }
        }
        if counts.len() < 2 {
            return None;
        }
        let modal = counts.values().copied().max().unwrap_or(0);
        let differing = counts.values().sum::<usize>().saturating_sub(modal);
        (differing.saturating_mul(2) > self.songs.len()).then_some("derived")
    }
}

fn album_artist(song: &Song) -> bool {
    credits::has_album_artist(&song.source.fields)
}

pub(crate) fn songs(sql: &Connection) -> Fallible<Vec<Song>> {
    let rows = all(
        sql,
        "SELECT t.id,t.source,t.art_hash,t.art_front,f.id,coalesce(f.parent_id,f.id),f.absolute FROM library_tracks t JOIN library_folders f ON f.id=(SELECT p.folder_id FROM library_track_paths p JOIN library_roots r ON r.id=p.root_id WHERE p.track_id=t.id ORDER BY r.ordinal,p.relative LIMIT 1) ORDER BY f.id,t.disc,t.track_number,t.id",
        [],
        |row| {
            Ok((
                row.get(0)?,
                row.get::<_, String>(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
                row_path(row, 6)?,
            ))
        },
    )?;
    rows.into_iter()
        .map(|(id, source, art, art_front, folder, parent, directory)| {
            Ok(Song {
                id,
                source: json::decode(&source)?,
                art,
                art_front,
                parent,
                folder,
                directory,
            })
        })
        .collect()
}

pub(crate) fn group(songs: Vec<Song>) -> BTreeMap<String, AlbumGroup> {
    let tagged = tagged_disc_folders(&songs);
    let songs: Vec<_> = songs
        .into_iter()
        .map(|song| {
            let folder = if disc_name(&song.directory).is_some() || tagged.contains(&song.folder) {
                song.parent
            } else {
                song.folder
            };
            let raw = song.raw(folder);
            (song, raw)
        })
        .collect();
    let majorities = majority(&songs);
    let mut albums = BTreeMap::new();
    for (song, raw) in songs {
        let own = raw.key();
        let majority = raw
            .partition()
            .and_then(|partition| majorities.get(&partition));
        let key = majority.cloned().unwrap_or_else(|| own.clone());
        let chosen = key == own;
        let group: &mut AlbumGroup = albums.entry(key).or_insert_with(|| AlbumGroup {
            raw: raw.clone(),
            songs: Vec::new(),
        });
        if chosen
            && group
                .songs
                .iter()
                .all(|(_, overridden)| overridden.is_some())
        {
            group.raw = raw.clone();
        }
        group.songs.push((song, (!chosen).then_some(raw)));
    }
    for album in albums.values_mut() {
        album
            .songs
            .sort_by_key(|(song, _)| (song.disc(), song.source.fields.track_number, song.id));
    }
    albums
}

type Partition = (i64, Option<String>);

fn majority(songs: &[(Song, Raw)]) -> BTreeMap<Partition, String> {
    let mut folders: BTreeMap<_, (usize, BTreeMap<String, usize>)> = BTreeMap::new();
    for (_, raw) in songs {
        let Some(partition) = raw.partition() else {
            continue;
        };
        let (total, counts) = folders.entry(partition).or_default();
        *total += 1;
        *counts.entry(raw.key()).or_default() += 1;
    }
    folders
        .into_iter()
        .filter_map(|(partition, (total, counts))| {
            counts
                .into_iter()
                .find(|(_, count)| *count > total / 2)
                .map(|(key, _)| (partition, key))
        })
        .collect()
}

fn tagged_disc_folders(songs: &[Song]) -> BTreeSet<i64> {
    let mut siblings: BTreeMap<_, BTreeMap<_, BTreeSet<_>>> = BTreeMap::new();
    for song in songs {
        let fields = &song.source.fields;
        let (Some(disc), Some(title)) = (fields.disc_number, fields.album.as_deref()) else {
            continue;
        };
        if song.folder != song.parent {
            siblings
                .entry((song.parent, fold(title)))
                .or_default()
                .entry(song.folder)
                .or_default()
                .insert(disc);
        }
    }
    let mut joined = BTreeSet::new();
    for folders in siblings.into_values() {
        let distinct: BTreeSet<_> = folders
            .values()
            .filter(|discs| discs.len() == 1)
            .flatten()
            .collect();
        if distinct.len() > 1 {
            joined.extend(
                folders
                    .into_iter()
                    .filter(|(_, disc)| disc.len() == 1)
                    .map(|(folder, _)| folder),
            );
        }
    }
    joined
}

fn disc_name(path: &Path) -> Option<u32> {
    let name = path.file_name()?.to_str()?.to_ascii_lowercase();
    let number = name
        .strip_prefix("disc")
        .or_else(|| name.strip_prefix("cd"))?
        .trim_start_matches([' ', '_', '-']);
    number.parse::<u32>().ok().filter(|&number| number != 0)
}
