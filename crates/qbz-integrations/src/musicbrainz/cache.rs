//! MusicBrainz cache for resolved entities and settings
//!
//! SQLite-based cache with TTL expiration for MusicBrainz lookups.
//! Also persists integration settings (enabled state).

use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use super::models::{
    ArtistMetadata, ArtistRelationships, ArtistType, LocationDiscoveryResponse, MatchConfidence,
    ResolvedArtist, ResolvedTrack,
};

/// TTL for recording cache (30 days)
const RECORDING_TTL_SECS: i64 = 30 * 24 * 60 * 60;
/// TTL for artist cache (7 days)
const ARTIST_TTL_SECS: i64 = 7 * 24 * 60 * 60;
/// TTL for release cache (30 days)
const RELEASE_TTL_SECS: i64 = 30 * 24 * 60 * 60;
/// TTL for artist relationships cache (7 days)
const RELATIONS_TTL_SECS: i64 = 7 * 24 * 60 * 60;
/// TTL for artist metadata cache (30 days)
const METADATA_TTL_SECS: i64 = 30 * 24 * 60 * 60;
/// TTL for scene discovery cache (30 days)
const SCENE_TTL_SECS: i64 = 30 * 24 * 60 * 60;
/// TTL for Qobuz artist validation cache (30 days)
const QOBUZ_VALIDATION_TTL_SECS: i64 = 30 * 24 * 60 * 60;
/// TTL for the typed Qobuz artist-match cache (30 days, same as above)
const QOBUZ_ARTIST_MATCH_TTL_SECS: i64 = 30 * 24 * 60 * 60;

/// The Qobuz half of a scene validation, and NOTHING else.
///
/// This exists because the old `mb_qobuz_validation` table stores a whole
/// `LocationCandidate` — MBID, affinity score and genres included — keyed on
/// the normalised artist NAME alone. Tauri only ever wrote it; reading it back
/// would transplant one scene's identity and ranking onto another scene's
/// candidate with the same name. Only these four fields are a pure function of
/// the name (they are literally what the Qobuz artist search returns), so only
/// these four are safe to replay. The caller rebuilds the candidate around them
/// from the CURRENT mbid, score and genres.
///
/// Negative results are deliberately NOT cached: Tauri disabled its negative
/// cache (`// Negative cache — TEMPORARILY DISABLED`) and a "not on Qobuz"
/// verdict that sticks for 30 days is how a newly-added artist stays invisible.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct QobuzArtistMatch {
    pub qobuz_id: i64,
    pub name: String,
    pub image: Option<String>,
    pub albums_count: Option<u32>,
}

/// Cache statistics
#[derive(Debug, Clone, serde::Serialize)]
pub struct CacheStats {
    pub recordings: u64,
    pub artists: u64,
    pub releases: u64,
    pub relations: u64,
}

/// MusicBrainz cache
pub struct MusicBrainzCache {
    conn: Connection,
}

impl MusicBrainzCache {
    /// Create a new cache at the given path
    pub fn new(db_path: &Path) -> Result<Self, String> {
        let conn = Connection::open(db_path)
            .map_err(|e| format!("Failed to open MusicBrainz cache: {}", e))?;

        // Enable WAL mode for concurrent read/write (ADR-002)
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")
            .map_err(|e| format!("Failed to enable WAL mode: {}", e))?;

        let cache = Self { conn };
        cache.init_schema()?;

        Ok(cache)
    }

    fn init_schema(&self) -> Result<(), String> {
        self.conn
            .execute_batch(
                "
                -- Settings (enabled state, etc.)
                CREATE TABLE IF NOT EXISTS mb_settings (
                    key TEXT PRIMARY KEY,
                    value TEXT NOT NULL
                );

                -- Recordings indexed by ISRC
                CREATE TABLE IF NOT EXISTS mb_recordings (
                    isrc TEXT PRIMARY KEY,
                    data TEXT NOT NULL,
                    fetched_at INTEGER NOT NULL
                );
                CREATE INDEX IF NOT EXISTS idx_mb_recordings_fetched ON mb_recordings(fetched_at);

                -- Artists indexed by normalized name
                CREATE TABLE IF NOT EXISTS mb_artists (
                    name_normalized TEXT PRIMARY KEY,
                    data TEXT NOT NULL,
                    fetched_at INTEGER NOT NULL
                );
                CREATE INDEX IF NOT EXISTS idx_mb_artists_fetched ON mb_artists(fetched_at);

                -- Releases indexed by UPC/barcode
                CREATE TABLE IF NOT EXISTS mb_releases (
                    barcode TEXT PRIMARY KEY,
                    data TEXT NOT NULL,
                    fetched_at INTEGER NOT NULL
                );
                CREATE INDEX IF NOT EXISTS idx_mb_releases_fetched ON mb_releases(fetched_at);

                -- Artist relationships indexed by MBID
                CREATE TABLE IF NOT EXISTS mb_artist_relations (
                    mbid TEXT PRIMARY KEY,
                    data TEXT NOT NULL,
                    fetched_at INTEGER NOT NULL
                );
                CREATE INDEX IF NOT EXISTS idx_mb_relations_fetched ON mb_artist_relations(fetched_at);

                -- Artist metadata (location, genres, life span) indexed by MBID
                CREATE TABLE IF NOT EXISTS mb_artist_metadata (
                    mbid TEXT PRIMARY KEY,
                    data TEXT NOT NULL,
                    fetched_at INTEGER NOT NULL
                );
                CREATE INDEX IF NOT EXISTS idx_mb_metadata_fetched ON mb_artist_metadata(fetched_at);

                -- Scene discovery results indexed by area + seed hash
                CREATE TABLE IF NOT EXISTS mb_scene_cache (
                    cache_key TEXT PRIMARY KEY,
                    data TEXT NOT NULL,
                    fetched_at INTEGER NOT NULL
                );
                CREATE INDEX IF NOT EXISTS idx_mb_scene_fetched ON mb_scene_cache(fetched_at);

                -- Qobuz artist validation cache
                CREATE TABLE IF NOT EXISTS mb_qobuz_validation (
                    name_normalized TEXT PRIMARY KEY,
                    data TEXT NOT NULL,
                    fetched_at INTEGER NOT NULL
                );
                CREATE INDEX IF NOT EXISTS idx_mb_qobuz_validation_fetched ON mb_qobuz_validation(fetched_at);

                -- Typed Qobuz artist match (id/name/image/album count only),
                -- keyed on the normalized artist name. Separate table from
                -- mb_qobuz_validation because that one holds Tauri-era whole
                -- LocationCandidate rows that must never be replayed.
                CREATE TABLE IF NOT EXISTS mb_qobuz_artist_match (
                    name_normalized TEXT PRIMARY KEY,
                    data TEXT NOT NULL,
                    fetched_at INTEGER NOT NULL
                );
                CREATE INDEX IF NOT EXISTS idx_mb_qobuz_artist_match_fetched ON mb_qobuz_artist_match(fetched_at);

                -- V2 resolved tracks (simple cache)
                CREATE TABLE IF NOT EXISTS resolved_tracks (
                    isrc TEXT PRIMARY KEY,
                    recording_mbid TEXT NOT NULL,
                    title TEXT NOT NULL,
                    artist_mbids TEXT NOT NULL,
                    release_mbid TEXT,
                    isrcs TEXT NOT NULL,
                    confidence TEXT NOT NULL,
                    cached_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now'))
                );

                -- V2 resolved artists (simple cache)
                CREATE TABLE IF NOT EXISTS resolved_artists (
                    name_lower TEXT PRIMARY KEY,
                    mbid TEXT NOT NULL,
                    name TEXT NOT NULL,
                    sort_name TEXT,
                    artist_type TEXT NOT NULL,
                    country TEXT,
                    disambiguation TEXT,
                    confidence TEXT NOT NULL,
                    cached_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now'))
                );

                CREATE TABLE IF NOT EXISTS cache_stats (
                    key TEXT PRIMARY KEY,
                    value INTEGER NOT NULL DEFAULT 0
                );
                INSERT OR IGNORE INTO cache_stats (key, value) VALUES ('hits', 0);
                INSERT OR IGNORE INTO cache_stats (key, value) VALUES ('misses', 0);
            ",
            )
            .map_err(|e| format!("Failed to init MusicBrainz schema: {}", e))
    }

    fn current_timestamp() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0)
    }

    /// Normalize artist name for consistent cache keys
    pub fn normalize_name(name: &str) -> String {
        name.to_lowercase()
            .trim()
            .replace(['\'', '"', '.', ','], "")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    }

    // ============ Settings ============

    /// Check if MusicBrainz is enabled
    pub fn is_enabled(&self) -> Result<bool, String> {
        let result: rusqlite::Result<String> = self.conn.query_row(
            "SELECT value FROM mb_settings WHERE key = 'enabled'",
            [],
            |row| row.get(0),
        );
        match result {
            Ok(val) => Ok(val != "0"),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(true), // Default enabled
            Err(e) => Err(format!("Failed to get enabled state: {}", e)),
        }
    }

    /// Set enabled state
    pub fn set_enabled(&self, enabled: bool) -> Result<(), String> {
        let value = if enabled { "1" } else { "0" };
        self.conn
            .execute(
                "INSERT OR REPLACE INTO mb_settings (key, value) VALUES ('enabled', ?)",
                [value],
            )
            .map_err(|e| format!("Failed to set enabled: {}", e))?;
        Ok(())
    }

    // ============ Recording Cache (JSON-serialized) ============

    /// Get cached recording by ISRC (legacy format)
    pub fn get_recording(&self, isrc: &str) -> Result<Option<serde_json::Value>, String> {
        let min_fetched_at = Self::current_timestamp() - RECORDING_TTL_SECS;
        let result: Option<String> = self
            .conn
            .query_row(
                "SELECT data FROM mb_recordings WHERE isrc = ? AND fetched_at > ?",
                params![isrc, min_fetched_at],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| format!("Failed to query recording cache: {}", e))?;

        if let Some(data) = result {
            serde_json::from_str(&data)
                .map(Some)
                .map_err(|e| format!("Failed to parse cached recording: {}", e))
        } else {
            Ok(None)
        }
    }

    /// Cache a recording (JSON-serialized)
    pub fn set_recording<T: serde::Serialize>(&self, isrc: &str, data: &T) -> Result<(), String> {
        let fetched_at = Self::current_timestamp();
        let json = serde_json::to_string(data)
            .map_err(|e| format!("Failed to serialize recording: {}", e))?;
        self.conn
            .execute(
                "INSERT OR REPLACE INTO mb_recordings (isrc, data, fetched_at) VALUES (?, ?, ?)",
                params![isrc, json, fetched_at],
            )
            .map_err(|e| format!("Failed to cache recording: {}", e))?;
        Ok(())
    }

    // ============ Artist Cache (JSON-serialized) ============

    /// Get cached artist by name (JSON-serialized)
    pub fn get_artist_by_name<T: serde::de::DeserializeOwned>(
        &self,
        name: &str,
    ) -> Result<Option<T>, String> {
        let normalized = Self::normalize_name(name);
        let min_fetched_at = Self::current_timestamp() - ARTIST_TTL_SECS;
        let result: Option<String> = self
            .conn
            .query_row(
                "SELECT data FROM mb_artists WHERE name_normalized = ? AND fetched_at > ?",
                params![normalized, min_fetched_at],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| format!("Failed to query artist cache: {}", e))?;

        if let Some(data) = result {
            serde_json::from_str(&data)
                .map(Some)
                .map_err(|e| format!("Failed to parse cached artist: {}", e))
        } else {
            Ok(None)
        }
    }

    /// Cache an artist (JSON-serialized)
    pub fn set_artist_by_name<T: serde::Serialize>(
        &self,
        name: &str,
        data: &T,
    ) -> Result<(), String> {
        let normalized = Self::normalize_name(name);
        let fetched_at = Self::current_timestamp();
        let json = serde_json::to_string(data)
            .map_err(|e| format!("Failed to serialize artist: {}", e))?;
        self.conn
            .execute(
                "INSERT OR REPLACE INTO mb_artists (name_normalized, data, fetched_at) VALUES (?, ?, ?)",
                params![normalized, json, fetched_at],
            )
            .map_err(|e| format!("Failed to cache artist: {}", e))?;
        Ok(())
    }

    // ============ Release Cache ============

    /// Get cached release by barcode
    pub fn get_release<T: serde::de::DeserializeOwned>(
        &self,
        barcode: &str,
    ) -> Result<Option<T>, String> {
        let min_fetched_at = Self::current_timestamp() - RELEASE_TTL_SECS;
        let result: Option<String> = self
            .conn
            .query_row(
                "SELECT data FROM mb_releases WHERE barcode = ? AND fetched_at > ?",
                params![barcode, min_fetched_at],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| format!("Failed to query release cache: {}", e))?;

        if let Some(data) = result {
            serde_json::from_str(&data)
                .map(Some)
                .map_err(|e| format!("Failed to parse cached release: {}", e))
        } else {
            Ok(None)
        }
    }

    /// Cache a release
    pub fn set_release<T: serde::Serialize>(&self, barcode: &str, data: &T) -> Result<(), String> {
        let fetched_at = Self::current_timestamp();
        let json = serde_json::to_string(data)
            .map_err(|e| format!("Failed to serialize release: {}", e))?;
        self.conn
            .execute(
                "INSERT OR REPLACE INTO mb_releases (barcode, data, fetched_at) VALUES (?, ?, ?)",
                params![barcode, json, fetched_at],
            )
            .map_err(|e| format!("Failed to cache release: {}", e))?;
        Ok(())
    }

    // ============ Artist Relations Cache ============

    /// Get cached artist relationships by MBID
    pub fn get_artist_relations(&self, mbid: &str) -> Result<Option<ArtistRelationships>, String> {
        let min_fetched_at = Self::current_timestamp() - RELATIONS_TTL_SECS;
        let result: Option<String> = self
            .conn
            .query_row(
                "SELECT data FROM mb_artist_relations WHERE mbid = ? AND fetched_at > ?",
                params![mbid, min_fetched_at],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| format!("Failed to query relations cache: {}", e))?;

        if let Some(data) = result {
            serde_json::from_str(&data)
                .map(Some)
                .map_err(|e| format!("Failed to parse cached relations: {}", e))
        } else {
            Ok(None)
        }
    }

    /// Cache artist relationships
    pub fn set_artist_relations(
        &self,
        mbid: &str,
        data: &ArtistRelationships,
    ) -> Result<(), String> {
        let fetched_at = Self::current_timestamp();
        let json = serde_json::to_string(data)
            .map_err(|e| format!("Failed to serialize relations: {}", e))?;
        self.conn
            .execute(
                "INSERT OR REPLACE INTO mb_artist_relations (mbid, data, fetched_at) VALUES (?, ?, ?)",
                params![mbid, json, fetched_at],
            )
            .map_err(|e| format!("Failed to cache relations: {}", e))?;
        Ok(())
    }

    // ============ Artist Metadata Cache ============

    /// Get cached artist metadata by MBID
    pub fn get_artist_metadata(&self, mbid: &str) -> Result<Option<ArtistMetadata>, String> {
        let min_fetched_at = Self::current_timestamp() - METADATA_TTL_SECS;
        let result: Option<String> = self
            .conn
            .query_row(
                "SELECT data FROM mb_artist_metadata WHERE mbid = ? AND fetched_at > ?",
                params![mbid, min_fetched_at],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| format!("Failed to query metadata cache: {}", e))?;

        if let Some(data) = result {
            serde_json::from_str(&data)
                .map(Some)
                .map_err(|e| format!("Failed to parse cached metadata: {}", e))
        } else {
            Ok(None)
        }
    }

    /// Cache artist metadata
    pub fn set_artist_metadata(&self, mbid: &str, data: &ArtistMetadata) -> Result<(), String> {
        let fetched_at = Self::current_timestamp();
        let json = serde_json::to_string(data)
            .map_err(|e| format!("Failed to serialize metadata: {}", e))?;
        self.conn
            .execute(
                "INSERT OR REPLACE INTO mb_artist_metadata (mbid, data, fetched_at) VALUES (?, ?, ?)",
                params![mbid, json, fetched_at],
            )
            .map_err(|e| format!("Failed to cache metadata: {}", e))?;
        Ok(())
    }

    // ============ Scene Discovery Cache ============

    /// Get cached scene discovery results
    pub fn get_scene_cache(
        &self,
        cache_key: &str,
    ) -> Result<Option<LocationDiscoveryResponse>, String> {
        let min_fetched_at = Self::current_timestamp() - SCENE_TTL_SECS;
        let result: Option<String> = self
            .conn
            .query_row(
                "SELECT data FROM mb_scene_cache WHERE cache_key = ? AND fetched_at > ?",
                params![cache_key, min_fetched_at],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| format!("Failed to query scene cache: {}", e))?;

        if let Some(data) = result {
            serde_json::from_str(&data)
                .map(Some)
                .map_err(|e| format!("Failed to parse cached scene: {}", e))
        } else {
            Ok(None)
        }
    }

    /// Cache scene discovery results
    pub fn set_scene_cache(
        &self,
        cache_key: &str,
        data: &LocationDiscoveryResponse,
    ) -> Result<(), String> {
        let fetched_at = Self::current_timestamp();
        let json =
            serde_json::to_string(data).map_err(|e| format!("Failed to serialize scene: {}", e))?;
        self.conn
            .execute(
                "INSERT OR REPLACE INTO mb_scene_cache (cache_key, data, fetched_at) VALUES (?, ?, ?)",
                params![cache_key, json, fetched_at],
            )
            .map_err(|e| format!("Failed to cache scene: {}", e))?;
        Ok(())
    }

    /// Drop every cached scene, leaving the rest of the cache alone.
    ///
    /// Targeted invalidation for the cases a post-filter cannot fix: the
    /// blacklist is applied INSIDE the validation loop (a blocked artist makes
    /// the loop pick the next-best same-name Qobuz artist for that MBID), so
    /// UN-blacklisting cannot be undone by filtering on the way out. Also the
    /// right hammer on account switch, because the account territory decides
    /// which candidates validate and is not yet part of the key.
    ///
    /// Returns the number of rows removed.
    pub fn clear_scene_cache(&self) -> Result<usize, String> {
        let deleted = self
            .conn
            .execute("DELETE FROM mb_scene_cache", [])
            .map_err(|e| format!("Failed to clear scene cache: {}", e))?;
        if deleted > 0 {
            log::info!("MusicBrainz scene cache invalidated: {} entries", deleted);
        }
        Ok(deleted)
    }

    // ============ Qobuz Validation Cache ============

    /// Get the typed Qobuz match for a normalized artist name.
    ///
    /// A parse failure is a MISS, not an error — the row is from an older
    /// shape and re-validating costs one search.
    pub fn get_qobuz_artist_match(
        &self,
        name_normalized: &str,
    ) -> Result<Option<QobuzArtistMatch>, String> {
        let min_fetched_at = Self::current_timestamp() - QOBUZ_ARTIST_MATCH_TTL_SECS;
        let result: Option<String> = self
            .conn
            .query_row(
                "SELECT data FROM mb_qobuz_artist_match WHERE name_normalized = ? AND fetched_at > ?",
                params![name_normalized, min_fetched_at],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| format!("Failed to query artist-match cache: {}", e))?;

        Ok(result.and_then(|data| serde_json::from_str(&data).ok()))
    }

    /// Cache the typed Qobuz match for a normalized artist name.
    pub fn set_qobuz_artist_match(
        &self,
        name_normalized: &str,
        data: &QobuzArtistMatch,
    ) -> Result<(), String> {
        let fetched_at = Self::current_timestamp();
        let json = serde_json::to_string(data)
            .map_err(|e| format!("Failed to serialize artist match: {}", e))?;
        self.conn
            .execute(
                "INSERT OR REPLACE INTO mb_qobuz_artist_match (name_normalized, data, fetched_at) VALUES (?, ?, ?)",
                params![name_normalized, json, fetched_at],
            )
            .map_err(|e| format!("Failed to cache artist match: {}", e))?;
        Ok(())
    }

    /// Get cached Qobuz validation result for an artist name
    ///
    /// DEAD in this tree and left that way on purpose: the stored value is a
    /// whole scene-specific `LocationCandidate` keyed on the name alone, so
    /// replaying it in another scene transplants the wrong MBID, score and
    /// genres. [`Self::get_qobuz_artist_match`] is the safe replacement.
    pub fn get_qobuz_validation(&self, name_normalized: &str) -> Result<Option<String>, String> {
        let min_fetched_at = Self::current_timestamp() - QOBUZ_VALIDATION_TTL_SECS;
        let result: Option<String> = self
            .conn
            .query_row(
                "SELECT data FROM mb_qobuz_validation WHERE name_normalized = ? AND fetched_at > ?",
                params![name_normalized, min_fetched_at],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| format!("Failed to query validation cache: {}", e))?;
        Ok(result)
    }

    /// Cache Qobuz validation result
    pub fn set_qobuz_validation(&self, name_normalized: &str, data: &str) -> Result<(), String> {
        let fetched_at = Self::current_timestamp();
        self.conn
            .execute(
                "INSERT OR REPLACE INTO mb_qobuz_validation (name_normalized, data, fetched_at) VALUES (?, ?, ?)",
                params![name_normalized, data, fetched_at],
            )
            .map_err(|e| format!("Failed to cache validation: {}", e))?;
        Ok(())
    }

    // ============ V2 Resolved Types (structured cache) ============

    /// Get cached track by ISRC (V2 structured format)
    pub fn get_track(&self, isrc: &str) -> Result<Option<ResolvedTrack>, String> {
        let result: rusqlite::Result<ResolvedTrack> = self.conn.query_row(
            "SELECT recording_mbid, title, artist_mbids, release_mbid, isrcs, confidence
             FROM resolved_tracks WHERE isrc = ?",
            [isrc],
            |row| {
                let artist_mbids_json: String = row.get(2)?;
                let isrcs_json: String = row.get(4)?;
                let confidence_str: String = row.get(5)?;
                Ok(ResolvedTrack {
                    recording_mbid: row.get(0)?,
                    title: row.get(1)?,
                    artist_mbids: serde_json::from_str(&artist_mbids_json).unwrap_or_default(),
                    release_mbid: row.get(3)?,
                    isrcs: serde_json::from_str(&isrcs_json).unwrap_or_default(),
                    confidence: match confidence_str.as_str() {
                        "exact" => MatchConfidence::Exact,
                        "high" => MatchConfidence::High,
                        "medium" => MatchConfidence::Medium,
                        "low" => MatchConfidence::Low,
                        _ => MatchConfidence::None,
                    },
                })
            },
        );
        match result {
            Ok(track) => {
                self.increment_stat("hits");
                Ok(Some(track))
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => {
                self.increment_stat("misses");
                Ok(None)
            }
            Err(e) => Err(format!("Failed to get track: {}", e)),
        }
    }

    /// Cache a resolved track (V2 structured format)
    pub fn put_track(&self, isrc: &str, track: &ResolvedTrack) -> Result<(), String> {
        let artist_mbids_json = serde_json::to_string(&track.artist_mbids).unwrap_or_default();
        let isrcs_json = serde_json::to_string(&track.isrcs).unwrap_or_default();
        let confidence = match track.confidence {
            MatchConfidence::Exact => "exact",
            MatchConfidence::High => "high",
            MatchConfidence::Medium => "medium",
            MatchConfidence::Low => "low",
            MatchConfidence::None => "none",
        };
        self.conn
            .execute(
                "INSERT OR REPLACE INTO resolved_tracks (isrc, recording_mbid, title, artist_mbids, release_mbid, isrcs, confidence)
                 VALUES (?, ?, ?, ?, ?, ?, ?)",
                params![isrc, track.recording_mbid, track.title, artist_mbids_json, track.release_mbid, isrcs_json, confidence],
            )
            .map_err(|e| format!("Failed to cache track: {}", e))?;
        Ok(())
    }

    /// Get cached artist by name (V2 structured format)
    pub fn get_artist(&self, name: &str) -> Result<Option<ResolvedArtist>, String> {
        let name_lower = name.to_lowercase();
        let result: rusqlite::Result<ResolvedArtist> = self.conn.query_row(
            "SELECT mbid, name, sort_name, artist_type, country, disambiguation, confidence
             FROM resolved_artists WHERE name_lower = ?",
            [&name_lower],
            |row| {
                let artist_type_str: String = row.get(3)?;
                let confidence_str: String = row.get(6)?;
                Ok(ResolvedArtist {
                    mbid: row.get(0)?,
                    name: row.get(1)?,
                    sort_name: row.get(2)?,
                    artist_type: ArtistType::from(Some(artist_type_str.as_str())),
                    country: row.get(4)?,
                    disambiguation: row.get(5)?,
                    confidence: match confidence_str.as_str() {
                        "exact" => MatchConfidence::Exact,
                        "high" => MatchConfidence::High,
                        "medium" => MatchConfidence::Medium,
                        "low" => MatchConfidence::Low,
                        _ => MatchConfidence::None,
                    },
                })
            },
        );
        match result {
            Ok(artist) => {
                self.increment_stat("hits");
                Ok(Some(artist))
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => {
                self.increment_stat("misses");
                Ok(None)
            }
            Err(e) => Err(format!("Failed to get artist: {}", e)),
        }
    }

    /// Cache a resolved artist (V2 structured format)
    pub fn put_artist(&self, artist: &ResolvedArtist) -> Result<(), String> {
        let name_lower = artist.name.to_lowercase();
        let artist_type = match artist.artist_type {
            ArtistType::Person => "person",
            ArtistType::Group => "group",
            ArtistType::Orchestra => "orchestra",
            ArtistType::Choir => "choir",
            ArtistType::Character => "character",
            ArtistType::Other => "other",
        };
        let confidence = match artist.confidence {
            MatchConfidence::Exact => "exact",
            MatchConfidence::High => "high",
            MatchConfidence::Medium => "medium",
            MatchConfidence::Low => "low",
            MatchConfidence::None => "none",
        };
        self.conn
            .execute(
                "INSERT OR REPLACE INTO resolved_artists (name_lower, mbid, name, sort_name, artist_type, country, disambiguation, confidence)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
                params![name_lower, artist.mbid, artist.name, artist.sort_name, artist_type, artist.country, artist.disambiguation, confidence],
            )
            .map_err(|e| format!("Failed to cache artist: {}", e))?;
        Ok(())
    }

    // ============ Maintenance ============

    /// Clear expired entries from all tables
    pub fn cleanup_expired(&self) -> Result<usize, String> {
        let now = Self::current_timestamp();
        let mut total_deleted = 0;

        let tables_and_ttls = [
            ("mb_recordings", RECORDING_TTL_SECS),
            ("mb_artists", ARTIST_TTL_SECS),
            ("mb_releases", RELEASE_TTL_SECS),
            ("mb_artist_relations", RELATIONS_TTL_SECS),
            ("mb_artist_metadata", METADATA_TTL_SECS),
            ("mb_scene_cache", SCENE_TTL_SECS),
            ("mb_qobuz_validation", QOBUZ_VALIDATION_TTL_SECS),
            ("mb_qobuz_artist_match", QOBUZ_ARTIST_MATCH_TTL_SECS),
        ];

        for (table, ttl) in &tables_and_ttls {
            total_deleted += self
                .conn
                .execute(
                    &format!("DELETE FROM {} WHERE fetched_at <= ?", table),
                    params![now - ttl],
                )
                .map_err(|e| format!("Failed to cleanup {}: {}", table, e))?;
        }

        if total_deleted > 0 {
            log::info!(
                "MusicBrainz cache cleanup: removed {} expired entries",
                total_deleted
            );
        }
        Ok(total_deleted)
    }

    /// Clear all cached data (not settings)
    pub fn clear_all(&self) -> Result<(), String> {
        self.conn
            .execute_batch(
                "
                DELETE FROM mb_recordings;
                DELETE FROM mb_artists;
                DELETE FROM mb_releases;
                DELETE FROM mb_artist_relations;
                DELETE FROM mb_artist_metadata;
                DELETE FROM mb_scene_cache;
                DELETE FROM mb_qobuz_validation;
                DELETE FROM mb_qobuz_artist_match;
                DELETE FROM resolved_tracks;
                DELETE FROM resolved_artists;
                UPDATE cache_stats SET value = 0;
                ",
            )
            .map_err(|e| format!("Failed to clear MusicBrainz cache: {}", e))?;
        log::info!("MusicBrainz cache cleared");
        Ok(())
    }

    /// Get cache statistics
    pub fn get_stats(&self) -> Result<CacheStats, String> {
        let recordings: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM mb_recordings", [], |row| row.get(0))
            .unwrap_or(0);
        let artists: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM mb_artists", [], |row| row.get(0))
            .unwrap_or(0);
        let releases: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM mb_releases", [], |row| row.get(0))
            .unwrap_or(0);
        let relations: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM mb_artist_relations", [], |row| {
                row.get(0)
            })
            .unwrap_or(0);

        Ok(CacheStats {
            recordings: recordings as u64,
            artists: artists as u64,
            releases: releases as u64,
            relations: relations as u64,
        })
    }

    /// TTL-based cleanup (V2 style)
    pub fn cleanup(&self, ttl_days: u32) -> Result<(u64, u64), String> {
        let cutoff = chrono::Utc::now().timestamp() - (ttl_days as i64 * 86400);
        let tracks_deleted =
            self.conn
                .execute("DELETE FROM resolved_tracks WHERE cached_at < ?", [cutoff])
                .map_err(|e| format!("Failed to cleanup tracks: {}", e))? as u64;
        let artists_deleted =
            self.conn
                .execute("DELETE FROM resolved_artists WHERE cached_at < ?", [cutoff])
                .map_err(|e| format!("Failed to cleanup artists: {}", e))? as u64;
        Ok((tracks_deleted, artists_deleted))
    }

    fn increment_stat(&self, key: &str) {
        let _ = self.conn.execute(
            "UPDATE cache_stats SET value = value + 1 WHERE key = ?",
            [key],
        );
    }
}
