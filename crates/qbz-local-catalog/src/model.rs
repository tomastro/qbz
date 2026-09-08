use serde::{Deserialize, Serialize};

/// Authoritative origin of a projected row. Wire values are deliberately the
/// same words the existing source/cache layers use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    Local,
    Offline,
    Plex,
    Jellyfin,
    Subsonic,
}

impl SourceKind {
    pub const ALL: [Self; 5] = [
        Self::Local,
        Self::Offline,
        Self::Plex,
        Self::Jellyfin,
        Self::Subsonic,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Offline => "offline",
            Self::Plex => "plex",
            Self::Jellyfin => "jellyfin",
            Self::Subsonic => "subsonic",
        }
    }

    pub fn from_str(value: &str) -> Option<Self> {
        match value {
            "local" => Some(Self::Local),
            "offline" => Some(Self::Offline),
            "plex" => Some(Self::Plex),
            "jellyfin" => Some(Self::Jellyfin),
            "subsonic" => Some(Self::Subsonic),
            _ => None,
        }
    }
}

/// Stable product identity. The catalog's integer rowid never leaves through
/// this type; playback and user data continue to use source-native identity.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TrackRef {
    pub source: SourceKind,
    pub source_instance: String,
    pub native_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SourceKey {
    pub source: SourceKind,
    pub source_instance: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CreditRole {
    TrackArtist,
    AlbumArtist,
    Composer,
    Performer,
    Featured,
}

impl CreditRole {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::TrackArtist => "track_artist",
            Self::AlbumArtist => "album_artist",
            Self::Composer => "composer",
            Self::Performer => "performer",
            Self::Featured => "featured",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtistCredit {
    pub display_name: String,
    pub role: CreditRole,
    pub ordinal: u32,
}

/// Source-neutral write shape. C/H adapters translate authoritative rows into
/// this type; the catalog itself never calls a source API.
#[derive(Debug, Clone)]
pub struct ProjectedTrack {
    pub track_ref: TrackRef,
    /// Raw presentation provenance from the authoritative row. Local cache
    /// rows may be `qobuz_download`/`qobuz_purchase` while their stable
    /// ownership remains [`SourceKind::Local`].
    pub source_raw: String,
    pub local_track_id: Option<i64>,
    pub local_path: Option<String>,
    /// Stable album id from the same authoritative source when available.
    /// Text-only caches may leave it empty; bootstrap then records a weak,
    /// reversible per-source fallback association.
    pub native_album_id: Option<String>,
    pub source_copy_id: Option<i64>,
    pub title: String,
    pub artist: String,
    pub album_artist: String,
    pub album: String,
    pub duration_ms: u64,
    pub year: Option<u32>,
    pub disc_number: Option<u32>,
    pub track_number: Option<u32>,
    pub format: String,
    pub bit_depth: Option<u32>,
    pub sample_rate_hz: Option<u32>,
    pub artwork_token: Option<String>,
    pub isrc: Option<String>,
    pub musicbrainz_recording_id: Option<String>,
    pub added_at: i64,
    pub available: bool,
    pub observed_generation: i64,
    pub credits: Vec<ArtistCredit>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TrackSort {
    #[default]
    Default,
    TitleAsc,
    TitleDesc,
    ArtistAsc,
    ArtistDesc,
    YearAsc,
    YearDesc,
    AddedDesc,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrackGroup {
    #[default]
    Off,
    Album,
    Artist,
    Name,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuerySurface {
    Tracks,
    Albums,
    Artists,
}

/// Immutable query snapshot. Builders consume and return a new descriptor so
/// an async request cannot observe later UI mutations through shared state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryDescriptor {
    surface: QuerySurface,
    search: String,
    sources: Vec<SourceKey>,
    source_buckets: Vec<String>,
    formats: Vec<String>,
    other_formats: bool,
    quality_tiers: Vec<String>,
    sort: TrackSort,
    group: TrackGroup,
    available_only: bool,
}

impl QueryDescriptor {
    pub fn tracks() -> Self {
        Self::for_surface(QuerySurface::Tracks)
    }

    pub fn albums() -> Self {
        Self::for_surface(QuerySurface::Albums)
    }

    pub fn artists() -> Self {
        Self::for_surface(QuerySurface::Artists)
    }

    pub fn for_surface(surface: QuerySurface) -> Self {
        Self {
            surface,
            search: String::new(),
            sources: Vec::new(),
            source_buckets: Vec::new(),
            formats: Vec::new(),
            other_formats: false,
            quality_tiers: Vec::new(),
            sort: TrackSort::Default,
            group: TrackGroup::Off,
            available_only: true,
        }
    }

    pub fn with_search(mut self, search: impl Into<String>) -> Self {
        self.search = search.into().trim().to_string();
        self
    }

    pub fn with_sources(mut self, sources: Vec<SourceKey>) -> Self {
        self.sources = sources;
        self.sources.sort();
        self.sources.dedup();
        self
    }

    pub fn with_formats(mut self, formats: Vec<String>) -> Self {
        self.formats = formats
            .into_iter()
            .map(|value| value.trim().to_ascii_lowercase())
            .filter(|value| !value.is_empty())
            .collect();
        self.formats.sort();
        self.formats.dedup();
        self
    }

    pub fn including_other_formats(mut self, include: bool) -> Self {
        self.other_formats = include;
        self
    }

    pub fn with_source_buckets(mut self, buckets: Vec<String>) -> Self {
        self.source_buckets = buckets
            .into_iter()
            .map(|value| value.trim().to_ascii_lowercase())
            .filter(|value| !value.is_empty())
            .collect();
        self.source_buckets.sort();
        self.source_buckets.dedup();
        self
    }

    pub fn with_quality_tiers(mut self, tiers: Vec<String>) -> Self {
        self.quality_tiers = tiers
            .into_iter()
            .map(|value| value.trim().to_ascii_lowercase())
            .filter(|value| !value.is_empty())
            .collect();
        self.quality_tiers.sort();
        self.quality_tiers.dedup();
        self
    }

    pub fn with_sort(mut self, sort: TrackSort) -> Self {
        self.sort = sort;
        self
    }

    pub fn with_group(mut self, group: TrackGroup) -> Self {
        self.group = group;
        self
    }

    pub fn including_unavailable(mut self) -> Self {
        self.available_only = false;
        self
    }

    pub fn surface(&self) -> QuerySurface {
        self.surface
    }

    pub fn search(&self) -> &str {
        &self.search
    }

    pub fn sources(&self) -> &[SourceKey] {
        &self.sources
    }

    pub fn source_buckets(&self) -> &[String] {
        &self.source_buckets
    }

    pub fn formats(&self) -> &[String] {
        &self.formats
    }

    pub fn other_formats(&self) -> bool {
        self.other_formats
    }

    pub fn quality_tiers(&self) -> &[String] {
        &self.quality_tiers
    }

    pub fn sort(&self) -> TrackSort {
        self.sort
    }

    pub fn group(&self) -> TrackGroup {
        self.group
    }

    pub fn available_only(&self) -> bool {
        self.available_only
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackRecord {
    pub track_ref: TrackRef,
    pub source_raw: String,
    pub local_track_id: Option<i64>,
    pub local_path: Option<String>,
    pub native_album_id: Option<String>,
    pub title: String,
    pub artist: String,
    pub album_artist: String,
    pub album: String,
    pub duration_ms: u64,
    pub year: Option<u32>,
    pub disc_number: Option<u32>,
    pub track_number: Option<u32>,
    pub format: String,
    pub bit_depth: Option<u32>,
    pub sample_rate_hz: Option<u32>,
    pub artwork_token: Option<String>,
    pub available: bool,
}

/// Opaque keyset cursor. It carries the exact normalized ORDER BY values plus
/// the catalog rowid tie-breaker, but exposes none as product identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackCursor {
    pub(crate) sort: TrackSort,
    pub(crate) descriptor_key: String,
    pub(crate) sort_title: String,
    pub(crate) sort_artist: String,
    pub(crate) sort_track_artist: String,
    pub(crate) sort_album: String,
    pub(crate) year_missing: i64,
    pub(crate) year_value: i64,
    pub(crate) disc_sort: i64,
    pub(crate) track_sort: i64,
    pub(crate) added_at: i64,
    pub(crate) row_id: i64,
}

impl TrackCursor {
    /// Normalized grouping key of the row immediately before a keyset page.
    /// This lets a paged frontend decide whether the first resident row starts
    /// a new global group without issuing an overlap query.
    pub fn group_key(&self, group: TrackGroup) -> String {
        match group {
            TrackGroup::Off => String::new(),
            TrackGroup::Album => self.sort_album.clone(),
            TrackGroup::Artist => self.sort_track_artist.clone(),
            TrackGroup::Name => self
                .sort_title
                .chars()
                .next()
                .unwrap_or('#')
                .to_uppercase()
                .collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackPage {
    pub rows: Vec<TrackRecord>,
    pub next_cursor: Option<TrackCursor>,
    pub has_more: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlbumRecord {
    pub edition_id: i64,
    pub source: SourceKind,
    pub native_album_id: String,
    pub source_raw: String,
    /// Distinct presentation source words contributed by every physical copy
    /// of this edition.  Unlike `source`, which selects one copy for routing,
    /// this list is for multi-source badges and capabilities such as local
    /// favorites.  Raw words (for example `qobuz_purchase` or a Subsonic
    /// brand) are preserved.
    pub source_words: Vec<String>,
    pub title: String,
    pub artist: String,
    pub all_artists: String,
    pub year: Option<u32>,
    pub track_count: u32,
    pub total_duration_ms: u64,
    pub quality_tier: String,
    pub format: String,
    pub bit_depth: Option<u32>,
    pub sample_rate_hz: Option<u32>,
    pub artwork_source: String,
    pub artwork_token: String,
    pub directory_path: String,
    pub folder_count: u32,
    pub added_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlbumCursor {
    pub(crate) descriptor_key: String,
    pub(crate) sort_title: String,
    pub(crate) sort_artist: String,
    pub(crate) year_missing: i64,
    pub(crate) year_value: i64,
    pub(crate) added_at: i64,
    pub(crate) edition_id: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlbumPage {
    pub rows: Vec<AlbumRecord>,
    pub next_cursor: Option<AlbumCursor>,
    pub has_more: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtistRecord {
    pub artist_key: String,
    pub display_name: String,
    pub album_count: u32,
    pub track_count: u32,
    pub artwork_source: String,
    pub artwork_token: String,
    /// `local`, a remote source word, or `mixed` when several source kinds
    /// contribute to the aggregate visible under the query descriptor.
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtistCursor {
    pub(crate) descriptor_key: String,
    pub(crate) sort_name: String,
    pub(crate) artist_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtistPage {
    pub rows: Vec<ArtistRecord>,
    pub next_cursor: Option<ArtistCursor>,
    pub has_more: bool,
}
