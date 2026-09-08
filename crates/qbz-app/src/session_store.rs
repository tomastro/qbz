//! Session persistence store.
//!
//! The playback queue/session state is portable application state. The current
//! Tauri/Svelte shell also stores view restoration fields in the same DB table;
//! those fields are modeled here only so the existing schema can round-trip
//! unchanged during the extraction.

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

fn default_streamable() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PersistedQueueTrack {
    pub id: u64,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration_secs: u64,
    pub artwork_url: Option<String>,
    #[serde(default)]
    pub hires: bool,
    pub bit_depth: Option<u32>,
    pub sample_rate: Option<f64>,
    #[serde(default)]
    pub is_local: bool,
    pub album_id: Option<String>,
    pub artist_id: Option<u64>,
    /// Resolved availability — absence was already interpreted upstream by
    /// `qbz_models::Track::is_streamable`, so this stays a plain `bool` while
    /// the catalog model is an `Option<bool>`, and `default_streamable` (TRUE)
    /// covers a JSON snapshot written before the column existed. Values written
    /// before the model became tri-state are untrustworthy and are cleared once
    /// by the `user_version = 1` migration in `open_at`; from that point on
    /// `false` here means only "Qobuz said no", which is a state a DOWNLOADED
    /// track legitimately persists in.
    #[serde(default = "default_streamable")]
    pub streamable: bool,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub parental_warning: bool,
    #[serde(default)]
    pub source_item_id_hint: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PersistedPlaybackSession {
    pub queue_tracks: Vec<PersistedQueueTrack>,
    pub current_index: Option<usize>,
    pub current_position_secs: u64,
    pub volume: f32,
    pub shuffle_enabled: bool,
    pub repeat_mode: String,
    pub was_playing: bool,
    pub saved_at: i64,
}

/// Additive queue fields that the legacy portable session shape cannot carry
/// without breaking older frontend struct literals. Positions refer to the
/// simultaneously persisted `queue_tracks` vector.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistedQueueTrackEdition {
    pub position: usize,
    pub track_id: u64,
    pub version: Option<String>,
    pub album_version: Option<String>,
}

/// One oldest-first play-history entry. `track_id` validates that `position`
/// still refers to the same queue snapshot before the entry is restored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistedQueueHistoryEntry {
    pub position: usize,
    pub track_id: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PersistedQueueExtras {
    pub editions: Vec<PersistedQueueTrackEdition>,
    pub history: Vec<PersistedQueueHistoryEntry>,
}

impl Default for PersistedPlaybackSession {
    fn default() -> Self {
        Self {
            queue_tracks: Vec::new(),
            current_index: None,
            current_position_secs: 0,
            volume: 0.75,
            shuffle_enabled: false,
            repeat_mode: "off".to_string(),
            was_playing: false,
            saved_at: 0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PersistedShellViewState {
    #[serde(default = "default_last_view")]
    pub last_view: String,
    #[serde(default)]
    pub view_context_id: Option<String>,
    #[serde(default)]
    pub view_context_type: Option<String>,
}

fn default_last_view() -> String {
    "home".to_string()
}

impl Default for PersistedShellViewState {
    fn default() -> Self {
        Self {
            last_view: "home".to_string(),
            view_context_id: None,
            view_context_type: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct PersistedSessionSnapshot {
    pub playback: PersistedPlaybackSession,
    pub shell_view: PersistedShellViewState,
}

pub struct SessionStore {
    conn: Connection,
}

impl SessionStore {
    pub fn new() -> Result<Self, String> {
        let data_dir = dirs::data_dir()
            .ok_or("Could not determine data directory")?
            .join("qbz");
        Self::open_at(&data_dir, "session.db")
    }

    pub fn new_at(base_dir: &Path) -> Result<Self, String> {
        Self::open_at(base_dir, "session.db")
    }

    fn open_at(dir: &Path, db_name: &str) -> Result<Self, String> {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("Failed to create data directory: {}", e))?;

        let db_path = dir.join(db_name);
        let conn = Connection::open(&db_path)
            .map_err(|e| format!("Failed to open session database: {}", e))?;

        // WAL mode for non-blocking reads/writes (ADR-002). synchronous=FULL,
        // not NORMAL: the session DB must survive hard reboots (issue #440).
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;")
            .map_err(|e| format!("Failed to set WAL mode: {}", e))?;

        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS player_state (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                current_index INTEGER,
                current_position_secs INTEGER NOT NULL DEFAULT 0,
                volume REAL NOT NULL DEFAULT 0.75,
                shuffle_enabled INTEGER NOT NULL DEFAULT 0,
                repeat_mode TEXT NOT NULL DEFAULT 'off',
                was_playing INTEGER NOT NULL DEFAULT 0,
                saved_at INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS queue_tracks (
                position INTEGER PRIMARY KEY,
                track_id INTEGER NOT NULL,
                title TEXT NOT NULL,
                artist TEXT NOT NULL,
                album TEXT NOT NULL,
                duration_secs INTEGER NOT NULL,
                artwork_url TEXT,
                hires INTEGER NOT NULL DEFAULT 0,
                bit_depth INTEGER,
                sample_rate REAL,
                source TEXT
            );

            CREATE TABLE IF NOT EXISTS queue_track_extras (
                position INTEGER PRIMARY KEY,
                track_id INTEGER NOT NULL,
                version TEXT,
                album_version TEXT
            );

            CREATE TABLE IF NOT EXISTS queue_history (
                sequence INTEGER PRIMARY KEY,
                position INTEGER NOT NULL,
                track_id INTEGER NOT NULL
            );

            INSERT OR IGNORE INTO player_state (id, current_position_secs, volume, shuffle_enabled, repeat_mode, was_playing, saved_at)
            VALUES (1, 0, 0.75, 0, 'off', 0, 0);
            ",
        )
        .map_err(|e| format!("Failed to create session tables: {}", e))?;

        let has_hires: bool = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('queue_tracks') WHERE name = 'hires'",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0)
            > 0;

        if !has_hires {
            let _ = conn.execute_batch(
                "
                ALTER TABLE queue_tracks ADD COLUMN hires INTEGER NOT NULL DEFAULT 0;
                ALTER TABLE queue_tracks ADD COLUMN bit_depth INTEGER;
                ALTER TABLE queue_tracks ADD COLUMN sample_rate REAL;
                ",
            );
        }

        let has_is_local: bool = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('queue_tracks') WHERE name = 'is_local'",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0)
            > 0;

        if !has_is_local {
            let _ = conn.execute_batch(
                "
                ALTER TABLE queue_tracks ADD COLUMN is_local INTEGER NOT NULL DEFAULT 0;
                ALTER TABLE queue_tracks ADD COLUMN album_id TEXT;
                ALTER TABLE queue_tracks ADD COLUMN artist_id INTEGER;
                ",
            );
        }

        let has_source: bool = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('queue_tracks') WHERE name = 'source'",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0)
            > 0;

        if !has_source {
            let _ = conn.execute_batch(
                "
                ALTER TABLE queue_tracks ADD COLUMN source TEXT;
                ",
            );
        }

        let has_streamable: bool = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('queue_tracks') WHERE name = 'streamable'",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0)
            > 0;

        if !has_streamable {
            let _ = conn.execute_batch(
                "
                ALTER TABLE queue_tracks ADD COLUMN streamable INTEGER NOT NULL DEFAULT 1;
                ALTER TABLE queue_tracks ADD COLUMN parental_warning INTEGER NOT NULL DEFAULT 0;
                ALTER TABLE queue_tracks ADD COLUMN source_item_id_hint TEXT;
                ",
            );
        }

        let has_last_view: bool = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('player_state') WHERE name = 'last_view'",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0)
            > 0;

        if !has_last_view {
            let _ = conn.execute_batch(
                "
                ALTER TABLE player_state ADD COLUMN last_view TEXT NOT NULL DEFAULT 'home';
                ALTER TABLE player_state ADD COLUMN view_context_id TEXT;
                ALTER TABLE player_state ADD COLUMN view_context_type TEXT;
                ",
            );
        }

        // ── One-shot: distrust every `streamable` written before the model
        //    became tri-state ────────────────────────────────────────────────
        //
        // `queue_tracks.streamable` is persisted from `QueueTrack.streamable`,
        // which was fed from `qbz_models::Track.streamable` — a plain `bool`
        // whose `#[serde(default)]` turned an ABSENT key into `false`. So a row
        // queued from any endpoint that omits the key was stored as `0` meaning
        // "the payload was terse", not "Qobuz pulled this track".
        //
        // The unavailability work gives `0` a new, destructive meaning: an
        // unavailable track never enters the queue. Left alone, those legacy
        // zeroes would silently delete tracks from every restored session, for
        // good — on a player whose entire point is not losing the user's music.
        //
        // The original value is unrecoverable (nothing on disk records WHY a
        // row is 0), so the only honest repair is to clear the poison once and
        // let the live API re-establish the truth on the next listing fetch,
        // with the reactive auto-skip as the backstop for anything genuinely
        // dead. Erring toward "available" here is the same asymmetry
        // `Track::is_streamable` documents: a wasted round trip is cheap, a
        // vanished track is not.
        //
        // It must be ONE-SHOT rather than a coercion on read, because after the
        // model change a `0` is meaningful again: a track Qobuz pulled but the
        // user already DOWNLOADED stays in the queue and plays from disk, and
        // it stays there with `streamable = 0`. Coercing on read would
        // resurrect that row as available forever and re-hide the very state
        // the render is meant to distinguish.
        //
        // `PRAGMA user_version` is the guard — it is 0 on every database that
        // predates this build and on a freshly created one, where the UPDATE
        // simply matches no rows. The column-probe idiom used by the migrations
        // above cannot express this: nothing is being added, only rewritten.
        let schema_version: i64 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap_or(0);
        if schema_version < 1 {
            let _ = conn.execute_batch(
                "
                UPDATE queue_tracks SET streamable = 1 WHERE streamable = 0;
                PRAGMA user_version = 1;
                ",
            );
        }

        Ok(Self { conn })
    }

    pub fn save_session(&self, session: &PersistedSessionSnapshot) -> Result<(), String> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        self.conn
            .execute("BEGIN TRANSACTION", [])
            .map_err(|e| format!("Failed to begin transaction: {}", e))?;

        if let Err(e) = self.conn.execute("DELETE FROM queue_tracks", []) {
            let _ = self.conn.execute("ROLLBACK", []);
            return Err(format!("Failed to clear queue: {}", e));
        }
        if let Err(e) = self.conn.execute("DELETE FROM queue_track_extras", []) {
            let _ = self.conn.execute("ROLLBACK", []);
            return Err(format!("Failed to clear queue track extras: {e}"));
        }
        if let Err(e) = self.conn.execute("DELETE FROM queue_history", []) {
            let _ = self.conn.execute("ROLLBACK", []);
            return Err(format!("Failed to clear queue history: {e}"));
        }

        for (pos, track) in session.playback.queue_tracks.iter().enumerate() {
            if let Err(e) = self.conn.execute(
                "INSERT INTO queue_tracks (position, track_id, title, artist, album, duration_secs, artwork_url, hires, bit_depth, sample_rate, is_local, album_id, artist_id, source, streamable, parental_warning, source_item_id_hint)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
                params![
                    pos as i64,
                    track.id as i64,
                    track.title,
                    track.artist,
                    track.album,
                    track.duration_secs as i64,
                    track.artwork_url,
                    track.hires as i64,
                    track.bit_depth.map(|v| v as i64),
                    track.sample_rate,
                    track.is_local as i64,
                    track.album_id,
                    track.artist_id.map(|v| v as i64),
                    track.source,
                    track.streamable as i64,
                    track.parental_warning as i64,
                    track.source_item_id_hint,
                ],
            ) {
                let _ = self.conn.execute("ROLLBACK", []);
                return Err(format!("Failed to insert queue track: {}", e));
            }
        }

        if let Err(e) = self.conn.execute(
            "UPDATE player_state SET
                current_index = ?1,
                current_position_secs = ?2,
                volume = ?3,
                shuffle_enabled = ?4,
                repeat_mode = ?5,
                was_playing = ?6,
                saved_at = ?7,
                last_view = ?8,
                view_context_id = ?9,
                view_context_type = ?10
             WHERE id = 1",
            params![
                session.playback.current_index.map(|i| i as i64),
                session.playback.current_position_secs as i64,
                session.playback.volume as f64,
                session.playback.shuffle_enabled as i64,
                session.playback.repeat_mode,
                session.playback.was_playing as i64,
                now,
                session.shell_view.last_view,
                session.shell_view.view_context_id,
                session.shell_view.view_context_type,
            ],
        ) {
            let _ = self.conn.execute("ROLLBACK", []);
            return Err(format!("Failed to update player state: {}", e));
        }

        self.conn
            .execute("COMMIT", [])
            .map_err(|e| format!("Failed to commit transaction: {}", e))?;

        Ok(())
    }

    pub fn load_session(&self) -> Result<PersistedSessionSnapshot, String> {
        let (
            current_index,
            current_position_secs,
            volume,
            shuffle_enabled,
            repeat_mode,
            was_playing,
            saved_at,
            last_view,
            view_context_id,
            view_context_type,
        ): (
            Option<i64>,
            i64,
            f64,
            i64,
            String,
            i64,
            i64,
            String,
            Option<String>,
            Option<String>,
        ) = self
            .conn
            .query_row(
                "SELECT current_index, current_position_secs, volume, shuffle_enabled, repeat_mode, was_playing, saved_at, last_view, view_context_id, view_context_type
                 FROM player_state WHERE id = 1",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                        row.get::<_, String>(7)
                            .unwrap_or_else(|_| "home".to_string()),
                        row.get(8)?,
                        row.get(9)?,
                    ))
                },
            )
            .map_err(|e| format!("Failed to load player state: {}", e))?;

        let mut stmt = self.conn
            .prepare("SELECT track_id, title, artist, album, duration_secs, artwork_url, hires, bit_depth, sample_rate, is_local, album_id, artist_id, source, streamable, parental_warning, source_item_id_hint FROM queue_tracks ORDER BY position")
            .map_err(|e| format!("Failed to prepare queue query: {}", e))?;

        let tracks: Vec<PersistedQueueTrack> = stmt
            .query_map([], |row| {
                Ok(PersistedQueueTrack {
                    id: row.get::<_, i64>(0)? as u64,
                    title: row.get(1)?,
                    artist: row.get(2)?,
                    album: row.get(3)?,
                    duration_secs: row.get::<_, i64>(4)? as u64,
                    artwork_url: row.get(5)?,
                    hires: row.get::<_, i64>(6).unwrap_or(0) != 0,
                    bit_depth: row.get::<_, Option<i64>>(7)?.map(|v| v as u32),
                    sample_rate: row.get(8)?,
                    is_local: row.get::<_, i64>(9).unwrap_or(0) != 0,
                    album_id: row.get(10)?,
                    artist_id: row.get::<_, Option<i64>>(11)?.map(|v| v as u64),
                    source: row.get(12)?,
                    streamable: row.get::<_, i64>(13).unwrap_or(1) != 0,
                    parental_warning: row.get::<_, i64>(14).unwrap_or(0) != 0,
                    source_item_id_hint: row.get(15)?,
                })
            })
            .map_err(|e| format!("Failed to query queue tracks: {}", e))?
            .filter_map(|result| result.ok())
            .collect();

        Ok(PersistedSessionSnapshot {
            playback: PersistedPlaybackSession {
                queue_tracks: tracks,
                current_index: current_index.map(|i| i as usize),
                current_position_secs: current_position_secs as u64,
                volume: volume as f32,
                shuffle_enabled: shuffle_enabled != 0,
                repeat_mode,
                was_playing: was_playing != 0,
                saved_at,
            },
            shell_view: PersistedShellViewState {
                last_view,
                view_context_id,
                view_context_type,
            },
        })
    }

    /// Persist edition subtitles and oldest-first playback history alongside
    /// the portable session snapshot. Kept additive so older frontends that
    /// construct `PersistedPlaybackSession` remain source-compatible.
    pub fn save_queue_extras(&self, extras: &PersistedQueueExtras) -> Result<(), String> {
        self.conn
            .execute("BEGIN TRANSACTION", [])
            .map_err(|e| format!("Failed to begin queue-extras transaction: {e}"))?;

        let save = (|| -> Result<(), String> {
            self.conn
                .execute("DELETE FROM queue_track_extras", [])
                .map_err(|e| format!("Failed to clear queue track extras: {e}"))?;
            self.conn
                .execute("DELETE FROM queue_history", [])
                .map_err(|e| format!("Failed to clear queue history: {e}"))?;

            for edition in &extras.editions {
                self.conn
                    .execute(
                        "INSERT INTO queue_track_extras (position, track_id, version, album_version) VALUES (?1, ?2, ?3, ?4)",
                        params![
                            edition.position as i64,
                            edition.track_id as i64,
                            edition.version.as_deref(),
                            edition.album_version.as_deref(),
                        ],
                    )
                    .map_err(|e| format!("Failed to save queue track extra: {e}"))?;
            }

            for (sequence, entry) in extras.history.iter().enumerate() {
                self.conn
                    .execute(
                        "INSERT INTO queue_history (sequence, position, track_id) VALUES (?1, ?2, ?3)",
                        params![sequence as i64, entry.position as i64, entry.track_id as i64],
                    )
                    .map_err(|e| format!("Failed to save queue history: {e}"))?;
            }
            Ok(())
        })();

        if let Err(error) = save {
            let _ = self.conn.execute("ROLLBACK", []);
            return Err(error);
        }
        self.conn
            .execute("COMMIT", [])
            .map_err(|e| format!("Failed to commit queue-extras transaction: {e}"))?;
        Ok(())
    }

    pub fn load_queue_extras(&self) -> Result<PersistedQueueExtras, String> {
        let mut editions_stmt = self
            .conn
            .prepare(
                "SELECT position, track_id, version, album_version FROM queue_track_extras ORDER BY position",
            )
            .map_err(|e| format!("Failed to prepare queue track extras query: {e}"))?;
        let editions = editions_stmt
            .query_map([], |row| {
                Ok(PersistedQueueTrackEdition {
                    position: row.get::<_, i64>(0)? as usize,
                    track_id: row.get::<_, i64>(1)? as u64,
                    version: row.get(2)?,
                    album_version: row.get(3)?,
                })
            })
            .map_err(|e| format!("Failed to query queue track extras: {e}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to read queue track extras: {e}"))?;

        let mut history_stmt = self
            .conn
            .prepare("SELECT position, track_id FROM queue_history ORDER BY sequence")
            .map_err(|e| format!("Failed to prepare queue history query: {e}"))?;
        let history = history_stmt
            .query_map([], |row| {
                Ok(PersistedQueueHistoryEntry {
                    position: row.get::<_, i64>(0)? as usize,
                    track_id: row.get::<_, i64>(1)? as u64,
                })
            })
            .map_err(|e| format!("Failed to query queue history: {e}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to read queue history: {e}"))?;

        Ok(PersistedQueueExtras { editions, history })
    }

    pub fn save_position(&self, position_secs: u64) -> Result<(), String> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        self.conn
            .execute(
                "UPDATE player_state SET current_position_secs = ?1, saved_at = ?2 WHERE id = 1",
                params![position_secs as i64, now],
            )
            .map_err(|e| format!("Failed to save position: {}", e))?;

        Ok(())
    }

    pub fn save_volume(&self, volume: f32) -> Result<(), String> {
        self.conn
            .execute(
                "UPDATE player_state SET volume = ?1 WHERE id = 1",
                params![volume as f64],
            )
            .map_err(|e| format!("Failed to save volume: {}", e))?;

        Ok(())
    }

    pub fn save_playback_mode(&self, shuffle: bool, repeat_mode: &str) -> Result<(), String> {
        self.conn
            .execute(
                "UPDATE player_state SET shuffle_enabled = ?1, repeat_mode = ?2 WHERE id = 1",
                params![shuffle as i64, repeat_mode],
            )
            .map_err(|e| format!("Failed to save playback mode: {}", e))?;

        Ok(())
    }

    pub fn clear_session(&self) -> Result<(), String> {
        self.conn
            .execute("DELETE FROM queue_tracks", [])
            .map_err(|e| format!("Failed to clear queue: {}", e))?;
        self.conn
            .execute("DELETE FROM queue_track_extras", [])
            .map_err(|e| format!("Failed to clear queue track extras: {e}"))?;
        self.conn
            .execute("DELETE FROM queue_history", [])
            .map_err(|e| format!("Failed to clear queue history: {e}"))?;

        self.conn.execute(
            "UPDATE player_state SET current_index = NULL, current_position_secs = 0, was_playing = 0, last_view = 'home', view_context_id = NULL, view_context_type = NULL WHERE id = 1",
            [],
        ).map_err(|e| format!("Failed to reset player state: {}", e))?;

        Ok(())
    }

    #[cfg(test)]
    fn pragma_synchronous(&self) -> Result<i64, String> {
        self.conn
            .query_row("PRAGMA synchronous", [], |row| row.get(0))
            .map_err(|e| format!("Failed to read synchronous pragma: {}", e))
    }

    #[cfg(test)]
    fn pragma_journal_mode(&self) -> Result<String, String> {
        self.conn
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .map_err(|e| format!("Failed to read journal mode pragma: {}", e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unique_test_dir(name: &str) -> std::path::PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("qbz-app-{name}-{}-{nonce}", std::process::id()))
    }

    fn sample_track() -> PersistedQueueTrack {
        PersistedQueueTrack {
            id: 42,
            title: "Track".to_string(),
            artist: "Artist".to_string(),
            album: "Album".to_string(),
            duration_secs: 300,
            artwork_url: Some("https://example.test/art.jpg".to_string()),
            hires: true,
            bit_depth: Some(24),
            sample_rate: Some(96_000.0),
            is_local: true,
            album_id: Some("album-1".to_string()),
            artist_id: Some(7),
            streamable: false,
            source: Some("mixtape".to_string()),
            parental_warning: true,
            source_item_id_hint: Some("item-1".to_string()),
        }
    }

    #[test]
    fn default_session_values_are_stable() {
        let session = PersistedSessionSnapshot::default();

        assert!(session.playback.queue_tracks.is_empty());
        assert_eq!(session.playback.current_index, None);
        assert_eq!(session.playback.current_position_secs, 0);
        assert_eq!(session.playback.volume, 0.75);
        assert!(!session.playback.shuffle_enabled);
        assert_eq!(session.playback.repeat_mode, "off");
        assert!(!session.playback.was_playing);
        assert_eq!(session.shell_view.last_view, "home");
        assert_eq!(session.shell_view.view_context_id, None);
        assert_eq!(session.shell_view.view_context_type, None);
    }

    #[test]
    fn session_store_uses_wal_and_full_synchronous() {
        let dir = unique_test_dir("session-pragmas");
        let store = SessionStore::new_at(&dir).expect("open store");

        assert_eq!(store.pragma_journal_mode().expect("journal mode"), "wal");
        assert_eq!(store.pragma_synchronous().expect("synchronous"), 2);

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn session_store_round_trips_queue_and_shell_view_state() {
        let dir = unique_test_dir("session-round-trip");
        let store = SessionStore::new_at(&dir).expect("open store");
        let session = PersistedSessionSnapshot {
            playback: PersistedPlaybackSession {
                queue_tracks: vec![sample_track()],
                current_index: Some(0),
                current_position_secs: 123,
                volume: 0.42,
                shuffle_enabled: true,
                repeat_mode: "all".to_string(),
                was_playing: true,
                saved_at: 0,
            },
            shell_view: PersistedShellViewState {
                last_view: "album".to_string(),
                view_context_id: Some("album-1".to_string()),
                view_context_type: Some("album".to_string()),
            },
        };

        store.save_session(&session).expect("save session");
        let loaded = store.load_session().expect("load session");

        assert_eq!(loaded.playback.queue_tracks, vec![sample_track()]);
        assert_eq!(loaded.playback.current_index, Some(0));
        assert_eq!(loaded.playback.current_position_secs, 123);
        assert_eq!(loaded.playback.volume, 0.42);
        assert!(loaded.playback.shuffle_enabled);
        assert_eq!(loaded.playback.repeat_mode, "all");
        assert!(loaded.playback.was_playing);
        assert!(loaded.playback.saved_at > 0);
        assert_eq!(loaded.shell_view.last_view, "album");
        assert_eq!(
            loaded.shell_view.view_context_id.as_deref(),
            Some("album-1")
        );
        assert_eq!(
            loaded.shell_view.view_context_type.as_deref(),
            Some("album")
        );

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn session_store_round_trips_queue_editions_and_history() {
        let dir = unique_test_dir("session-queue-extras");
        let store = SessionStore::new_at(&dir).expect("open store");
        let session = PersistedSessionSnapshot {
            playback: PersistedPlaybackSession {
                queue_tracks: vec![sample_track()],
                current_index: Some(0),
                ..PersistedPlaybackSession::default()
            },
            shell_view: PersistedShellViewState::default(),
        };
        let extras = PersistedQueueExtras {
            editions: vec![PersistedQueueTrackEdition {
                position: 0,
                track_id: 42,
                version: Some("Backing Track / Bonus Track".to_string()),
                album_version: Some("Remastered 2014".to_string()),
            }],
            history: vec![PersistedQueueHistoryEntry {
                position: 0,
                track_id: 42,
            }],
        };

        store.save_session(&session).expect("save session");
        store.save_queue_extras(&extras).expect("save queue extras");
        drop(store);

        let reopened = SessionStore::new_at(&dir).expect("reopen store");
        assert_eq!(
            reopened.load_queue_extras().expect("load queue extras"),
            extras
        );

        reopened.clear_session().expect("clear session");
        assert_eq!(
            reopened.load_queue_extras().expect("load cleared extras"),
            PersistedQueueExtras::default()
        );

        let _ = std::fs::remove_dir_all(dir);
    }

    /// The `user_version = 1` repair clears legacy `streamable = 0` rows ONCE,
    /// and never fires again.
    ///
    /// Both halves matter and they pull in opposite directions. A `0` written
    /// by an older build is untrustworthy — it may only mean the endpoint was
    /// terse — and under the new queue filter it would silently delete the
    /// track from every restored session. A `0` written AFTER the repair is
    /// real: it is a track Qobuz pulled that the user has downloaded, which
    /// stays in the queue and plays from disk. A coercion on read would satisfy
    /// the first half and destroy the second, which is why this is a migration.
    #[test]
    fn legacy_streamable_zeroes_are_cleared_once_then_honoured() {
        let dir = unique_test_dir("session-streamable-migration");

        let session_with_sample = || {
            let mut session = PersistedSessionSnapshot::default();
            session.playback.queue_tracks = vec![sample_track()];
            session.playback.current_index = Some(0);
            session
        };

        // A database as an older build left it: a row at `streamable = 0` and
        // no schema stamp. Rewinding the pragma is what makes it "older" —
        // `sample_track()` already carries `streamable: false`.
        {
            let store = SessionStore::new_at(&dir).expect("open store");
            store
                .save_session(&session_with_sample())
                .expect("save legacy session");
            store
                .conn
                .execute_batch("PRAGMA user_version = 0;")
                .expect("rewind schema stamp");
        }

        // Reopening runs the repair: the untrustworthy 0 is cleared.
        {
            let store = SessionStore::new_at(&dir).expect("reopen store");
            let loaded = store.load_session().expect("load repaired session");
            assert!(
                loaded.playback.queue_tracks[0].streamable,
                "a pre-migration 0 is untrustworthy and must be cleared, not \
                 read as 'Qobuz pulled this track'"
            );

            // Now write a 0 that IS trustworthy — the downloaded-but-pulled
            // state the render distinguishes.
            store
                .save_session(&session_with_sample())
                .expect("save post-migration session");
        }

        // Reopening again must leave it alone: the repair is one-shot.
        {
            let store = SessionStore::new_at(&dir).expect("reopen store again");
            let loaded = store.load_session().expect("load session");
            assert!(
                !loaded.playback.queue_tracks[0].streamable,
                "the repair must not fire twice — a post-migration 0 is real \
                 and resurrecting it would hide a downloaded-but-pulled track"
            );
        }

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn quick_saves_update_only_targeted_playback_fields() {
        let dir = unique_test_dir("session-quick-save");
        let store = SessionStore::new_at(&dir).expect("open store");

        store.save_position(77).expect("save position");
        store.save_volume(0.25).expect("save volume");
        store
            .save_playback_mode(true, "one")
            .expect("save playback mode");

        let loaded = store.load_session().expect("load session");

        assert_eq!(loaded.playback.current_position_secs, 77);
        assert_eq!(loaded.playback.volume, 0.25);
        assert!(loaded.playback.shuffle_enabled);
        assert_eq!(loaded.playback.repeat_mode, "one");
        assert_eq!(loaded.shell_view.last_view, "home");

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn clear_session_resets_playback_and_shell_view_fields() {
        let dir = unique_test_dir("session-clear");
        let store = SessionStore::new_at(&dir).expect("open store");
        let session = PersistedSessionSnapshot {
            playback: PersistedPlaybackSession {
                queue_tracks: vec![sample_track()],
                current_index: Some(0),
                current_position_secs: 55,
                volume: 0.9,
                shuffle_enabled: true,
                repeat_mode: "all".to_string(),
                was_playing: true,
                saved_at: 0,
            },
            shell_view: PersistedShellViewState {
                last_view: "artist".to_string(),
                view_context_id: Some("7".to_string()),
                view_context_type: Some("artist".to_string()),
            },
        };

        store.save_session(&session).expect("save session");
        store.clear_session().expect("clear session");
        let loaded = store.load_session().expect("load session");

        assert!(loaded.playback.queue_tracks.is_empty());
        assert_eq!(loaded.playback.current_index, None);
        assert_eq!(loaded.playback.current_position_secs, 0);
        assert_eq!(loaded.playback.volume, 0.9);
        assert!(loaded.playback.shuffle_enabled);
        assert_eq!(loaded.playback.repeat_mode, "all");
        assert!(!loaded.playback.was_playing);
        assert_eq!(loaded.shell_view.last_view, "home");
        assert_eq!(loaded.shell_view.view_context_id, None);
        assert_eq!(loaded.shell_view.view_context_type, None);

        let _ = std::fs::remove_dir_all(dir);
    }
}
