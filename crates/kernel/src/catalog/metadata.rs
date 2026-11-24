use std::time::{Duration, SystemTime};

/// A reference suitable for linking to an artist.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArtistRef {
    /// Opaque artist id.
    pub id: String,
    /// Credited display name.
    pub name: String,
}

/// An artist credited in one role.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Credit {
    /// The linked artist and the credited name.
    pub artist: ArtistRef,
    /// Credit role, such as `artist`, `albumartist`, `composer` or `performer`.
    pub role: String,
    /// The instrument or other specialization of the role.
    pub sub_role: Option<String>,
    /// Verbatim separator between this artist and the following artist, if stored.
    pub join_phrase: String,
}

/// An artist and the aggregate of their library credits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Artist {
    /// Opaque artist id.
    pub id: String,
    /// Name to display.
    pub name: String,
    /// Tagged sort name, when supplied.
    pub sort_name: Option<String>,
    /// Normalized browse order name.
    pub order_name: String,
    /// `MusicBrainz` artist id.
    pub musicbrainz_id: Option<String>,
    /// Roles for which the artist has credits, in lexical order.
    pub roles: Vec<String>,
    /// Number of albums credited to this artist.
    pub album_count: u64,
    /// Artwork id, when available.
    pub cover_art: Option<String>,
}

/// A track or album content advisory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExplicitStatus {
    /// Tagged as having no advisory.
    None,
    /// Explicit material.
    Explicit,
    /// A clean version.
    Clean,
}

/// A full or partial tag date.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ItemDate {
    /// Year of release.
    pub year: u32,
    /// Month of release, when known.
    pub month: Option<u8>,
    /// Day of release, when known.
    pub day: Option<u8>,
}

/// A disc subtitle and its optional art.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiscTitle {
    /// Disc number.
    pub disc: u32,
    /// Display subtitle.
    pub title: String,
    /// Artwork id for the disc, when available.
    pub cover_art: Option<String>,
}

/// Why an album is marked as a compilation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompilationSource {
    /// At least one track carries the compilation tag.
    Tag,
    /// Inferred from the credits in the album folder.
    Derived,
}

/// Album metadata, independent of the album's current page of tracks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Album {
    /// Opaque album id.
    pub id: String,
    /// Display title.
    pub name: String,
    /// Tagged sort name, when supplied.
    pub sort_name: Option<String>,
    /// Normalized browse order name.
    pub order_name: String,
    /// Display album artist text.
    pub display_artist: String,
    /// Ordered album artist links and other credits.
    pub credits: Vec<Credit>,
    /// `MusicBrainz` release id.
    pub musicbrainz_id: Option<String>,
    /// `MusicBrainz` release-group id, not used as an identity.
    pub release_group_id: Option<String>,
    /// The tagged release edition.
    pub version: Option<String>,
    /// Tagged release types, including `compilation` for compilations.
    pub release_types: Vec<String>,
    /// Some only for compilations; identifies how they were classified.
    pub compilation: Option<CompilationSource>,
    /// Tagged release year.
    pub year: Option<u32>,
    /// Original release date.
    pub original_release_date: Option<ItemDate>,
    /// Date of this release.
    pub release_date: Option<ItemDate>,
    /// Disc subtitles in disc number order.
    pub disc_titles: Vec<DiscTitle>,
    /// Release labels.
    pub record_labels: Vec<String>,
    /// Moods tagged on the release.
    pub moods: Vec<String>,
    /// Genre names.
    pub genres: Vec<String>,
    /// Tagged content advisory, if known.
    pub explicit_status: Option<ExplicitStatus>,
    /// Selected artwork id, when available.
    pub cover_art: Option<String>,
    /// Number of tracks.
    pub song_count: u64,
    /// Combined playing time.
    pub duration: Duration,
    /// When this album was first indexed.
    pub created: SystemTime,
}
