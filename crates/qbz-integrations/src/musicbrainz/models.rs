//! MusicBrainz API response models
//!
//! Types for deserializing MusicBrainz JSON responses

use serde::{Deserialize, Serialize};

/// Match confidence levels for MusicBrainz lookups
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MatchConfidence {
    Exact,  // ISRC/UPC exact match
    High,   // Score >= 95
    Medium, // Score >= 80
    Low,    // Score >= 60
    None,   // No match found
}

impl MatchConfidence {
    pub fn from_score(score: Option<i32>) -> Self {
        match score {
            Some(s) if s >= 100 => Self::Exact,
            Some(s) if s >= 95 => Self::High,
            Some(s) if s >= 80 => Self::Medium,
            Some(s) if s >= 60 => Self::Low,
            _ => Self::None,
        }
    }
}

/// Artist type classification
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ArtistType {
    Person,
    Group,
    Orchestra,
    Choir,
    Character,
    Other,
}

impl Default for ArtistType {
    fn default() -> Self {
        Self::Other
    }
}

impl From<Option<&str>> for ArtistType {
    fn from(s: Option<&str>) -> Self {
        match s.map(|s| s.to_lowercase()).as_deref() {
            Some("person") => Self::Person,
            Some("group") => Self::Group,
            Some("orchestra") => Self::Orchestra,
            Some("choir") => Self::Choir,
            Some("character") => Self::Character,
            _ => Self::Other,
        }
    }
}

// ============ API Response Types ============

/// Recording search response
#[derive(Debug, Deserialize)]
pub struct RecordingSearchResponse {
    pub created: Option<String>,
    pub count: i32,
    pub offset: i32,
    pub recordings: Vec<RecordingResult>,
}

/// Single recording in search results
#[derive(Debug, Deserialize)]
pub struct RecordingResult {
    pub id: String,
    pub score: Option<i32>,
    pub title: Option<String>,
    pub length: Option<i64>,
    #[serde(rename = "artist-credit")]
    pub artist_credit: Option<Vec<ArtistCredit>>,
    pub isrcs: Option<Vec<String>>,
    pub releases: Option<Vec<ReleaseRef>>,
}

/// Response from the single-recording lookup `/recording/{mbid}?inc=isrcs`.
#[derive(Debug, Deserialize)]
pub struct RecordingLookupResponse {
    pub id: String,
    pub title: Option<String>,
    pub isrcs: Option<Vec<String>>,
}

/// Artist credit entry
#[derive(Debug, Deserialize)]
pub struct ArtistCredit {
    pub name: Option<String>,
    pub joinphrase: Option<String>,
    pub artist: ArtistRef,
}

/// Reference to an artist
#[derive(Debug, Deserialize)]
pub struct ArtistRef {
    pub id: String,
    pub name: String,
    #[serde(rename = "sort-name")]
    pub sort_name: Option<String>,
    pub disambiguation: Option<String>,
}

/// Reference to a release (album)
#[derive(Debug, Deserialize)]
pub struct ReleaseRef {
    pub id: String,
    pub title: Option<String>,
    pub status: Option<String>,
    pub date: Option<String>,
    pub country: Option<String>,
    #[serde(rename = "release-group")]
    pub release_group: Option<ReleaseGroupRef>,
}

/// Reference to a release group
#[derive(Debug, Deserialize)]
pub struct ReleaseGroupRef {
    pub id: String,
    #[serde(rename = "primary-type")]
    pub primary_type: Option<String>,
}

/// Artist search response
#[derive(Debug, Deserialize)]
pub struct ArtistSearchResponse {
    pub created: Option<String>,
    pub count: i32,
    pub offset: i32,
    pub artists: Vec<ArtistResult>,
}

/// Single artist in search results
#[derive(Debug, Deserialize)]
pub struct ArtistResult {
    pub id: String,
    pub score: Option<i32>,
    pub name: String,
    #[serde(rename = "sort-name")]
    pub sort_name: Option<String>,
    #[serde(rename = "type")]
    pub artist_type: Option<String>,
    pub country: Option<String>,
    pub disambiguation: Option<String>,
    #[serde(default)]
    pub aliases: Option<Vec<Alias>>,
    #[serde(rename = "life-span")]
    pub life_span: Option<LifeSpan>,
    #[serde(default)]
    pub area: Option<Area>,
    #[serde(rename = "begin-area", default)]
    pub begin_area: Option<Area>,
    #[serde(default)]
    pub tags: Option<Vec<Tag>>,
}

/// Artist alias
#[derive(Debug, Deserialize)]
pub struct Alias {
    pub name: String,
    #[serde(rename = "sort-name")]
    pub sort_name: Option<String>,
    #[serde(rename = "type")]
    pub alias_type: Option<String>,
    pub locale: Option<String>,
    pub primary: Option<bool>,
}

/// MusicBrainz area (city, state, country, etc.)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Area {
    pub id: String,
    pub name: String,
    #[serde(rename = "sort-name")]
    pub sort_name: Option<String>,
    #[serde(rename = "type")]
    pub area_type: Option<String>,
}

/// Community tag (used for genres)
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Tag {
    pub name: String,
    pub count: Option<i32>,
}

/// Life span for an artist
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LifeSpan {
    pub begin: Option<String>,
    pub end: Option<String>,
    pub ended: Option<bool>,
}

/// Relation between entities
#[derive(Debug, Deserialize)]
pub struct Relation {
    #[serde(rename = "type")]
    pub relation_type: String,
    #[serde(rename = "type-id")]
    pub type_id: Option<String>,
    pub direction: Option<String>,
    pub begin: Option<String>,
    pub end: Option<String>,
    pub ended: Option<bool>,
    pub attributes: Option<Vec<String>>,
    pub artist: Option<ArtistRef>,
}

// ============ Resolved Types (for caching/output) ============

/// Resolved artist with all metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedArtist {
    pub mbid: String,
    pub name: String,
    pub sort_name: Option<String>,
    pub artist_type: ArtistType,
    pub country: Option<String>,
    pub disambiguation: Option<String>,
    pub confidence: MatchConfidence,
}

/// Resolved track with MusicBrainz data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedTrack {
    pub recording_mbid: String,
    pub title: String,
    pub artist_mbids: Vec<String>,
    pub release_mbid: Option<String>,
    pub isrcs: Vec<String>,
    pub confidence: MatchConfidence,
}

/// Resolved release (album) with MusicBrainz data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedRelease {
    pub mbid: String,
    pub title: String,
    pub release_group_mbid: Option<String>,
    pub date: Option<String>,
    pub country: Option<String>,
    pub confidence: MatchConfidence,
}

// ============ Full Response Types (for lookups with includes) ============

/// Full artist response (with includes like relations, tags)
#[derive(Debug, Deserialize)]
pub struct ArtistFullResponse {
    pub id: String,
    pub name: String,
    #[serde(rename = "sort-name")]
    pub sort_name: Option<String>,
    #[serde(rename = "type")]
    pub artist_type: Option<String>,
    pub country: Option<String>,
    pub disambiguation: Option<String>,
    #[serde(rename = "life-span")]
    pub life_span: Option<LifeSpan>,
    #[serde(default)]
    pub area: Option<Area>,
    #[serde(rename = "begin-area", default)]
    pub begin_area: Option<Area>,
    pub relations: Option<Vec<Relation>>,
    #[serde(default)]
    pub tags: Option<Vec<Tag>>,
}

/// Release search response
#[derive(Debug, Deserialize)]
pub struct ReleaseSearchResponse {
    pub created: Option<String>,
    pub count: i32,
    pub offset: i32,
    pub releases: Vec<ReleaseResult>,
}

/// Single release in search results
#[derive(Debug, Deserialize)]
pub struct ReleaseResult {
    pub id: String,
    pub score: Option<i32>,
    pub title: String,
    pub status: Option<String>,
    pub date: Option<String>,
    pub country: Option<String>,
    pub barcode: Option<String>,
    #[serde(rename = "label-info")]
    pub label_info: Option<Vec<LabelInfo>>,
    #[serde(rename = "release-group")]
    pub release_group: Option<ReleaseGroupRef>,
    #[serde(rename = "artist-credit")]
    pub artist_credit: Option<Vec<ArtistCredit>>,
    pub media: Option<Vec<ReleaseSearchMedium>>,
    #[serde(rename = "track-count")]
    pub track_count: Option<u16>,
}

/// Simplified medium info for search results
#[derive(Debug, Deserialize)]
pub struct ReleaseSearchMedium {
    pub format: Option<String>,
    #[serde(rename = "track-count")]
    pub track_count: Option<u16>,
}

/// Label information
#[derive(Debug, Deserialize)]
pub struct LabelInfo {
    #[serde(rename = "catalog-number")]
    pub catalog_number: Option<String>,
    pub label: Option<LabelRef>,
}

/// Reference to a label
#[derive(Debug, Deserialize)]
pub struct LabelRef {
    pub id: String,
    pub name: String,
}

/// Full release response from lookup endpoint (with tracks)
#[derive(Debug, Deserialize)]
pub struct ReleaseFullResponse {
    pub id: String,
    pub title: String,
    pub status: Option<String>,
    pub date: Option<String>,
    pub country: Option<String>,
    pub barcode: Option<String>,
    #[serde(rename = "artist-credit")]
    pub artist_credit: Option<Vec<ArtistCredit>>,
    #[serde(rename = "label-info")]
    pub label_info: Option<Vec<LabelInfo>>,
    #[serde(rename = "release-group")]
    pub release_group: Option<ReleaseGroupRef>,
    pub media: Option<Vec<Medium>>,
    pub tags: Option<Vec<Tag>>,
}

/// A medium (disc) containing tracks
#[derive(Debug, Deserialize)]
pub struct Medium {
    pub position: Option<u8>,
    pub format: Option<String>,
    #[serde(rename = "track-count")]
    pub track_count: Option<u16>,
    pub tracks: Option<Vec<MediumTrack>>,
}

/// A track on a medium
#[derive(Debug, Deserialize)]
pub struct MediumTrack {
    #[serde(default)]
    pub id: Option<String>,
    pub position: Option<u8>,
    pub number: Option<String>,
    pub title: Option<String>,
    pub length: Option<i64>,
    pub recording: Option<TrackRecording>,
}

/// Recording reference within a track
#[derive(Debug, Deserialize)]
pub struct TrackRecording {
    pub id: String,
    pub title: Option<String>,
    pub length: Option<i64>,
    #[serde(rename = "artist-credit")]
    pub artist_credit: Option<Vec<ArtistCredit>>,
}

/// Artist browse response (from browse API)
#[derive(Debug, Deserialize)]
pub struct ArtistBrowseResponse {
    #[serde(rename = "artist-count")]
    pub artist_count: Option<i32>,
    #[serde(rename = "artist-offset")]
    pub artist_offset: Option<i32>,
    pub artists: Vec<ArtistResult>,
}

/// Area search response
#[derive(Debug, Deserialize)]
pub struct AreaSearchResponse {
    pub count: Option<i32>,
    pub offset: Option<i32>,
    pub areas: Vec<AreaResult>,
}

/// Single area in search results
#[derive(Debug, Deserialize)]
pub struct AreaResult {
    pub id: String,
    pub score: Option<i32>,
    pub name: String,
    #[serde(rename = "sort-name")]
    pub sort_name: Option<String>,
    #[serde(rename = "type")]
    pub area_type: Option<String>,
    #[serde(rename = "iso-3166-1-codes", default)]
    pub iso_codes: Option<Vec<String>>,
    #[serde(rename = "life-span")]
    pub life_span: Option<LifeSpan>,
}

/// Area detail response (from /area/{id}?inc=area-rels)
#[derive(Debug, Deserialize)]
pub struct AreaDetailResponse {
    pub id: String,
    pub name: String,
    #[serde(rename = "sort-name")]
    pub sort_name: Option<String>,
    #[serde(rename = "type")]
    pub area_type: Option<String>,
    #[serde(rename = "iso-3166-1-codes", default)]
    pub iso_codes: Option<Vec<String>>,
    #[serde(rename = "iso-3166-2-codes", default)]
    pub iso_3166_2_codes: Option<Vec<String>>,
    #[serde(default)]
    pub relations: Option<Vec<AreaRelation>>,
}

/// Area relationship entry
#[derive(Debug, Deserialize)]
pub struct AreaRelation {
    #[serde(rename = "type")]
    pub relation_type: String,
    pub direction: Option<String>,
    pub area: Option<AreaRelTarget>,
}

/// Target area in an area relationship
#[derive(Debug, Deserialize)]
pub struct AreaRelTarget {
    pub id: String,
    pub name: String,
    #[serde(rename = "sort-name")]
    pub sort_name: Option<String>,
    #[serde(rename = "type")]
    pub area_type: Option<String>,
    #[serde(rename = "iso-3166-1-codes", default)]
    pub iso_codes: Option<Vec<String>>,
    #[serde(rename = "iso-3166-2-codes", default)]
    pub iso_3166_2_codes: Option<Vec<String>>,
}

// ============ Relationship Types ============

/// Related artist (for relationships)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelatedArtist {
    pub mbid: String,
    pub name: String,
    pub role: Option<String>,
    pub period: Option<Period>,
    pub ended: bool,
}

/// Time period for a relationship
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Period {
    pub begin: Option<String>,
    pub end: Option<String>,
}

/// Artist relationships
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtistRelationships {
    pub members: Vec<RelatedArtist>,
    pub past_members: Vec<RelatedArtist>,
    pub groups: Vec<RelatedArtist>,
    pub collaborators: Vec<RelatedArtist>,
}

impl ArtistRelationships {
    pub fn empty() -> Self {
        Self {
            members: Vec::new(),
            past_members: Vec::new(),
            groups: Vec::new(),
            collaborators: Vec::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.members.is_empty()
            && self.past_members.is_empty()
            && self.groups.is_empty()
            && self.collaborators.is_empty()
    }
}

// ============ Artist Metadata (Location Discovery) ============

/// Precision level for artist location data
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LocationPrecision {
    City,
    State,
    Country,
}

/// Resolved location for an artist
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtistLocation {
    pub city: Option<String>,
    pub area_id: Option<String>,
    pub country: Option<String>,
    pub country_code: Option<String>,
    pub display_name: String,
    pub precision: LocationPrecision,
}

/// Affinity seeds extracted from an artist's tags
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AffinitySeeds {
    pub genres: Vec<String>,
    pub tags: Vec<String>,
    pub normalized_seeds: Vec<String>,
}

/// Complete artist metadata for location discovery
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtistMetadata {
    pub mbid: String,
    pub name: String,
    pub artist_type: ArtistType,
    pub life_span: Option<LifeSpan>,
    pub location: Option<ArtistLocation>,
    pub affinity_seeds: AffinitySeeds,
}

/// A candidate artist from location discovery, validated against Qobuz
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocationCandidate {
    pub mbid: String,
    pub mb_name: String,
    pub qobuz_id: Option<i64>,
    pub qobuz_name: Option<String>,
    pub qobuz_image: Option<String>,
    pub score: i32,
    pub genres: Vec<String>,
    pub qobuz_albums_count: Option<u32>,
}

/// Response from the location discovery pipeline
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocationDiscoveryResponse {
    pub artists: Vec<LocationCandidate>,
    pub scene_label: String,
    pub genre_summary: String,
    pub total_candidates: usize,
    pub has_more: bool,
    pub next_offset: usize,
}

/// One candidate from the "you may also like" tag-based discovery pipeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveryArtist {
    pub mbid: String,
    pub name: String,
    pub qobuz_id: Option<u64>,
}

/// Result of the tag-based discovery pipeline. `primary_tag` is the tag
/// the discovery was seeded with — frontends save dismissals keyed by
/// it so a "thumbs down" stays sticky for that tag across artists.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveryResponse {
    pub artists: Vec<DiscoveryArtist>,
    pub primary_tag: String,
}

// ============ Musician Types ============

/// Musician confidence level for MusicBrainz <-> Qobuz matching
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MusicianConfidence {
    Confirmed,
    Contextual,
    Weak,
    None,
}

impl MusicianConfidence {
    pub fn level(&self) -> u8 {
        match self {
            Self::Confirmed => 3,
            Self::Contextual => 2,
            Self::Weak => 1,
            Self::None => 0,
        }
    }
}

impl Default for MusicianConfidence {
    fn default() -> Self {
        Self::None
    }
}

/// Resolved musician with confidence assessment
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedMusician {
    pub name: String,
    pub role: String,
    pub mbid: Option<String>,
    pub qobuz_artist_id: Option<i64>,
    pub confidence: MusicianConfidence,
    pub bands: Vec<String>,
    pub appears_on_count: usize,
}

impl ResolvedMusician {
    pub fn empty(name: String, role: String) -> Self {
        Self {
            name,
            role,
            mbid: None,
            qobuz_artist_id: None,
            confidence: MusicianConfidence::None,
            bands: Vec::new(),
            appears_on_count: 0,
        }
    }
}

/// Album appearance for a musician
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlbumAppearance {
    pub album_id: String,
    pub album_title: String,
    pub album_artwork: String,
    pub artist_name: String,
    pub year: Option<String>,
    pub role_on_album: String,
}

/// Musician appearances response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MusicianAppearances {
    pub albums: Vec<AlbumAppearance>,
    pub total: usize,
}
