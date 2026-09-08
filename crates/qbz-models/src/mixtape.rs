//! Mixtapes & Collections types.
//!
//! See spec: qbz-nix-docs/specs/2026-04-18-mixtapes-collections-design.md

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CollectionKind {
    Mixtape,
    Collection,
    ArtistCollection,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CollectionSourceType {
    Manual,
    ArtistDiscography,
    // Reserved, not implemented in this spec:
    // TagFilter, SmartFilter,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CollectionPlayMode {
    InOrder,
    AlbumShuffle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemType {
    Album,
    Track,
    Playlist,
}

/// Top-level source of a Mixtape item. `Local` is an umbrella that LocalLibrary
/// resolves internally to any of its providers (file / plex / qobuz_download /
/// qobuz_purchase / future: jellyfin / roon / …). Mixtapes do NOT know about
/// providers — when a new LocalLibrary provider is added, zero changes here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AlbumSource {
    Qobuz,
    Local,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MixtapeCollection {
    pub id: String,
    pub kind: CollectionKind,
    pub name: String,
    pub description: Option<String>,
    pub source_type: CollectionSourceType,
    pub source_ref: Option<String>,
    pub play_mode: CollectionPlayMode,
    pub custom_artwork_path: Option<String>,
    pub position: i32,
    pub hidden: bool,
    pub last_played_at: Option<i64>,
    pub play_count: i32,
    pub last_synced_at: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
    #[serde(default)]
    pub items: Vec<MixtapeCollectionItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MixtapeCollectionItem {
    pub collection_id: String,
    pub position: i32,
    pub item_type: ItemType,
    pub source: AlbumSource,
    pub source_item_id: String,
    pub title: String,
    pub subtitle: Option<String>,
    pub artwork_url: Option<String>,
    pub year: Option<i32>,
    pub track_count: Option<i32>,
    pub added_at: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_serde_roundtrip() {
        let k = CollectionKind::ArtistCollection;
        let s = serde_json::to_string(&k).unwrap();
        assert_eq!(s, "\"artist_collection\"");
        let back: CollectionKind = serde_json::from_str(&s).unwrap();
        assert_eq!(back, k);
    }

    #[test]
    fn collection_serde_omits_empty_items() {
        let c = MixtapeCollection {
            id: "abc".into(),
            kind: CollectionKind::Mixtape,
            name: "90s Cassettes".into(),
            description: None,
            source_type: CollectionSourceType::Manual,
            source_ref: None,
            play_mode: CollectionPlayMode::InOrder,
            custom_artwork_path: None,
            position: 0,
            hidden: false,
            last_played_at: None,
            play_count: 0,
            last_synced_at: None,
            created_at: 1,
            updated_at: 1,
            items: Vec::new(),
        };
        let s = serde_json::to_string(&c).unwrap();
        // items serializes as empty array (serde default Vec). Snake-case fields.
        assert!(s.contains("\"kind\":\"mixtape\""));
        assert!(s.contains("\"source_type\":\"manual\""));
        assert!(s.contains("\"play_mode\":\"in_order\""));
    }
}
