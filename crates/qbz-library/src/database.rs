//! SQLite database layer for library persistence

use rusqlite::{params, Connection, OptionalExtension};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use crate::{
    reachability::{probe_default, Reach},
    AudioFormat, FolderTreeEntry, LibraryError, LocalAlbum, LocalArtist, LocalTrack,
};

#[derive(Debug, Clone)]
pub struct AlbumTrackUpdate {
    pub id: i64,
    pub title: String,
    pub disc_number: Option<u32>,
    pub track_number: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct TrackMetadataUpdateFull {
    pub id: i64,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub album_artist: Option<String>,
    pub album_group_title: String,
    pub track_number: Option<u32>,
    pub disc_number: Option<u32>,
    pub year: Option<u32>,
    pub genre: Option<String>,
    pub catalog_number: Option<String>,
}

/// Library database wrapper
pub struct LibraryDatabase {
    conn: Connection,
}

/// Qt creates several library-backed singletons and starts the folder scanner
/// during the same startup turn. Each owns a separate SQLite connection, but
/// schema discovery plus `ALTER TABLE` is a check-then-act sequence and must
/// not run concurrently inside this process.
static SCHEMA_INIT_LOCK: Mutex<()> = Mutex::new(());

/// Scope reconciliation walks every indexed SACD against every registered
/// root. One pass per database and process is enough: runtime folder changes
/// update ownership directly, while the next process start repairs legacy or
/// externally modified state again.
static SACD_SCOPE_RECONCILED: LazyLock<Mutex<HashSet<PathBuf>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

/// A SQL predicate restricting the shared remote mirror to the sources the user
/// has enabled.
///
/// The words are VALIDATED, not escaped: only `[a-z]` survives, and anything
/// else drops the entry. They arrive from `SourceId::as_str`, so they are
/// already a closed set — but this string is interpolated into SQL, and a
/// validator that cannot be bypassed beats an escape that can be forgotten.
/// An all-invalid list yields `0`, which shows nothing rather than everything.
fn remote_source_filter(sources: &[&str]) -> String {
    let clean: Vec<String> = sources
        .iter()
        .filter(|w| !w.is_empty() && w.chars().all(|c| c.is_ascii_lowercase()))
        .map(|w| format!("'{w}'"))
        .collect();
    if clean.is_empty() {
        "0".to_string()
    } else {
        format!("source IN ({})", clean.join(", "))
    }
}

impl LibraryDatabase {
    /// Open or create database at path
    pub fn open(db_path: &Path) -> Result<Self, LibraryError> {
        log::info!("Opening library database");

        let conn = Connection::open(db_path)
            .map_err(|e| LibraryError::Database(format!("Failed to open database: {}", e)))?;
        conn.busy_timeout(Duration::from_millis(2_500))
            .map_err(|e| LibraryError::Database(format!("Failed to set busy timeout: {}", e)))?;

        let schema_guard = SCHEMA_INIT_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        // Enable WAL mode for better concurrent access
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")
            .map_err(|e| LibraryError::Database(format!("Failed to set WAL mode: {}", e)))?;

        let db = Self { conn };
        db.init_schema()?;
        db.run_migrations()?;
        let needs_sacd_scope_reconcile = SACD_SCOPE_RECONCILED
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(db_path)
            .is_none();
        if needs_sacd_scope_reconcile {
            db.reconcile_sacd_catalog_scope()?;
            SACD_SCOPE_RECONCILED
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .insert(db_path.to_path_buf());
        }
        // First-class LOCAL playlists (offline-mode D7) — separate module,
        // same database file. Idempotent CREATE IF NOT EXISTS.
        crate::local_playlists::init_schema(&db.conn)
            .map_err(|e| LibraryError::Database(format!("local_playlists schema: {}", e)))?;
        // Qobuz playlist snapshot (offline-mode B7/B8) — names + membership
        // captured opportunistically while online. Idempotent.
        crate::qobuz_playlist_snapshot::init_schema(&db.conn).map_err(|e| {
            LibraryError::Database(format!("qobuz_playlist_snapshot schema: {}", e))
        })?;
        drop(schema_guard);
        Ok(db)
    }

    /// Create tables if they don't exist
    fn init_schema(&self) -> Result<(), LibraryError> {
        self.conn
            .execute_batch(
                r#"
            CREATE TABLE IF NOT EXISTS library_folders (
                id INTEGER PRIMARY KEY,
                path TEXT UNIQUE NOT NULL,
                enabled INTEGER DEFAULT 1,
                last_scan INTEGER
            );

            CREATE TABLE IF NOT EXISTS local_tracks (
                id INTEGER PRIMARY KEY,
                file_path TEXT NOT NULL,
                title TEXT NOT NULL,
                artist TEXT NOT NULL,
                album TEXT NOT NULL,
                album_artist TEXT,
                track_number INTEGER,
                disc_number INTEGER,
                year INTEGER,
                genre TEXT,
                genres_json TEXT NOT NULL DEFAULT '[]',
                duration_secs INTEGER NOT NULL,
                format TEXT NOT NULL,
                bit_depth INTEGER,
                sample_rate REAL NOT NULL,
                channels INTEGER NOT NULL,
                file_size_bytes INTEGER NOT NULL,
                cue_file_path TEXT,
                cue_start_secs REAL,
                cue_end_secs REAL,
                artwork_path TEXT,
                last_modified INTEGER NOT NULL,
                indexed_at INTEGER NOT NULL,
                album_group_key TEXT,
                album_group_title TEXT,
                is_network_mount INTEGER NOT NULL DEFAULT 0,
                UNIQUE(file_path, cue_start_secs)
            );

            CREATE INDEX IF NOT EXISTS idx_tracks_artist ON local_tracks(artist);
            CREATE INDEX IF NOT EXISTS idx_tracks_album ON local_tracks(album);
            CREATE INDEX IF NOT EXISTS idx_tracks_album_artist ON local_tracks(album_artist);
            CREATE INDEX IF NOT EXISTS idx_tracks_file_path ON local_tracks(file_path);
            CREATE INDEX IF NOT EXISTS idx_tracks_title ON local_tracks(title);
            CREATE INDEX IF NOT EXISTS idx_local_tracks_album_lookup
                ON local_tracks(album, album_artist, artist);

            -- A SACD image is one physical file but exposes several virtual
            -- `sacd:/path/image.iso#N` local tracks. Keep that relationship
            -- outside `local_tracks`: the latter remains the authoritative
            -- playback row, while the disc fingerprint lets a successful
            -- re-import update those rows in place after the image moves.
            CREATE TABLE IF NOT EXISTS local_sacd_images (
                fingerprint TEXT PRIMARY KEY,
                image_path TEXT NOT NULL UNIQUE,
                image_size_bytes INTEGER NOT NULL,
                image_modified_ns INTEGER NOT NULL,
                observed_at INTEGER NOT NULL,
                parser_revision INTEGER NOT NULL DEFAULT 0
            );

            CREATE TABLE IF NOT EXISTS local_sacd_tracks (
                fingerprint TEXT NOT NULL,
                track_number INTEGER NOT NULL CHECK(track_number BETWEEN 1 AND 255),
                local_track_id INTEGER NOT NULL UNIQUE,
                PRIMARY KEY(fingerprint, track_number)
            );

            CREATE INDEX IF NOT EXISTS idx_local_sacd_tracks_local_id
                ON local_sacd_tracks(local_track_id);

            -- A registered Local Library root owns an indexed SACD. Keeping
            -- this separate from the image allows overlapping roots without
            -- making a manually opened image persistent.
            CREATE TABLE IF NOT EXISTS local_sacd_roots (
                root_id INTEGER NOT NULL,
                fingerprint TEXT NOT NULL,
                observed_generation INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY(root_id, fingerprint)
            );

            CREATE INDEX IF NOT EXISTS idx_local_sacd_roots_fingerprint
                ON local_sacd_roots(fingerprint);

            -- Incremental scanner state. `local_tracks` remains authoritative;
            -- these tables only remember what each root observed and whether
            -- a completed generation is allowed to prune stale rows.
            CREATE TABLE IF NOT EXISTS local_scan_roots (
                root_id INTEGER PRIMARY KEY,
                generation INTEGER NOT NULL DEFAULT 0,
                phase TEXT NOT NULL DEFAULT 'idle',
                checkpoint_path TEXT NOT NULL DEFAULT '',
                discovered INTEGER NOT NULL DEFAULT 0,
                processed INTEGER NOT NULL DEFAULT 0,
                status TEXT NOT NULL DEFAULT 'idle',
                prune_authorized INTEGER NOT NULL DEFAULT 0,
                updated_at INTEGER NOT NULL DEFAULT 0
            );

            CREATE TABLE IF NOT EXISTS local_scan_files (
                root_id INTEGER NOT NULL,
                file_path TEXT NOT NULL,
                file_kind TEXT NOT NULL CHECK (file_kind IN ('audio','cue')),
                file_id TEXT NOT NULL,
                size_bytes INTEGER NOT NULL,
                mtime_ns INTEGER NOT NULL,
                dependency_fingerprint TEXT NOT NULL DEFAULT '',
                cue_audio_path TEXT,
                extraction_ok INTEGER NOT NULL DEFAULT 1,
                observed_generation INTEGER NOT NULL,
                PRIMARY KEY (root_id, file_path, file_kind)
            );

            CREATE INDEX IF NOT EXISTS idx_local_scan_files_generation
                ON local_scan_files(root_id, observed_generation, file_kind, file_path);

            -- CUE references are generation-scoped and disk-backed so a pass
            -- never needs a HashSet containing every referenced audio path.
            CREATE TABLE IF NOT EXISTS local_scan_cue_refs (
                root_id INTEGER NOT NULL,
                generation INTEGER NOT NULL,
                audio_path TEXT NOT NULL,
                PRIMARY KEY (root_id, generation, audio_path)
            );

            -- Playlist folders (local organization for Qobuz playlists)
            CREATE TABLE IF NOT EXISTS playlist_folders (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                icon_type TEXT DEFAULT 'preset',
                icon_preset TEXT DEFAULT 'folder',
                icon_color TEXT DEFAULT '#6366f1',
                custom_image_path TEXT,
                is_hidden INTEGER DEFAULT 0,
                position INTEGER DEFAULT 0,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_playlist_folders_position ON playlist_folders(position);
            CREATE INDEX IF NOT EXISTS idx_playlist_folders_hidden ON playlist_folders(is_hidden);

            -- Playlist local settings (enhances remote Qobuz playlists)
            -- Note: For existing databases, folder_id is added via migration
            CREATE TABLE IF NOT EXISTS playlist_settings (
                qobuz_playlist_id INTEGER PRIMARY KEY,
                custom_artwork_path TEXT,
                sort_by TEXT DEFAULT 'default',
                sort_order TEXT DEFAULT 'asc',
                last_search_query TEXT,
                notes TEXT,
                hidden INTEGER DEFAULT 0,
                position INTEGER DEFAULT 0,
                folder_id TEXT REFERENCES playlist_folders(id) ON DELETE SET NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            );

            -- Note: idx_playlist_settings_folder is created conditionally after migrations run

            -- Playlist statistics (play counts, etc.)
            CREATE TABLE IF NOT EXISTS playlist_stats (
                qobuz_playlist_id INTEGER PRIMARY KEY,
                play_count INTEGER DEFAULT 0,
                last_played_at INTEGER,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            );

            -- Qobuz playlists the user has COPIED into their library (mirrors
            -- Tauri's user-scoped `qbz_copied_playlists`): stores the SOURCE
            -- playlist id so its detail view hides the Copy button on reopen.
            CREATE TABLE IF NOT EXISTS copied_playlists (
                qobuz_playlist_id INTEGER PRIMARY KEY,
                copied_at INTEGER NOT NULL
            );

            -- Local tracks added to playlists (mixed with remote Qobuz tracks)
            CREATE TABLE IF NOT EXISTS playlist_local_tracks (
                id INTEGER PRIMARY KEY,
                qobuz_playlist_id INTEGER NOT NULL,
                local_track_id INTEGER NOT NULL,
                position INTEGER NOT NULL,
                added_at INTEGER NOT NULL,
                FOREIGN KEY (local_track_id) REFERENCES local_tracks(id) ON DELETE CASCADE,
                UNIQUE(qobuz_playlist_id, local_track_id)
            );

            CREATE INDEX IF NOT EXISTS idx_playlist_local_tracks_playlist
                ON playlist_local_tracks(qobuz_playlist_id);

            -- The Add-to-Playlist picker asks the inverse question — "which
            -- playlists hold this track" (playlist_membership.rs).
            CREATE INDEX IF NOT EXISTS idx_playlist_local_tracks_track
                ON playlist_local_tracks(local_track_id, qobuz_playlist_id);

            -- Plex tracks added to playlists. Kept in its own table because
            -- Plex tracks live on a remote server and have a TEXT rating key,
            -- not the i64 filesystem id used by local_tracks. No foreign key
            -- to plex_cache_tracks: that cache can be purged without losing
            -- the user's intent (the rows gray out in the UI until Plex is
            -- reachable again).
            CREATE TABLE IF NOT EXISTS playlist_plex_tracks (
                id INTEGER PRIMARY KEY,
                qobuz_playlist_id INTEGER NOT NULL,
                plex_rating_key TEXT NOT NULL,
                position INTEGER NOT NULL,
                added_at INTEGER NOT NULL,
                UNIQUE(qobuz_playlist_id, plex_rating_key)
            );

            CREATE INDEX IF NOT EXISTS idx_playlist_plex_tracks_playlist
                ON playlist_plex_tracks(qobuz_playlist_id);

            CREATE INDEX IF NOT EXISTS idx_playlist_plex_tracks_key
                ON playlist_plex_tracks(plex_rating_key, qobuz_playlist_id);

            -- Jellyfin/Subsonic tracks added to playlists (2026-08-30). Their
            -- library rows are media-cache projections with no local_tracks
            -- rowid, so they get their own sidecar keyed by the server item
            -- id — the plex_rating_key pattern, source-qualified because two
            -- protocols share the table.
            CREATE TABLE IF NOT EXISTS playlist_remote_tracks (
                id INTEGER PRIMARY KEY,
                qobuz_playlist_id INTEGER NOT NULL,
                source TEXT NOT NULL,
                item_id TEXT NOT NULL,
                position INTEGER NOT NULL,
                added_at INTEGER NOT NULL,
                UNIQUE(qobuz_playlist_id, source, item_id)
            );

            CREATE INDEX IF NOT EXISTS idx_playlist_remote_tracks_playlist
                ON playlist_remote_tracks(qobuz_playlist_id);

            CREATE INDEX IF NOT EXISTS idx_playlist_remote_tracks_item
                ON playlist_remote_tracks(source, item_id, qobuz_playlist_id);

            -- Custom track order per playlist (user-defined arrangement)
            CREATE TABLE IF NOT EXISTS playlist_track_custom_order (
                id INTEGER PRIMARY KEY,
                qobuz_playlist_id INTEGER NOT NULL,
                track_id INTEGER NOT NULL,
                is_local INTEGER DEFAULT 0,
                custom_position INTEGER NOT NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                UNIQUE(qobuz_playlist_id, track_id, is_local)
            );

            CREATE INDEX IF NOT EXISTS idx_playlist_custom_order_playlist
                ON playlist_track_custom_order(qobuz_playlist_id);
            CREATE INDEX IF NOT EXISTS idx_playlist_custom_order_position
                ON playlist_track_custom_order(qobuz_playlist_id, custom_position);

            -- Album settings (per-album customization)
            CREATE TABLE IF NOT EXISTS album_settings (
                album_group_key TEXT PRIMARY KEY,
                hidden INTEGER DEFAULT 0,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            );

            -- Artist images cache (Qobuz/Discogs images and custom uploads)
            CREATE TABLE IF NOT EXISTS artist_images (
                artist_name TEXT PRIMARY KEY,
                image_url TEXT,
                source TEXT NOT NULL,
                custom_image_path TEXT,
                fetched_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_artist_images_fetched ON artist_images(fetched_at);

            -- Custom album covers (user-uploaded covers for Qobuz albums)
            CREATE TABLE IF NOT EXISTS custom_album_covers (
                album_id TEXT PRIMARY KEY,
                custom_image_path TEXT NOT NULL,
                created_at INTEGER NOT NULL
            );

            -- Downloaded purchases registry (permanent — user owns these files)
            CREATE TABLE IF NOT EXISTS downloaded_purchases (
                track_id INTEGER NOT NULL,
                format_id INTEGER NOT NULL DEFAULT 0,
                album_id TEXT,
                file_path TEXT NOT NULL,
                downloaded_at TEXT NOT NULL DEFAULT (datetime('now')),
                PRIMARY KEY (track_id, format_id)
            );

            CREATE INDEX IF NOT EXISTS idx_downloaded_purchases_album
                ON downloaded_purchases(album_id);

            CREATE TABLE IF NOT EXISTS library_kv (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
        "#,
            )
            .map_err(|e| LibraryError::Database(format!("Failed to create schema: {}", e)))?;

        crate::purchase_copies::init_schema(&self.conn)?;

        Ok(())
    }

    /// Read a small key-value setting (frontend-agnostic; e.g. the tag-editor
    /// direct-write acknowledgement flag). Returns None when the key is absent.
    pub fn get_kv(&self, key: &str) -> Result<Option<String>, LibraryError> {
        self.conn
            .query_row(
                "SELECT value FROM library_kv WHERE key = ?",
                params![key],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|e| LibraryError::Database(e.to_string()))
    }

    /// Write a small key-value setting (upsert).
    pub fn set_kv(&self, key: &str, value: &str) -> Result<(), LibraryError> {
        self.conn
            .execute(
                "INSERT INTO library_kv (key, value) VALUES (?, ?)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![key, value],
            )
            .map_err(|e| LibraryError::Database(e.to_string()))?;
        Ok(())
    }

    /// Run schema migrations for existing databases
    fn run_migrations(&self) -> Result<(), LibraryError> {
        let has_sacd_parser_revision: bool = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('local_sacd_images') WHERE name = 'parser_revision'",
                [],
                |row| row.get::<_, i32>(0),
            )
            .map(|count| count > 0)
            .unwrap_or(false);
        if !has_sacd_parser_revision {
            log::info!("Running migration: adding parser_revision to local_sacd_images");
            if let Err(error) = self.conn.execute_batch(
                "ALTER TABLE local_sacd_images ADD COLUMN parser_revision INTEGER NOT NULL DEFAULT 0;",
            ) {
                // The in-process lock handles QBZ's startup fan-out. This
                // second check also covers two QBZ processes migrating the
                // same profile: accept the losing ALTER only if the intended
                // schema is now observable, never by matching error text.
                let installed_by_peer = self
                    .conn
                    .query_row(
                        "SELECT COUNT(*) FROM pragma_table_info('local_sacd_images') WHERE name = 'parser_revision'",
                        [],
                        |row| row.get::<_, i32>(0),
                    )
                    .map(|count| count > 0)
                    .unwrap_or(false);
                if !installed_by_peer {
                    return Err(LibraryError::Database(format!(
                        "SACD parser revision migration failed: {error}"
                    )));
                }
                log::debug!("SACD parser revision migration completed by another process");
            }
        }

        // Migration: Add qobuz download tracking fields
        let has_source: bool = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('local_tracks') WHERE name = 'source'",
                [],
                |row| row.get::<_, i32>(0),
            )
            .map(|count| count > 0)
            .unwrap_or(false);

        if !has_source {
            log::info!("Running migration: adding source and qobuz_track_id to local_tracks");
            self.conn
                .execute_batch(
                    "ALTER TABLE local_tracks ADD COLUMN source TEXT DEFAULT 'user';
                 ALTER TABLE local_tracks ADD COLUMN qobuz_track_id INTEGER;
                 CREATE INDEX IF NOT EXISTS idx_tracks_source ON local_tracks(source);
                 CREATE INDEX IF NOT EXISTS idx_tracks_qobuz_id ON local_tracks(qobuz_track_id);",
                )
                .map_err(|e| LibraryError::Database(format!("Migration failed: {}", e)))?;
        }

        // Check if playlist_settings has the 'hidden' column (added in v2)
        let has_hidden: bool = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('playlist_settings') WHERE name = 'hidden'",
                [],
                |row| row.get::<_, i32>(0),
            )
            .map(|count| count > 0)
            .unwrap_or(false);

        if !has_hidden {
            log::info!(
                "Running migration: adding hidden and position columns to playlist_settings"
            );
            self.conn
                .execute_batch(
                    "ALTER TABLE playlist_settings ADD COLUMN hidden INTEGER DEFAULT 0;
                 ALTER TABLE playlist_settings ADD COLUMN position INTEGER DEFAULT 0;",
                )
                .map_err(|e| LibraryError::Database(format!("Migration failed: {}", e)))?;
        }

        // Check if playlist_stats table exists
        let has_stats_table: bool = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='playlist_stats'",
                [],
                |row| row.get::<_, i32>(0),
            )
            .map(|count| count > 0)
            .unwrap_or(false);

        if !has_stats_table {
            log::info!("Running migration: creating playlist_stats table");
            self.conn
                .execute_batch(
                    "CREATE TABLE IF NOT EXISTS playlist_stats (
                    qobuz_playlist_id INTEGER PRIMARY KEY,
                    play_count INTEGER DEFAULT 0,
                    last_played_at INTEGER,
                    created_at INTEGER NOT NULL,
                    updated_at INTEGER NOT NULL
                );",
                )
                .map_err(|e| LibraryError::Database(format!("Migration failed: {}", e)))?;
        }

        let has_album_group_key: bool = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('local_tracks') WHERE name = 'album_group_key'",
                [],
                |row| row.get::<_, i32>(0),
            )
            .map(|count| count > 0)
            .unwrap_or(false);

        if !has_album_group_key {
            log::info!("Running migration: adding album_group_key to local_tracks");
            self.conn
                .execute_batch("ALTER TABLE local_tracks ADD COLUMN album_group_key TEXT;")
                .map_err(|e| LibraryError::Database(format!("Migration failed: {}", e)))?;
        }

        let has_album_group_title: bool = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('local_tracks') WHERE name = 'album_group_title'",
                [],
                |row| row.get::<_, i32>(0),
            )
            .map(|count| count > 0)
            .unwrap_or(false);

        if !has_album_group_title {
            log::info!("Running migration: adding album_group_title to local_tracks");
            self.conn
                .execute_batch("ALTER TABLE local_tracks ADD COLUMN album_group_title TEXT;")
                .map_err(|e| LibraryError::Database(format!("Migration failed: {}", e)))?;
        }

        self.conn
            .execute_batch(
                "CREATE INDEX IF NOT EXISTS idx_tracks_album_group ON local_tracks(album_group_key);",
            )
            .map_err(|e| LibraryError::Database(format!("Migration failed: {}", e)))?;

        // Migration: Add has_local_content column to playlist_settings
        let has_local_content: bool = self.conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('playlist_settings') WHERE name = 'has_local_content'",
                [],
                |row| row.get::<_, i32>(0),
            )
            .map(|count| count > 0)
            .unwrap_or(false);

        if !has_local_content {
            log::info!("Running migration: adding has_local_content column to playlist_settings");
            self.conn.execute_batch(
                "ALTER TABLE playlist_settings ADD COLUMN has_local_content TEXT DEFAULT 'unknown';
                 CREATE INDEX IF NOT EXISTS idx_playlist_local_content ON playlist_settings(has_local_content);"
            ).map_err(|e| LibraryError::Database(format!("Migration failed: {}", e)))?;
        }

        let has_file_nocue_index: bool = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name='idx_tracks_file_nocue'",
                [],
                |row| row.get::<_, i32>(0),
            )
            .map(|count| count > 0)
            .unwrap_or(false);

        if !has_file_nocue_index {
            log::warn!("Skipping deduplication migration to prevent data loss");
            log::info!("Creating unique index for non-CUE tracks (INSERT OR REPLACE will handle duplicates)");
            // CHANGED: Don't delete duplicates automatically - let INSERT OR REPLACE handle it
            // This prevents accidental data loss from aggressive deduplication
            self.conn
                .execute_batch(
                    r#"
                CREATE UNIQUE INDEX IF NOT EXISTS idx_tracks_file_nocue
                  ON local_tracks(file_path)
                  WHERE cue_file_path IS NULL;
            "#,
                )
                .map_err(|e| LibraryError::Database(format!("Migration failed: {}", e)))?;
        }

        // Migration: Add folder metadata columns (alias, network info)
        let has_folder_alias: bool = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('library_folders') WHERE name = 'alias'",
                [],
                |row| row.get::<_, i32>(0),
            )
            .map(|count| count > 0)
            .unwrap_or(false);

        if !has_folder_alias {
            log::info!("Running migration: adding folder metadata columns (alias, network info)");
            self.conn
                .execute_batch(
                    "ALTER TABLE library_folders ADD COLUMN alias TEXT;
                 ALTER TABLE library_folders ADD COLUMN is_network INTEGER DEFAULT 0;
                 ALTER TABLE library_folders ADD COLUMN network_fs_type TEXT;
                 ALTER TABLE library_folders ADD COLUMN user_override_network INTEGER DEFAULT 0;",
                )
                .map_err(|e| LibraryError::Database(format!("Migration failed: {}", e)))?;
        }

        // Migration: Add is_favorite column to playlist_settings
        let has_is_favorite: bool = self.conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('playlist_settings') WHERE name = 'is_favorite'",
                [],
                |row| row.get::<_, i32>(0),
            )
            .map(|count| count > 0)
            .unwrap_or(false);

        if !has_is_favorite {
            log::info!("Running migration: adding is_favorite column to playlist_settings");
            self.conn.execute_batch(
                "ALTER TABLE playlist_settings ADD COLUMN is_favorite INTEGER DEFAULT 0;
                 CREATE INDEX IF NOT EXISTS idx_playlist_favorite ON playlist_settings(is_favorite);"
            ).map_err(|e| LibraryError::Database(format!("Migration failed: {}", e)))?;
        }

        // Migration: Add playlist_folders table and folder_id column
        let has_playlist_folders: bool = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='playlist_folders'",
                [],
                |row| row.get::<_, i32>(0),
            )
            .map(|count| count > 0)
            .unwrap_or(false);

        if !has_playlist_folders {
            log::info!("Running migration: creating playlist_folders table");
            self.conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS playlist_folders (
                    id TEXT PRIMARY KEY,
                    name TEXT NOT NULL,
                    icon_type TEXT DEFAULT 'preset',
                    icon_preset TEXT DEFAULT 'folder',
                    icon_color TEXT DEFAULT '#6366f1',
                    custom_image_path TEXT,
                    is_hidden INTEGER DEFAULT 0,
                    position INTEGER DEFAULT 0,
                    created_at INTEGER NOT NULL,
                    updated_at INTEGER NOT NULL
                );
                CREATE INDEX IF NOT EXISTS idx_playlist_folders_position ON playlist_folders(position);
                CREATE INDEX IF NOT EXISTS idx_playlist_folders_hidden ON playlist_folders(is_hidden);"
            ).map_err(|e| LibraryError::Database(format!("Migration failed: {}", e)))?;
        }

        // Migration: Add folder_id column to playlist_settings
        let has_folder_id: bool = self.conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('playlist_settings') WHERE name = 'folder_id'",
                [],
                |row| row.get::<_, i32>(0),
            )
            .map(|count| count > 0)
            .unwrap_or(false);

        if !has_folder_id {
            log::info!("Running migration: adding folder_id column to playlist_settings");
            self.conn.execute_batch(
                "ALTER TABLE playlist_settings ADD COLUMN folder_id TEXT REFERENCES playlist_folders(id) ON DELETE SET NULL;
                 CREATE INDEX IF NOT EXISTS idx_playlist_settings_folder ON playlist_settings(folder_id);"
            ).map_err(|e| LibraryError::Database(format!("Migration failed: {}", e)))?;
        }

        // Migration: Add catalog_number column to local_tracks
        let has_catalog_number: bool = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('local_tracks') WHERE name = 'catalog_number'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .map(|count| count > 0)
            .unwrap_or(false);

        if !has_catalog_number {
            log::info!("Running migration: adding catalog_number to local_tracks");
            self.conn
                .execute_batch("ALTER TABLE local_tracks ADD COLUMN catalog_number TEXT;")
                .map_err(|e| LibraryError::Database(format!("Migration failed: {}", e)))?;
        }

        // Migration: cross-source identity tags (ISRC + MusicBrainz ids).
        // Additive; NULL until the next scan re-reads the file's tags.
        self.ensure_identity_columns()?;

        // Migration: Change sample_rate from INTEGER to REAL for decimal precision (44.1kHz, 88.2kHz, etc.)
        // Check if sample_rate is currently INTEGER
        let sample_rate_type: String = self
            .conn
            .query_row(
                "SELECT type FROM pragma_table_info('local_tracks') WHERE name = 'sample_rate'",
                [],
                |row| row.get(0),
            )
            .unwrap_or_else(|_| "REAL".to_string());

        if sample_rate_type == "INTEGER" {
            log::info!("Running migration: changing sample_rate from INTEGER to REAL for decimal precision");

            // SQLite doesn't support ALTER COLUMN type change, need to recreate table
            // CRITICAL: Explicitly list all columns to handle different DB versions safely
            self.conn
                .execute_batch(
                    r#"
                -- Clean up any leftover temp table from previous failed migration
                DROP TABLE IF EXISTS local_tracks_new;

                -- Create new table with REAL sample_rate (only core columns)
                CREATE TABLE local_tracks_new (
                    id INTEGER PRIMARY KEY,
                    file_path TEXT NOT NULL,
                    title TEXT NOT NULL,
                    artist TEXT NOT NULL,
                    album TEXT NOT NULL,
                    album_artist TEXT,
                    track_number INTEGER,
                    disc_number INTEGER,
                    year INTEGER,
                    genre TEXT,
                    duration_secs INTEGER NOT NULL,
                    format TEXT NOT NULL,
                    bit_depth INTEGER,
                    sample_rate REAL NOT NULL,
                    channels INTEGER NOT NULL,
                    file_size_bytes INTEGER NOT NULL,
                    cue_file_path TEXT,
                    cue_start_secs REAL,
                    cue_end_secs REAL,
                    artwork_path TEXT,
                    last_modified INTEGER NOT NULL,
                    indexed_at INTEGER NOT NULL,
                    UNIQUE(file_path, cue_start_secs)
                );

                -- Copy core columns explicitly (handles all DB versions)
                -- Use COALESCE to handle NULL values and provide safe defaults
                INSERT INTO local_tracks_new
                    (id, file_path, title, artist, album, album_artist, track_number,
                     disc_number, year, genre, duration_secs, format, bit_depth,
                     sample_rate, channels, file_size_bytes, cue_file_path,
                     cue_start_secs, cue_end_secs, artwork_path, last_modified, indexed_at)
                SELECT
                    id, file_path, title, artist, album,
                    album_artist, track_number, disc_number, year, genre,
                    duration_secs, format, bit_depth,
                    CAST(sample_rate AS REAL),
                    channels,
                    COALESCE(file_size_bytes, 0),
                    cue_file_path, cue_start_secs, cue_end_secs,
                    artwork_path, last_modified, indexed_at
                FROM local_tracks;

                -- Drop old table
                DROP TABLE local_tracks;

                -- Rename new table
                ALTER TABLE local_tracks_new RENAME TO local_tracks;

                -- Recreate core indexes
                CREATE INDEX IF NOT EXISTS idx_tracks_artist ON local_tracks(artist);
                CREATE INDEX IF NOT EXISTS idx_tracks_album ON local_tracks(album);
                CREATE INDEX IF NOT EXISTS idx_tracks_album_artist ON local_tracks(album_artist);
                CREATE INDEX IF NOT EXISTS idx_tracks_file_path ON local_tracks(file_path);
                CREATE INDEX IF NOT EXISTS idx_tracks_title ON local_tracks(title);
                CREATE UNIQUE INDEX IF NOT EXISTS idx_tracks_file_nocue
                    ON local_tracks(file_path)
                    WHERE cue_file_path IS NULL;
                "#,
                )
                .map_err(|e| {
                    LibraryError::Database(format!("sample_rate migration failed: {}", e))
                })?;

            // Add optional columns if they existed in old table
            // These were added in previous migrations, so they may or may not exist
            let has_album_group_key: bool = self.conn
                .query_row(
                    "SELECT COUNT(*) FROM pragma_table_info('local_tracks') WHERE name = 'album_group_key'",
                    [],
                    |row| row.get::<_, i32>(0),
                )
                .map(|count| count > 0)
                .unwrap_or(false);

            if !has_album_group_key {
                // Re-add album grouping columns (will be populated by next migration check)
                self.conn.execute_batch(
                    "ALTER TABLE local_tracks ADD COLUMN album_group_key TEXT;
                     ALTER TABLE local_tracks ADD COLUMN album_group_title TEXT;
                     CREATE INDEX IF NOT EXISTS idx_tracks_album_group ON local_tracks(album_group_key);"
                ).map_err(|e| LibraryError::Database(format!("Failed to re-add album_group columns: {}", e)))?;
            } else {
                // Columns existed, copy their data from old backup
                // Note: Old table is already dropped, so this branch means columns were preserved
                // This should not happen because we only copy core columns above
                // But keep this for safety
            }

            // Re-add source and qobuz_track_id columns if they don't exist
            let has_source: bool = self
                .conn
                .query_row(
                    "SELECT COUNT(*) FROM pragma_table_info('local_tracks') WHERE name = 'source'",
                    [],
                    |row| row.get::<_, i32>(0),
                )
                .map(|count| count > 0)
                .unwrap_or(false);

            if !has_source {
                self.conn.execute_batch(
                    "ALTER TABLE local_tracks ADD COLUMN source TEXT DEFAULT 'user';
                     ALTER TABLE local_tracks ADD COLUMN qobuz_track_id INTEGER;
                     CREATE INDEX IF NOT EXISTS idx_tracks_source ON local_tracks(source);
                     CREATE INDEX IF NOT EXISTS idx_tracks_qobuz_id ON local_tracks(qobuz_track_id);"
                ).map_err(|e| LibraryError::Database(format!("Failed to re-add source columns: {}", e)))?;
            }

            // Re-add catalog_number if it doesn't exist
            let has_catalog: bool = self.conn
                .query_row(
                    "SELECT COUNT(*) FROM pragma_table_info('local_tracks') WHERE name = 'catalog_number'",
                    [],
                    |row| row.get::<_, i32>(0),
                )
                .map(|count| count > 0)
                .unwrap_or(false);

            if !has_catalog {
                self.conn
                    .execute_batch("ALTER TABLE local_tracks ADD COLUMN catalog_number TEXT;")
                    .map_err(|e| {
                        LibraryError::Database(format!("Failed to re-add catalog_number: {}", e))
                    })?;
            }
            // Same for the identity columns (they post-date the rebuild too).
            self.ensure_identity_columns()?;

            log::info!("Migration completed: sample_rate is now REAL");
        }

        // Migration: Add is_network_mount flag to local_tracks. Default
        // 0; callers can re-scan folders to populate real values for
        // existing rows.
        let has_network_mount: bool = self.conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('local_tracks') WHERE name = 'is_network_mount'",
                [],
                |row| row.get::<_, i32>(0),
            )
            .map(|count| count > 0)
            .unwrap_or(false);

        if !has_network_mount {
            log::info!("Running migration: adding is_network_mount to local_tracks");
            self.conn
                .execute_batch(
                    "ALTER TABLE local_tracks ADD COLUMN is_network_mount INTEGER NOT NULL DEFAULT 0;",
                )
                .map_err(|e| LibraryError::Database(format!("Migration failed: {}", e)))?;
        }

        // Multi-genre metadata is additive: old rows keep their singular
        // `genre`, and readers use it whenever this JSON array is empty.
        let has_genres_json: bool = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('local_tracks') WHERE name = 'genres_json'",
                [],
                |row| row.get::<_, i32>(0),
            )
            .map(|count| count > 0)
            .unwrap_or(false);
        if !has_genres_json {
            log::info!("Running migration: adding genres_json to local_tracks");
            self.conn
                .execute_batch(
                    "ALTER TABLE local_tracks ADD COLUMN genres_json TEXT NOT NULL DEFAULT '[]';",
                )
                .map_err(|e| LibraryError::Database(format!("Migration failed: {}", e)))?;
        }

        // Migration: Add canonical_name column to artist_images for artist name normalization
        let has_canonical_name: bool = self.conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('artist_images') WHERE name = 'canonical_name'",
                [],
                |row| row.get::<_, i32>(0),
            )
            .map(|count| count > 0)
            .unwrap_or(false);

        if !has_canonical_name {
            log::info!("Running migration: adding canonical_name to artist_images");
            self.conn
                .execute_batch("ALTER TABLE artist_images ADD COLUMN canonical_name TEXT;")
                .map_err(|e| LibraryError::Database(format!("Migration failed: {}", e)))?;
        }

        // Create folder_id index after all migrations have run (ensures column exists)
        self.conn.execute_batch(
            "CREATE INDEX IF NOT EXISTS idx_playlist_settings_folder ON playlist_settings(folder_id);"
        ).map_err(|e| LibraryError::Database(format!("Failed to create folder index: {}", e)))?;

        // Migration: Create playlist_track_custom_order table for custom track arrangement
        let has_custom_order_table: bool = self.conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='playlist_track_custom_order'",
                [],
                |row| row.get::<_, i32>(0),
            )
            .map(|count| count > 0)
            .unwrap_or(false);

        if !has_custom_order_table {
            log::info!("Running migration: creating playlist_track_custom_order table");
            self.conn
                .execute_batch(
                    "CREATE TABLE IF NOT EXISTS playlist_track_custom_order (
                    id INTEGER PRIMARY KEY,
                    qobuz_playlist_id INTEGER NOT NULL,
                    track_id INTEGER NOT NULL,
                    is_local INTEGER DEFAULT 0,
                    custom_position INTEGER NOT NULL,
                    created_at INTEGER NOT NULL,
                    updated_at INTEGER NOT NULL,
                    UNIQUE(qobuz_playlist_id, track_id, is_local)
                );
                CREATE INDEX IF NOT EXISTS idx_playlist_custom_order_playlist
                    ON playlist_track_custom_order(qobuz_playlist_id);
                CREATE INDEX IF NOT EXISTS idx_playlist_custom_order_position
                    ON playlist_track_custom_order(qobuz_playlist_id, custom_position);",
                )
                .map_err(|e| LibraryError::Database(format!("Migration failed: {}", e)))?;
        }

        // Migration: Add format_id to downloaded_purchases (compound PK: track_id + format_id)
        let has_format_id: bool = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('downloaded_purchases') WHERE name = 'format_id'",
                [],
                |row| row.get::<_, i32>(0),
            )
            .map(|count| count > 0)
            .unwrap_or(false);

        if !has_format_id {
            log::info!("Running migration: adding format_id to downloaded_purchases (compound PK)");
            self.conn
                .execute_batch(
                    r#"
                DROP TABLE IF EXISTS downloaded_purchases_new;

                CREATE TABLE downloaded_purchases_new (
                    track_id INTEGER NOT NULL,
                    format_id INTEGER NOT NULL DEFAULT 0,
                    album_id TEXT,
                    file_path TEXT NOT NULL,
                    downloaded_at TEXT NOT NULL DEFAULT (datetime('now')),
                    PRIMARY KEY (track_id, format_id)
                );

                INSERT INTO downloaded_purchases_new (track_id, format_id, album_id, file_path, downloaded_at)
                    SELECT track_id, 0, album_id, file_path, downloaded_at
                    FROM downloaded_purchases;

                DROP TABLE downloaded_purchases;
                ALTER TABLE downloaded_purchases_new RENAME TO downloaded_purchases;

                CREATE INDEX IF NOT EXISTS idx_downloaded_purchases_album
                    ON downloaded_purchases(album_id);
                "#,
                )
                .map_err(|e| {
                    LibraryError::Database(format!(
                        "downloaded_purchases format_id migration failed: {}",
                        e
                    ))
                })?;
        }

        crate::purchase_copies::migrate_legacy_registry(&self.conn)?;

        Ok(())
    }

    /// Provide raw connection access for external schema migrations.
    ///
    /// This is intentionally narrow: callers receive a shared reference so
    /// they can run DDL (CREATE TABLE, ALTER TABLE) but cannot move the
    /// connection out or replace it.  Use sparingly — prefer adding methods
    /// to LibraryDatabase directly for DML queries.
    pub fn with_connection<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&Connection) -> R,
    {
        f(&self.conn)
    }

    /// Provide mutable raw connection access for operations that require a
    /// transaction (e.g. reorder operations that delete + reinsert rows).
    ///
    /// Use sparingly — prefer adding methods to LibraryDatabase directly.
    pub fn with_connection_mut<F, R>(&mut self, f: F) -> R
    where
        F: FnOnce(&mut Connection) -> R,
    {
        f(&mut self.conn)
    }

    // === Folder Management ===

    /// Add a folder to the library with optional network info
    pub fn add_folder(&self, path: &str) -> Result<(), LibraryError> {
        self.conn
            .execute(
                "INSERT OR IGNORE INTO library_folders (path) VALUES (?)",
                params![path],
            )
            .map_err(|e| LibraryError::Database(e.to_string()))?;
        Ok(())
    }

    /// Add a folder with network detection info
    pub fn add_folder_with_network_info(
        &self,
        path: &str,
        is_network: bool,
        network_fs_type: Option<&str>,
    ) -> Result<i64, LibraryError> {
        self.conn
            .execute(
                "INSERT OR IGNORE INTO library_folders (path, is_network, network_fs_type) VALUES (?, ?, ?)",
                params![path, is_network as i32, network_fs_type],
            )
            .map_err(|e| LibraryError::Database(e.to_string()))?;

        // Get the folder ID (either newly inserted or existing)
        let id: i64 = self
            .conn
            .query_row(
                "SELECT id FROM library_folders WHERE path = ?",
                params![path],
                |row| row.get(0),
            )
            .map_err(|e| LibraryError::Database(e.to_string()))?;

        Ok(id)
    }

    /// Remove a folder from the library
    pub fn remove_folder(&self, path: &str) -> Result<(), LibraryError> {
        let root_id = self
            .conn
            .query_row(
                "SELECT id FROM library_folders WHERE path = ?",
                params![path],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map_err(|e| LibraryError::Database(e.to_string()))?;
        if let Some(root_id) = root_id {
            // A newly registered overlapping root may not have scanned yet.
            // Materialize every path-based owner before dropping this one so
            // removing the outer folder cannot orphan a still-owned image.
            self.reconcile_sacd_catalog_scope()?;
            self.remove_sacd_root(root_id)?;
            self.conn
                .execute(
                    "DELETE FROM local_scan_files WHERE root_id = ?",
                    params![root_id],
                )
                .map_err(|e| LibraryError::Database(e.to_string()))?;
            self.conn
                .execute(
                    "DELETE FROM local_scan_cue_refs WHERE root_id = ?",
                    params![root_id],
                )
                .map_err(|e| LibraryError::Database(e.to_string()))?;
            self.conn
                .execute(
                    "DELETE FROM local_scan_roots WHERE root_id = ?",
                    params![root_id],
                )
                .map_err(|e| LibraryError::Database(e.to_string()))?;
        }
        self.conn
            .execute("DELETE FROM library_folders WHERE path = ?", params![path])
            .map_err(|e| LibraryError::Database(e.to_string()))?;
        Ok(())
    }

    /// Get all enabled library folders (paths only, for scanning)
    pub fn get_folders(&self) -> Result<Vec<String>, LibraryError> {
        let mut stmt = self
            .conn
            .prepare("SELECT path FROM library_folders WHERE enabled = 1")
            .map_err(|e| LibraryError::Database(e.to_string()))?;

        let rows = stmt
            .query_map([], |row| row.get(0))
            .map_err(|e| LibraryError::Database(e.to_string()))?;

        let mut folders = Vec::new();
        for path in rows {
            folders.push(path.map_err(|e| LibraryError::Database(e.to_string()))?);
        }
        Ok(folders)
    }

    /// Get paths of all network folders (for offline filtering)
    pub fn get_network_folder_paths(&self) -> Result<Vec<String>, LibraryError> {
        let mut stmt = self
            .conn
            .prepare("SELECT path FROM library_folders WHERE is_network = 1")
            .map_err(|e| LibraryError::Database(e.to_string()))?;

        let rows = stmt
            .query_map([], |row| row.get(0))
            .map_err(|e| LibraryError::Database(e.to_string()))?;

        let mut folders = Vec::new();
        for path in rows {
            folders.push(path.map_err(|e| LibraryError::Database(e.to_string()))?);
        }
        Ok(folders)
    }

    /// Get all library folders with full metadata
    pub fn get_folders_with_metadata(&self) -> Result<Vec<LibraryFolder>, LibraryError> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, path, alias, enabled, is_network, network_fs_type, user_override_network, last_scan
                 FROM library_folders ORDER BY path"
            )
            .map_err(|e| LibraryError::Database(e.to_string()))?;

        let rows = stmt
            .query_map([], |row| {
                Ok(LibraryFolder {
                    id: row.get(0)?,
                    path: row.get(1)?,
                    alias: row.get(2)?,
                    enabled: row.get::<_, i32>(3)? != 0,
                    is_network: row.get::<_, i32>(4).unwrap_or(0) != 0,
                    network_fs_type: row.get(5)?,
                    user_override_network: row.get::<_, i32>(6).unwrap_or(0) != 0,
                    last_scan: row.get(7)?,
                })
            })
            .map_err(|e| LibraryError::Database(e.to_string()))?;

        let mut folders = Vec::new();
        for folder in rows {
            folders.push(folder.map_err(|e| LibraryError::Database(e.to_string()))?);
        }
        Ok(folders)
    }

    /// Get a single folder by ID
    pub fn get_folder_by_id(&self, id: i64) -> Result<Option<LibraryFolder>, LibraryError> {
        let result = self
            .conn
            .query_row(
                "SELECT id, path, alias, enabled, is_network, network_fs_type, user_override_network, last_scan
                 FROM library_folders WHERE id = ?",
                params![id],
                |row| {
                    Ok(LibraryFolder {
                        id: row.get(0)?,
                        path: row.get(1)?,
                        alias: row.get(2)?,
                        enabled: row.get::<_, i32>(3)? != 0,
                        is_network: row.get::<_, i32>(4).unwrap_or(0) != 0,
                        network_fs_type: row.get(5)?,
                        user_override_network: row.get::<_, i32>(6).unwrap_or(0) != 0,
                        last_scan: row.get(7)?,
                    })
                },
            )
            .optional()
            .map_err(|e| LibraryError::Database(e.to_string()))?;

        Ok(result)
    }

    /// Update folder settings
    pub fn update_folder_settings(
        &self,
        id: i64,
        alias: Option<&str>,
        enabled: bool,
        is_network: bool,
        network_fs_type: Option<&str>,
        user_override_network: bool,
    ) -> Result<(), LibraryError> {
        self.conn
            .execute(
                "UPDATE library_folders
                 SET alias = ?, enabled = ?, is_network = ?, network_fs_type = ?, user_override_network = ?
                 WHERE id = ?",
                params![alias, enabled as i32, is_network as i32, network_fs_type, user_override_network as i32, id],
            )
            .map_err(|e| LibraryError::Database(e.to_string()))?;
        Ok(())
    }

    /// Set folder enabled state
    pub fn set_folder_enabled(&self, id: i64, enabled: bool) -> Result<(), LibraryError> {
        self.conn
            .execute(
                "UPDATE library_folders SET enabled = ? WHERE id = ?",
                params![enabled as i32, id],
            )
            .map_err(|e| LibraryError::Database(e.to_string()))?;
        Ok(())
    }

    /// Update last scan time for a folder
    pub fn update_folder_scan_time(&self, path: &str, timestamp: i64) -> Result<(), LibraryError> {
        self.conn
            .execute(
                "UPDATE library_folders SET last_scan = ? WHERE path = ?",
                params![timestamp, path],
            )
            .map_err(|e| LibraryError::Database(e.to_string()))?;
        Ok(())
    }

    /// Update folder path (moves the folder to a new location)
    /// This also clears the last_scan since the new path needs to be scanned
    pub fn update_folder_path(&self, id: i64, new_path: &str) -> Result<(), LibraryError> {
        // Check if new path already exists as a different folder
        let existing: Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM library_folders WHERE path = ? AND id != ?",
                params![new_path, id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| LibraryError::Database(e.to_string()))?;

        if existing.is_some() {
            return Err(LibraryError::Database(
                "A folder with this path already exists".to_string(),
            ));
        }

        self.conn
            .execute(
                "UPDATE library_folders SET path = ?, last_scan = NULL WHERE id = ?",
                params![new_path, id],
            )
            .map_err(|e| LibraryError::Database(e.to_string()))?;
        self.conn
            .execute(
                "DELETE FROM local_scan_files WHERE root_id = ?",
                params![id],
            )
            .map_err(|e| LibraryError::Database(e.to_string()))?;
        self.conn
            .execute(
                "DELETE FROM local_scan_cue_refs WHERE root_id = ?",
                params![id],
            )
            .map_err(|e| LibraryError::Database(e.to_string()))?;
        self.conn
            .execute(
                "DELETE FROM local_scan_roots WHERE root_id = ?",
                params![id],
            )
            .map_err(|e| LibraryError::Database(e.to_string()))?;
        self.reconcile_sacd_catalog_scope()?;
        Ok(())
    }

    // === Track Management ===

    /// Check if a file path is already registered as a Qobuz cached track
    /// Returns true if the file exists with source = 'qobuz_download' (legacy name kept for DB compatibility)
    pub fn is_qobuz_cached_track_by_path(&self, file_path: &str) -> Result<bool, LibraryError> {
        let count: i64 = self.conn
            .query_row(
                "SELECT COUNT(*) FROM local_tracks WHERE file_path = ?1 AND source = 'qobuz_download'",
                params![file_path],
                |row| row.get(0)
            )
            .map_err(|e| LibraryError::Database(e.to_string()))?;
        Ok(count > 0)
    }

    /// Insert or update a track (skips if file is already a Qobuz cached track)
    pub fn insert_track(&self, track: &LocalTrack) -> Result<i64, LibraryError> {
        let is_network_mount = crate::mount_info::is_network_path(Path::new(&track.file_path));
        self.insert_track_with_mount_hint(track, is_network_mount)
    }

    /// Scanner-only insert path. Mount classification is a property of the
    /// root and is computed once before enumeration, not once per track.
    pub(crate) fn insert_scanned_track(
        &self,
        track: &LocalTrack,
        is_network_mount: bool,
    ) -> Result<i64, LibraryError> {
        self.insert_track_with_mount_hint(track, is_network_mount)
    }

    fn insert_track_with_mount_hint(
        &self,
        track: &LocalTrack,
        is_network_mount: bool,
    ) -> Result<i64, LibraryError> {
        // Don't overwrite Qobuz cached tracks with scanned data
        if self.is_qobuz_cached_track_by_path(&track.file_path)? {
            log::debug!(
                "Skipping track insert - already exists as Qobuz cached track: {}",
                track.file_path
            );
            // Return the existing ID
            return self
                .conn
                .query_row(
                    "SELECT id FROM local_tracks WHERE file_path = ?1",
                    params![track.file_path],
                    |row| row.get(0),
                )
                .map_err(|e| LibraryError::Database(e.to_string()));
        }

        // Detect if this file is a Qobuz purchased download
        let is_purchase: bool = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM downloaded_purchases WHERE file_path = ?1",
                params![track.file_path],
                |row| row.get::<_, i64>(0),
            )
            .unwrap_or(0)
            > 0;

        let source = if is_purchase {
            "qobuz_purchase"
        } else {
            "user"
        };
        let genres_json = Self::track_genres_json(track);

        // Re-indexing an already-known file must KEEP ITS ROWID.
        //
        // `INSERT OR REPLACE` resolves a conflict by DELETING the old row and
        // inserting a new one, which hands out a fresh rowid. `local_tracks.id`
        // is a foreign key elsewhere — `playlist_local_tracks.local_track_id`
        // (a local file added to a Qobuz playlist) — and SQLite's foreign keys
        // are OFF here (the only pragmas set are journal_mode and synchronous),
        // so the cascade never fires and the playlist row survives pointing at
        // an id that no longer exists. The INNER JOIN that reads it then
        // returns nothing: the track silently vanishes from the playlist. The
        // scan re-inserts EVERY file (there is no mtime skip), so this happened
        // on every rescan. Verified against a scratch database.
        //
        // First-class LOCAL playlists are unaffected — `local_playlist_tracks`
        // stores `local_path`, not an id.
        //
        // Two identities, because there are two unique constraints: the table's
        // `UNIQUE(file_path, cue_start_secs)`, and the partial index
        // `idx_tracks_file_nocue ON local_tracks(file_path) WHERE cue_file_path
        // IS NULL` (NULL `cue_start_secs` values do not collide under the
        // former, so the latter is what actually catches an ordinary track).
        let existing_id: Option<i64> = if track.cue_file_path.is_none() {
            self.conn
                .query_row(
                    "SELECT id FROM local_tracks WHERE file_path = ?1 AND cue_file_path IS NULL",
                    params![track.file_path],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|e| LibraryError::Database(e.to_string()))?
        } else {
            self.conn
                .query_row(
                    "SELECT id FROM local_tracks
                     WHERE file_path = ?1
                       AND cue_start_secs IS ?2",
                    params![track.file_path, track.cue_start_secs],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|e| LibraryError::Database(e.to_string()))?
        };

        if let Some(id) = existing_id {
            self.conn
                .execute(
                    r#"UPDATE local_tracks SET
                        title = ?1, artist = ?2, album = ?3, album_artist = ?4,
                        track_number = ?5, disc_number = ?6, year = ?7, genre = ?8,
                        genres_json = ?9, catalog_number = ?10, duration_secs = ?11, format = ?12,
                        bit_depth = ?13, sample_rate = ?14, channels = ?15,
                        file_size_bytes = ?16, cue_file_path = ?17, cue_start_secs = ?18,
                        cue_end_secs = ?19, artwork_path = ?20, last_modified = ?21,
                        indexed_at = ?22, album_group_key = ?23, album_group_title = ?24,
                        source = ?25, is_network_mount = ?26
                       WHERE id = ?27"#,
                    params![
                        track.title,
                        track.artist,
                        track.album,
                        track.album_artist,
                        track.track_number,
                        track.disc_number,
                        track.year,
                        track.genre,
                        genres_json,
                        track.catalog_number,
                        track.duration_secs as i64,
                        track.format.to_string(),
                        track.bit_depth,
                        track.sample_rate,
                        track.channels,
                        track.file_size_bytes as i64,
                        track.cue_file_path,
                        track.cue_start_secs,
                        track.cue_end_secs,
                        track.artwork_path,
                        track.last_modified,
                        track.indexed_at,
                        track.album_group_key,
                        track.album_group_title,
                        source,
                        is_network_mount as i64,
                        id,
                    ],
                )
                .map_err(|e| LibraryError::Database(e.to_string()))?;
            return Ok(id);
        }

        self.conn
            .execute(
                r#"INSERT OR REPLACE INTO local_tracks
               (file_path, title, artist, album, album_artist, track_number,
                disc_number, year, genre, genres_json, catalog_number, duration_secs, format, bit_depth,
                sample_rate, channels, file_size_bytes, cue_file_path,
                cue_start_secs, cue_end_secs, artwork_path, last_modified, indexed_at,
                album_group_key, album_group_title, source, is_network_mount,
                isrc, musicbrainz_recording_id, musicbrainz_track_id,
                musicbrainz_release_id, musicbrainz_release_group_id, musicbrainz_artist_id)
               VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?,
                       ?, ?, ?, ?, ?, ?)"#,
                params![
                    track.file_path,
                    track.title,
                    track.artist,
                    track.album,
                    track.album_artist,
                    track.track_number,
                    track.disc_number,
                    track.year,
                    track.genre,
                    genres_json,
                    track.catalog_number,
                    track.duration_secs as i64,
                    track.format.to_string(),
                    track.bit_depth,
                    track.sample_rate,
                    track.channels,
                    track.file_size_bytes as i64,
                    track.cue_file_path,
                    track.cue_start_secs,
                    track.cue_end_secs,
                    track.artwork_path,
                    track.last_modified,
                    track.indexed_at,
                    track.album_group_key,
                    track.album_group_title,
                    source,
                    is_network_mount as i64,
                    track.isrc,
                    track.musicbrainz_recording_id,
                    track.musicbrainz_track_id,
                    track.musicbrainz_release_id,
                    track.musicbrainz_release_group_id,
                    track.musicbrainz_artist_id,
                ],
            )
            .map_err(|e| LibraryError::Database(e.to_string()))?;

        Ok(self.conn.last_insert_rowid())
    }

    /// Add the ISRC + MusicBrainz id columns when absent (idempotent, cheap:
    /// one `pragma_table_info` probe). Called from the migration chain AND
    /// after the sample_rate table rebuild, which recreates `local_tracks`
    /// without any column added by a later ALTER.
    fn ensure_identity_columns(&self) -> Result<(), LibraryError> {
        let has_isrc: bool = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('local_tracks') WHERE name = 'isrc'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .map(|count| count > 0)
            .unwrap_or(false);
        if !has_isrc {
            log::info!(
                "Running migration: adding identity columns (isrc, musicbrainz_*) to local_tracks"
            );
            self.conn
                .execute_batch(
                    "ALTER TABLE local_tracks ADD COLUMN isrc TEXT;
                     ALTER TABLE local_tracks ADD COLUMN musicbrainz_recording_id TEXT;
                     ALTER TABLE local_tracks ADD COLUMN musicbrainz_track_id TEXT;
                     ALTER TABLE local_tracks ADD COLUMN musicbrainz_release_id TEXT;
                     ALTER TABLE local_tracks ADD COLUMN musicbrainz_release_group_id TEXT;
                     ALTER TABLE local_tracks ADD COLUMN musicbrainz_artist_id TEXT;
                     CREATE INDEX IF NOT EXISTS idx_tracks_isrc ON local_tracks(isrc);
                     CREATE INDEX IF NOT EXISTS idx_tracks_mb_recording ON local_tracks(musicbrainz_recording_id);",
                )
                .map_err(|e| LibraryError::Database(format!("Migration failed: {}", e)))?;
        }
        Ok(())
    }

    /// Get a track by ID
    pub fn get_track(&self, id: i64) -> Result<Option<LocalTrack>, LibraryError> {
        let mut stmt = self
            .conn
            .prepare(&format!(
                "SELECT {} FROM local_tracks WHERE id = ?",
                Self::TRACK_COLUMNS
            ))
            .map_err(|e| LibraryError::Database(e.to_string()))?;

        stmt.query_row(params![id], |row| Self::row_to_track(row))
            .optional()
            .map_err(|e| LibraryError::Database(e.to_string()))
    }

    /// Get a track by file path (for non-CUE tracks)
    pub fn get_track_by_path(&self, path: &str) -> Result<Option<LocalTrack>, LibraryError> {
        let mut stmt = self
            .conn
            .prepare(&format!(
                "SELECT {} FROM local_tracks WHERE file_path = ? AND cue_file_path IS NULL",
                Self::TRACK_COLUMNS
            ))
            .map_err(|e| LibraryError::Database(e.to_string()))?;

        stmt.query_row(params![path], |row| Self::row_to_track(row))
            .optional()
            .map_err(|e| LibraryError::Database(e.to_string()))
    }

    /// Delete all tracks in a folder
    pub fn delete_tracks_in_folder(&self, folder: &str) -> Result<usize, LibraryError> {
        let pattern = format!("{}%", folder);
        let count = self
            .conn
            .execute(
                "DELETE FROM local_tracks WHERE file_path LIKE ?",
                params![pattern],
            )
            .map_err(|e| LibraryError::Database(e.to_string()))?;
        Ok(count)
    }

    /// Delete all tracks under a folder, matching a path prefix terminated by
    /// the separator so a sibling like `/music/jazz2` is NOT removed when
    /// deleting `/music/jazz`. Use this for folder removal — the older
    /// `delete_tracks_in_folder` (kept for backward behavior compatibility with
    /// the Tauri command) has a prefix-collision bug (`{}%`, no separator).
    pub fn delete_tracks_in_folder_prefixed(&self, folder: &str) -> Result<usize, LibraryError> {
        let pattern = format!("{}/%", folder.trim_end_matches('/'));
        let count = self
            .conn
            .execute(
                "DELETE FROM local_tracks WHERE file_path LIKE ?",
                params![pattern],
            )
            .map_err(|e| LibraryError::Database(e.to_string()))?;
        Ok(count)
    }

    /// Distinct `album_group_key`s of the indexed tracks under `folder` — the
    /// same keys used as the playback/Recently-Played album id. Call BEFORE
    /// deleting the folder so the frontend can prune those albums from the
    /// recently-played store.
    pub fn album_keys_in_folder(&self, folder: &str) -> Result<Vec<String>, LibraryError> {
        let pattern = format!("{}/%", folder.trim_end_matches('/'));
        let mut stmt = self
            .conn
            .prepare(
                "SELECT DISTINCT album_group_key FROM local_tracks
                 WHERE file_path LIKE ? AND album_group_key IS NOT NULL AND album_group_key != ''",
            )
            .map_err(|e| LibraryError::Database(e.to_string()))?;
        let rows = stmt
            .query_map(params![pattern], |row| row.get::<_, String>(0))
            .map_err(|e| LibraryError::Database(e.to_string()))?;
        let mut keys = Vec::new();
        for k in rows {
            keys.push(k.map_err(|e| LibraryError::Database(e.to_string()))?);
        }
        Ok(keys)
    }

    /// Remove a folder and its indexed tracks (separator-safe cascade). Mirrors
    /// the Tauri remove-folder command order: drop the folder row, then the
    /// tracks under it. Returns the number of tracks removed.
    pub fn remove_folder_with_tracks(&self, path: &str) -> Result<usize, LibraryError> {
        self.remove_folder(path)?;
        self.delete_tracks_in_folder_prefixed(path)
    }

    /// Clear all LOCAL library tracks (preserves Qobuz downloads)
    pub fn clear_all_tracks(&self) -> Result<(), LibraryError> {
        self.conn
            .execute_batch(
                "BEGIN IMMEDIATE;
                 DELETE FROM local_tracks WHERE source IS NULL OR source != 'qobuz_download';
                 DELETE FROM local_sacd_tracks;
                 DELETE FROM local_sacd_roots;
                 DELETE FROM local_sacd_images;
                 DELETE FROM local_scan_files;
                 DELETE FROM local_scan_cue_refs;
                 DELETE FROM local_scan_roots;
                 UPDATE library_folders SET last_scan=NULL;
                 COMMIT;",
            )
            .map_err(|e| LibraryError::Database(e.to_string()))?;
        Ok(())
    }

    /// Get all file paths for local tracks (for cleanup check)
    pub fn get_all_track_paths(&self) -> Result<Vec<(i64, String)>, LibraryError> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, file_path FROM local_tracks WHERE source IS NULL OR source = 'user'",
            )
            .map_err(|e| LibraryError::Database(e.to_string()))?;

        let rows = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .map_err(|e| LibraryError::Database(e.to_string()))?;

        let mut paths = Vec::new();
        for row in rows {
            paths.push(row.map_err(|e| LibraryError::Database(e.to_string()))?);
        }
        Ok(paths)
    }

    /// Delete tracks by their IDs
    pub fn delete_tracks_by_ids(&self, ids: &[i64]) -> Result<usize, LibraryError> {
        if ids.is_empty() {
            return Ok(0);
        }

        let placeholders: Vec<String> = ids.iter().map(|_| "?".to_string()).collect();
        let query = format!(
            "DELETE FROM local_tracks WHERE id IN ({})",
            placeholders.join(",")
        );

        let params: Vec<&dyn rusqlite::ToSql> =
            ids.iter().map(|id| id as &dyn rusqlite::ToSql).collect();

        let count = self
            .conn
            .execute(&query, params.as_slice())
            .map_err(|e| LibraryError::Database(e.to_string()))?;

        Ok(count)
    }

    // === Query Methods ===

    /// Get all albums with optional hidden filter
    pub fn get_albums(&self, include_hidden: bool) -> Result<Vec<LocalAlbum>, LibraryError> {
        self.get_albums_with_filter(include_hidden, true)
    }

    /// Get all albums with optional filters for hidden and Qobuz downloads
    pub fn get_albums_with_filter(
        &self,
        include_hidden: bool,
        include_qobuz_downloads: bool,
    ) -> Result<Vec<LocalAlbum>, LibraryError> {
        self.get_albums_with_full_filter(include_hidden, include_qobuz_downloads, false)
    }

    /// Get all albums with full filter options including network folder exclusion
    /// This method filters network folders directly in SQL to avoid N+1 query patterns
    pub fn get_albums_with_full_filter(
        &self,
        include_hidden: bool,
        include_qobuz_downloads: bool,
        exclude_network_folders: bool,
    ) -> Result<Vec<LocalAlbum>, LibraryError> {
        let source_filter = if include_qobuz_downloads {
            ""
        } else {
            "AND (source IS NULL OR source != 'qobuz_download')"
        };

        // Network folder filter: exclude tracks whose file_path starts with any network folder path
        let network_filter = if exclude_network_folders {
            "AND NOT EXISTS (
                SELECT 1 FROM library_folders nf
                WHERE nf.is_network = 1
                AND local_tracks.file_path LIKE nf.path || '%'
            )"
        } else {
            ""
        };

        let query = if include_hidden {
            format!(
                r#"
            SELECT
                group_key,
                MIN(title) as title,
                CASE
                    WHEN COUNT(DISTINCT artist) > 1 THEN 'Various Artists'
                    ELSE MIN(artist)
                END as artist,
                GROUP_CONCAT(DISTINCT artist) as all_artists,
                MIN(year) as year,
                MIN(catalog_number) as catalog_number,
                MAX(CASE WHEN artwork_path IS NOT NULL THEN artwork_path END) as artwork,
                COUNT(*) as track_count,
                SUM(duration_secs) as total_duration,
                MAX(format) as format,
                MAX(bit_depth) as bit_depth,
                MAX(sample_rate) as sample_rate,
                json_group_array(json(genres_json)) as genre_sets,
                MAX(group_key) as directory_path,
                MAX(source) as source
            FROM (
                SELECT
                    COALESCE(album_group_key, album || '|' || COALESCE(album_artist, artist)) as group_key,
                    COALESCE(album_group_title, album) as title,
                    COALESCE(album_artist, artist) as artist,
                    year,
                    catalog_number,
                    artwork_path,
                    duration_secs,
                    format,
                    bit_depth,
                    sample_rate,
                    COALESCE(
                        NULLIF(genres_json, '[]'),
                        CASE WHEN genre IS NULL OR TRIM(genre) = ''
                             THEN '[]' ELSE json_array(TRIM(genre)) END
                    ) AS genres_json,
                    COALESCE(source, 'user') as source
                FROM local_tracks
                WHERE 1=1 {} {}
            )
            GROUP BY group_key
            ORDER BY artist, title
            "#,
                source_filter, network_filter
            )
        } else {
            format!(
                r#"
            SELECT
                group_key,
                MIN(title) as title,
                CASE
                    WHEN COUNT(DISTINCT artist) > 1 THEN 'Various Artists'
                    ELSE MIN(artist)
                END as artist,
                GROUP_CONCAT(DISTINCT artist) as all_artists,
                MIN(year) as year,
                MIN(catalog_number) as catalog_number,
                MAX(CASE WHEN artwork_path IS NOT NULL THEN artwork_path END) as artwork,
                COUNT(*) as track_count,
                SUM(duration_secs) as total_duration,
                MAX(format) as format,
                MAX(bit_depth) as bit_depth,
                MAX(sample_rate) as sample_rate,
                json_group_array(json(genres_json)) as genre_sets,
                MAX(group_key) as directory_path,
                MAX(source) as source
            FROM (
                SELECT
                    COALESCE(album_group_key, album || '|' || COALESCE(album_artist, artist)) as group_key,
                    COALESCE(album_group_title, album) as title,
                    COALESCE(album_artist, artist) as artist,
                    year,
                    catalog_number,
                    artwork_path,
                    duration_secs,
                    format,
                    bit_depth,
                    sample_rate,
                    COALESCE(
                        NULLIF(genres_json, '[]'),
                        CASE WHEN genre IS NULL OR TRIM(genre) = ''
                             THEN '[]' ELSE json_array(TRIM(genre)) END
                    ) AS genres_json,
                    COALESCE(source, 'user') as source
                FROM local_tracks
                WHERE 1=1 {} {}
            )
            WHERE group_key NOT IN (
                SELECT album_group_key FROM album_settings WHERE hidden = 1
            )
            GROUP BY group_key
            ORDER BY artist, title
            "#,
                source_filter, network_filter
            )
        };

        let mut stmt = self
            .conn
            .prepare(&query)
            .map_err(|e| LibraryError::Database(e.to_string()))?;

        let rows = stmt
            .query_map([], |row| {
                let group_key: String = row.get(0)?;
                let album: String = row.get(1)?;
                let artist: String = row.get(2)?;
                let all_artists: String = row.get::<_, Option<String>>(3)?.unwrap_or_default();
                let artwork_path: Option<String> = row.get(6)?;

                log::debug!(
                    "Album {} by {}: artwork_path = {:?}",
                    album,
                    artist,
                    artwork_path
                );

                Ok(LocalAlbum {
                    id: group_key.clone(),
                    title: album,
                    artist,
                    all_artists,
                    year: row.get(4)?,
                    catalog_number: row.get(5)?,
                    genres: Self::genres_from_sets_json(
                        row.get::<_, Option<String>>(12)?.as_deref(),
                    ),
                    artwork_path,
                    artwork_source: None,
                    track_count: row.get(7)?,
                    total_duration_secs: row.get::<_, i64>(8)? as u64,
                    format: Self::parse_format(
                        &row.get::<_, Option<String>>(9)?.unwrap_or_default(),
                    ),
                    bit_depth: row.get(10)?,
                    sample_rate: row.get::<_, Option<f64>>(11)?.unwrap_or(44100.0),
                    directory_path: row
                        .get::<_, Option<String>>(13)?
                        .unwrap_or_else(|| group_key.clone()),
                    source_folders: None,
                    source: row
                        .get::<_, Option<String>>(14)?
                        .unwrap_or_else(|| "user".to_string()),
                    sources: Vec::new(),
                    identity_tracks: Vec::new(),
                })
            })
            .map_err(|e| LibraryError::Database(e.to_string()))?;

        let mut albums = Vec::new();
        for album in rows {
            albums.push(album.map_err(|e| LibraryError::Database(e.to_string()))?);
        }
        Ok(albums)
    }

    /// Get tracks for an album group
    pub fn get_album_tracks(&self, group_key: &str) -> Result<Vec<LocalTrack>, LibraryError> {
        let sql = format!(
            "SELECT {} FROM local_tracks \
             WHERE COALESCE(album_group_key, album || '|' || COALESCE(album_artist, artist)) = ? \
             ORDER BY disc_number, track_number, title",
            Self::TRACK_COLUMNS
        );
        let mut stmt = self
            .conn
            .prepare(&sql)
            .map_err(|e| LibraryError::Database(e.to_string()))?;

        let rows = stmt
            .query_map(params![group_key], |row| Self::row_to_track(row))
            .map_err(|e| LibraryError::Database(e.to_string()))?;

        let mut tracks = Vec::new();
        for track in rows {
            tracks.push(track.map_err(|e| LibraryError::Database(e.to_string()))?);
        }
        Ok(tracks)
    }

    /// List the immediate children of a folder in the local-library
    /// filesystem hierarchy.
    ///
    /// Walks the physical file hierarchy and computes one row per direct
    /// child. SACD virtual tracks are projected beneath their backing image,
    /// so the image is an expandable synthetic folder. Returns folders first
    /// (alphabetical, case-insensitive), then tracks likewise.
    ///
    /// Filters `COALESCE(source, 'user') = 'user'` so Qobuz offline
    /// downloads are excluded; Plex rows already live outside
    /// `local_tracks`.
    ///
    /// `parent_path` is the absolute path of the folder whose children
    /// to enumerate. The `_` and `%` characters are escaped before
    /// binding to defend against pattern-injection on paths that
    /// contain SQL LIKE metacharacters.
    pub fn list_folder_children(
        &self,
        parent_path: &str,
        exclude_network_folders: bool,
    ) -> Result<Vec<FolderTreeEntry>, LibraryError> {
        let escaped_prefix = escape_like_pattern(parent_path);

        // Network folder filter: exclude tracks whose physical path starts
        // with any registered network-mount folder path. Mirrors the
        // mechanism used by `get_albums_with_full_filter` so tree rail
        // visibility matches flat-mode + recursive playback.
        let network_filter = if exclude_network_folders {
            "AND NOT EXISTS ( \
                SELECT 1 FROM library_folders nf \
                WHERE nf.is_network = 1 \
                AND folder_tracks.physical_path LIKE nf.path || '%' \
            )"
        } else {
            ""
        };

        // SQL strategy (CTE form for readability):
        //   browse_path   = file_path for ordinary tracks; for SACD tracks,
        //                   image_path + a stable synthetic leaf label
        //   suffix        = browse_path with the parent prefix + '/' stripped
        //   child_segment = leading path component of suffix
        //   kind          = 'folder' if suffix contains a '/', else 'track'
        // Group by (child_segment, kind) so folders aggregate over all
        // descendant tracks; track rows are 1:1 with their file. Include
        // MIN(file_path) so we can recover the absolute path for tracks
        // (folders ignore it and reconstruct path from parent + segment).
        let sql = format!(
            "WITH folder_tracks AS ( \
                SELECT local_tracks.*, \
                       COALESCE( \
                           sacd_images.image_path || '/' || \
                               printf('%03d - %s', sacd_tracks.track_number, \
                                      replace(local_tracks.title, '/', '∕')), \
                           local_tracks.file_path \
                       ) AS browse_path, \
                       COALESCE(sacd_images.image_path, local_tracks.file_path) \
                           AS physical_path \
                  FROM local_tracks \
                  LEFT JOIN local_sacd_tracks sacd_tracks \
                    ON sacd_tracks.local_track_id=local_tracks.id \
                  LEFT JOIN local_sacd_images sacd_images \
                    ON sacd_images.fingerprint=sacd_tracks.fingerprint \
             ), \
             candidates AS ( \
                SELECT \
                    substr(browse_path, length(?1) + 2) AS suffix, \
                    file_path, \
                    artwork_path \
                FROM folder_tracks \
                WHERE browse_path LIKE ?2 || '/%' ESCAPE '\\' \
                  AND COALESCE(source, 'user') = 'user' \
                  {network_filter} \
             ), \
             classified AS ( \
                SELECT \
                    CASE WHEN instr(suffix, '/') > 0 \
                         THEN substr(suffix, 1, instr(suffix, '/') - 1) \
                         ELSE suffix \
                    END AS child_segment, \
                    CASE WHEN instr(suffix, '/') > 0 \
                         THEN 'folder' ELSE 'track' \
                    END AS kind, \
                    file_path, \
                    artwork_path \
                FROM candidates \
             ) \
             SELECT \
                child_segment, \
                kind, \
                COUNT(*) AS track_count_under, \
                MAX(artwork_path) AS artwork, \
                MIN(file_path) AS one_file_path \
             FROM classified \
             GROUP BY child_segment, kind",
            network_filter = network_filter,
        );

        let mut stmt = self
            .conn
            .prepare(&sql)
            .map_err(|e| LibraryError::Database(e.to_string()))?;

        // ?1 bound with the unescaped path (used in length() arithmetic
        // on the computed browse_path, which is itself unescaped).
        // ?2 bound with the LIKE-escaped pattern prefix.
        let rows = stmt
            .query_map(params![parent_path, escaped_prefix], |row| {
                let segment: String = row.get(0)?;
                let kind: String = row.get(1)?;
                let count: u32 = row.get(2)?;
                let artwork: Option<String> = row.get(3)?;
                let one_file_path: Option<String> = row.get(4)?;
                Ok((segment, kind, count, artwork, one_file_path))
            })
            .map_err(|e| LibraryError::Database(e.to_string()))?;

        let mut entries: Vec<FolderTreeEntry> = Vec::new();
        for row in rows {
            let (segment, kind, count, artwork, one_file_path) =
                row.map_err(|e| LibraryError::Database(e.to_string()))?;
            match kind.as_str() {
                "folder" => {
                    let path = format!("{}/{}", parent_path, segment);
                    entries.push(FolderTreeEntry::Folder {
                        path,
                        segment,
                        track_count_under: count,
                        artwork,
                    });
                }
                "track" => {
                    // Use the actual file_path so paths with edge-case
                    // characters round-trip exactly as stored.
                    let path =
                        one_file_path.unwrap_or_else(|| format!("{}/{}", parent_path, segment));
                    entries.push(FolderTreeEntry::Track { path, segment });
                }
                _ => {
                    // Unknown kind — skip defensively.
                }
            }
        }

        // Sort: folders first, then tracks; alphabetical (case-insensitive)
        // within each group. Done in Rust because we already have all rows
        // in memory after the GROUP BY, and Rust's case-insensitive compare
        // is more obvious than COLLATE NOCASE on a CASE-derived column.
        entries.sort_by(|a, b| {
            let kind_rank = |e: &FolderTreeEntry| match e {
                FolderTreeEntry::Folder { .. } => 0,
                FolderTreeEntry::Track { .. } => 1,
            };
            let segment = |e: &FolderTreeEntry| match e {
                FolderTreeEntry::Folder { segment, .. } => segment.clone(),
                FolderTreeEntry::Track { segment, .. } => segment.clone(),
            };
            kind_rank(a)
                .cmp(&kind_rank(b))
                .then_with(|| segment(a).to_lowercase().cmp(&segment(b).to_lowercase()))
        });

        Ok(entries)
    }

    /// List the direct-child tracks of a folder (NON-recursive).
    ///
    /// Returns rows whose projected browse path is exactly
    /// `folder_path + "/" + filename` — files in subfolders are excluded.
    /// For SACD this makes the virtual tracks direct children of the image.
    /// Mirrors the source filter from
    /// [`Self::list_folder_children`] so Qobuz downloads do not appear.
    /// Ordering matches the canonical album-track ordering used by
    /// [`Self::get_album_tracks`]: disc, then track number, then title.
    pub fn list_folder_tracks(
        &self,
        folder_path: &str,
        exclude_network_folders: bool,
    ) -> Result<Vec<LocalTrack>, LibraryError> {
        let escaped_prefix = escape_like_pattern(folder_path);

        // See `list_folder_children` for the rationale on the network
        // filter — same EXISTS subquery so the tree rail and direct-
        // children listing reflect the same visible-track set.
        let network_filter = if exclude_network_folders {
            "AND NOT EXISTS ( \
                SELECT 1 FROM library_folders nf \
                WHERE nf.is_network = 1 \
                AND folder_tracks.physical_path LIKE nf.path || '%' \
            )"
        } else {
            ""
        };

        let sql = format!(
            "WITH folder_tracks AS ( \
                 SELECT local_tracks.*, \
                        COALESCE( \
                            sacd_images.image_path || '/' || \
                                printf('%03d - %s', sacd_tracks.track_number, \
                                       replace(local_tracks.title, '/', '∕')), \
                            local_tracks.file_path \
                        ) AS browse_path, \
                        COALESCE(sacd_images.image_path, local_tracks.file_path) \
                            AS physical_path \
                   FROM local_tracks \
                   LEFT JOIN local_sacd_tracks sacd_tracks \
                     ON sacd_tracks.local_track_id=local_tracks.id \
                   LEFT JOIN local_sacd_images sacd_images \
                     ON sacd_images.fingerprint=sacd_tracks.fingerprint \
             ) \
             SELECT {cols} FROM folder_tracks \
             WHERE browse_path LIKE ?1 || '/%' ESCAPE '\\' \
               AND substr(browse_path, length(?2) + 2) NOT LIKE '%/%' \
               AND COALESCE(source, 'user') = 'user' \
               {network_filter} \
             ORDER BY disc_number ASC NULLS LAST, \
                      track_number ASC NULLS LAST, \
                      title COLLATE NOCASE ASC",
            cols = Self::TRACK_COLUMNS,
            network_filter = network_filter,
        );

        let mut stmt = self
            .conn
            .prepare(&sql)
            .map_err(|e| LibraryError::Database(e.to_string()))?;

        // ?1 = LIKE-escaped pattern (matches paths under the folder).
        // ?2 = unescaped path used for substr arithmetic on browse_path.
        let rows = stmt
            .query_map(params![escaped_prefix, folder_path], |row| {
                Self::row_to_track(row)
            })
            .map_err(|e| LibraryError::Database(e.to_string()))?;

        let mut tracks = Vec::new();
        for track in rows {
            tracks.push(track.map_err(|e| LibraryError::Database(e.to_string()))?);
        }
        Ok(tracks)
    }

    /// List ALL tracks recursively under a folder (every descendant, at
    /// any depth). Mirrors the source filter and LIKE-escape strategy
    /// from [`Self::list_folder_tracks`] but does NOT require the
    /// browse path to live directly inside `folder_path` — every descendant
    /// row is included.
    ///
    /// Used by the tree-mode multi-select to populate the union of
    /// `selectedTrackIds` when the user ticks a folder-row checkbox.
    /// Returns the full track records (not just IDs) so the frontend
    /// can build queue items for "Play Next" / "Add to Queue" without
    /// a second round-trip.
    ///
    /// Ordering: by projected browse path ASC. This produces a stable tree
    /// order for ordinary files and SACD virtual leaves alike.
    pub fn list_folder_tracks_recursive(
        &self,
        folder_path: &str,
        exclude_network_folders: bool,
    ) -> Result<Vec<LocalTrack>, LibraryError> {
        let escaped_prefix = escape_like_pattern(folder_path);

        // Network filter mirrors the flat-mode `v2_library_search` /
        // `get_albums_with_full_filter` predicate so the recursive
        // multi-select boundary matches what the tree rail and the
        // playback path see.
        let network_filter = if exclude_network_folders {
            "AND NOT EXISTS ( \
                SELECT 1 FROM library_folders nf \
                WHERE nf.is_network = 1 \
                AND folder_tracks.physical_path LIKE nf.path || '%' \
            )"
        } else {
            ""
        };

        let sql = format!(
            "WITH folder_tracks AS ( \
                 SELECT local_tracks.*, \
                        COALESCE( \
                            sacd_images.image_path || '/' || \
                                printf('%03d - %s', sacd_tracks.track_number, \
                                       replace(local_tracks.title, '/', '∕')), \
                            local_tracks.file_path \
                        ) AS browse_path, \
                        COALESCE(sacd_images.image_path, local_tracks.file_path) \
                            AS physical_path \
                   FROM local_tracks \
                   LEFT JOIN local_sacd_tracks sacd_tracks \
                     ON sacd_tracks.local_track_id=local_tracks.id \
                   LEFT JOIN local_sacd_images sacd_images \
                     ON sacd_images.fingerprint=sacd_tracks.fingerprint \
             ) \
             SELECT {cols} FROM folder_tracks \
             WHERE browse_path LIKE ?1 || '/%' ESCAPE '\\' \
               AND COALESCE(source, 'user') = 'user' \
               {network_filter} \
             ORDER BY browse_path ASC",
            cols = Self::TRACK_COLUMNS,
            network_filter = network_filter,
        );

        let mut stmt = self
            .conn
            .prepare(&sql)
            .map_err(|e| LibraryError::Database(e.to_string()))?;

        let rows = stmt
            .query_map(params![escaped_prefix], |row| Self::row_to_track(row))
            .map_err(|e| LibraryError::Database(e.to_string()))?;

        let mut tracks = Vec::new();
        for track in rows {
            tracks.push(track.map_err(|e| LibraryError::Database(e.to_string()))?);
        }
        Ok(tracks)
    }

    /// Lightweight `COUNT(*)` of every user track whose projected browse path
    /// lives recursively under `folder_path`. Used by the tree-mode rail to
    /// populate the recursive descendant count on top-level scan-root
    /// rows (which are synthesized client-side and don't go through
    /// [`Self::list_folder_children`], so they don't carry their own
    /// precomputed `track_count_under`).
    ///
    /// Source filter (`COALESCE(source, 'user') = 'user'`) and the
    /// optional network-folder NOT EXISTS predicate match the listing
    /// primitives byte-for-byte so the count, the rail visibility, and
    /// recursive playback all agree on the same boundary.
    pub fn count_folder_tracks_recursive(
        &self,
        folder_path: &str,
        exclude_network_folders: bool,
    ) -> Result<u32, LibraryError> {
        let escaped_prefix = escape_like_pattern(folder_path);

        let network_filter = if exclude_network_folders {
            "AND NOT EXISTS ( \
                SELECT 1 FROM library_folders nf \
                WHERE nf.is_network = 1 \
                AND folder_tracks.physical_path LIKE nf.path || '%' \
            )"
        } else {
            ""
        };

        let sql = format!(
            "WITH folder_tracks AS ( \
                 SELECT local_tracks.*, \
                        COALESCE( \
                            sacd_images.image_path || '/' || \
                                printf('%03d - %s', sacd_tracks.track_number, \
                                       replace(local_tracks.title, '/', '∕')), \
                            local_tracks.file_path \
                        ) AS browse_path, \
                        COALESCE(sacd_images.image_path, local_tracks.file_path) \
                            AS physical_path \
                   FROM local_tracks \
                   LEFT JOIN local_sacd_tracks sacd_tracks \
                     ON sacd_tracks.local_track_id=local_tracks.id \
                   LEFT JOIN local_sacd_images sacd_images \
                     ON sacd_images.fingerprint=sacd_tracks.fingerprint \
             ) \
             SELECT COUNT(*) FROM folder_tracks \
             WHERE browse_path LIKE ?1 || '/%' ESCAPE '\\' \
               AND COALESCE(source, 'user') = 'user' \
               {network_filter}",
            network_filter = network_filter,
        );

        let count: i64 = self
            .conn
            .query_row(&sql, params![escaped_prefix], |row| row.get(0))
            .map_err(|e| LibraryError::Database(e.to_string()))?;
        Ok(count.try_into().unwrap_or(0))
    }

    /// Get all albums grouped by metadata (album + album_artist OR
    /// artist), with fallback to folder grouping for tracks with no
    /// usable album tag, and a single 'Unknown Album' bucket for total
    /// orphans.
    ///
    /// Mirrors the shape of [`Self::get_albums_with_full_filter`] but
    /// uses the metadata group key from
    /// [`crate::album_grouping::metadata_group_key_sql_expression`].
    /// Rows have `directory_path = ""` and `source_folders` populated
    /// with the comma-separated list of contributing folder keys (so
    /// the UI can show a tooltip when N folders > 1).
    ///
    /// `include_hidden` is currently ignored: the `album_settings.hidden`
    /// flag targets the FOLDER key, which does not map cleanly onto
    /// metadata-grouped rows. Revisit if user feedback asks for it.
    pub fn get_albums_metadata_grouped(
        &self,
        include_hidden: bool,
        include_qobuz_downloads: bool,
        exclude_network_folders: bool,
        group_mode: crate::album_grouping::AlbumGroupMode,
    ) -> Result<Vec<LocalAlbum>, LibraryError> {
        let _ = include_hidden;

        let source_filter = if include_qobuz_downloads {
            ""
        } else {
            "AND (source IS NULL OR source != 'qobuz_download')"
        };

        let network_filter = if exclude_network_folders {
            "AND NOT EXISTS (
                SELECT 1 FROM library_folders nf
                WHERE nf.is_network = 1
                AND local_tracks.file_path LIKE nf.path || '%'
            )"
        } else {
            ""
        };

        let group_key_expr = crate::album_grouping::group_key_sql_expression(group_mode);

        let query = format!(
            r#"
            WITH grouped AS (
                SELECT
                    {group_key} AS group_key,
                    -- Prefer `album` (metadata tag) over
                    -- `album_group_title` (scan-time snapshot, which
                    -- falls back to folder name if metadata is
                    -- missing). Fixes #411 — when album metadata is
                    -- valid, the folder name was winning because
                    -- COALESCE returned `album_group_title` first.
                    COALESCE(
                        NULLIF(NULLIF(TRIM(album), ''), 'Unknown Album'),
                        album_group_title,
                        'Unknown Album'
                    ) AS title,
                    COALESCE(album_artist, artist, 'Unknown Artist') AS artist,
                    year,
                    catalog_number,
                    artwork_path,
                    duration_secs,
                    format,
                    bit_depth,
                    sample_rate,
                    COALESCE(
                        NULLIF(genres_json, '[]'),
                        CASE WHEN genre IS NULL OR TRIM(genre) = ''
                             THEN '[]' ELSE json_array(TRIM(genre)) END
                    ) AS genres_json,
                    album_group_key AS source_folder,
                    COALESCE(source, 'user') AS source
                FROM local_tracks
                WHERE 1=1 {source_filter} {network_filter}
            )
            SELECT
                group_key,
                CASE WHEN group_key = '__unknown_album__'
                     THEN 'Unknown Album'
                     ELSE MIN(title)
                END AS title,
                CASE WHEN COUNT(DISTINCT artist) > 1
                     THEN 'Various Artists'
                     ELSE MIN(artist)
                END AS artist,
                GROUP_CONCAT(DISTINCT artist) AS all_artists,
                MIN(year) AS year,
                MIN(catalog_number) AS catalog_number,
                MAX(CASE WHEN artwork_path IS NOT NULL THEN artwork_path END) AS artwork,
                COUNT(*) AS track_count,
                SUM(duration_secs) AS total_duration,
                MAX(format) AS format,
                MAX(bit_depth) AS bit_depth,
                MAX(sample_rate) AS sample_rate,
                json_group_array(json(genres_json)) AS genre_sets,
                GROUP_CONCAT(DISTINCT source_folder) AS source_folders,
                MAX(source) AS source
            FROM grouped
            GROUP BY group_key
            ORDER BY (group_key = '__unknown_album__'), artist, title
            "#,
            group_key = group_key_expr,
            source_filter = source_filter,
            network_filter = network_filter,
        );

        let mut stmt = self
            .conn
            .prepare(&query)
            .map_err(|e| LibraryError::Database(e.to_string()))?;

        let rows = stmt
            .query_map([], |row| {
                let group_key: String = row.get(0)?;
                let album: String = row.get(1)?;
                let artist: String = row.get(2)?;
                let all_artists: String = row.get::<_, Option<String>>(3)?.unwrap_or_default();
                let artwork_path: Option<String> = row.get(6)?;
                let source_folders: Option<String> = row.get(13)?;

                Ok(LocalAlbum {
                    id: group_key.clone(),
                    title: album,
                    artist,
                    all_artists,
                    year: row.get(4)?,
                    catalog_number: row.get(5)?,
                    genres: Self::genres_from_sets_json(
                        row.get::<_, Option<String>>(12)?.as_deref(),
                    ),
                    artwork_path,
                    artwork_source: None,
                    track_count: row.get(7)?,
                    total_duration_secs: row.get::<_, i64>(8)? as u64,
                    format: Self::parse_format(
                        &row.get::<_, Option<String>>(9)?.unwrap_or_default(),
                    ),
                    bit_depth: row.get(10)?,
                    sample_rate: row.get::<_, Option<f64>>(11)?.unwrap_or(44100.0),
                    directory_path: String::new(),
                    source_folders,
                    source: row
                        .get::<_, Option<String>>(14)?
                        .unwrap_or_else(|| "user".to_string()),
                    sources: Vec::new(),
                    identity_tracks: Vec::new(),
                })
            })
            .map_err(|e| LibraryError::Database(e.to_string()))?;

        let mut albums = Vec::new();
        for album in rows {
            albums.push(album.map_err(|e| LibraryError::Database(e.to_string()))?);
        }
        Ok(albums)
    }

    /// Paginated, sort/filter-aware slice of metadata-grouped local
    /// albums. Designed to back the chunked-store + recycling-grid pool
    /// on the frontend: caller asks for `[offset, offset+limit)` and
    /// receives those rows plus the total count of rows matching the
    /// same filter (via `COUNT(*) OVER ()`).
    ///
    /// Sort: one of `"artist"` (default), `"title"`, `"year"`, paired
    /// with direction `"asc"` (default) or `"desc"`. Unknown values
    /// fall back to artist-ascending. Albums with no `year` always sink
    /// to the bottom for the year sort.
    ///
    /// Search: a non-empty `search` becomes a `LIKE '%pattern%'` match
    /// applied after aggregation against the album's title or artist
    /// (mirrors the legacy in-memory `matchesAlbumSearchFast`).
    ///
    /// Source consolidation: when `plex_cache_path` is provided and
    /// points to an existing file, the function `ATTACH`es that
    /// database and unions plex_cache_tracks (aggregated by album_key)
    /// with the local aggregation. Sort, filter, and pagination apply
    /// to the union as a single result set, so a Plex-dominant library
    /// behaves identically to a local-dominant one.
    pub fn get_albums_metadata_page(
        &self,
        offset: u64,
        limit: u64,
        search: Option<&str>,
        sort_by: &str,
        sort_dir: &str,
        include_qobuz_downloads: bool,
        exclude_network_folders: bool,
        plex_cache_path: Option<&std::path::Path>,
        remote_cache_path: Option<&std::path::Path>,
        remote_sources: &[&str],
        group_mode: crate::album_grouping::AlbumGroupMode,
    ) -> Result<crate::models::AlbumsMetadataPage, LibraryError> {
        // TWO attachments, deliberately. `plex_cache` is the original
        // per-source mirror; `remote_cache` is the shared one every source
        // added after it writes into (`qbz-media-cache`), keyed by a `source`
        // column. Plex will fold into the second and this will collapse back
        // to one — see that crate's header for why it is not folded in the
        // same change that introduced Jellyfin and Subsonic.
        //
        // Best-effort, and independently so: a missing or unreadable Plex
        // cache must not cost the user their Jellyfin rows, and vice versa.
        let plex_attached = self.attach_best_effort("plex_cache", plex_cache_path);
        // The shared mirror holds EVERY remote source, so which of them the
        // union may show is a separate question from whether the file is
        // there. Turning Jellyfin off has to hide Jellyfin's rows without
        // touching Subsonic's, and an empty list means "no remote source is
        // enabled" — do not attach at all.
        let remote_filter = remote_source_filter(remote_sources);
        let remote_attached = !remote_sources.is_empty()
            && self.attach_best_effort("remote_cache", remote_cache_path);
        let plex_has_genres_json = plex_attached
            && self.attached_has_column("plex_cache", "plex_cache_tracks", "genres_json");
        let remote_has_genres_json = remote_attached
            && self.attached_has_column("remote_cache", "remote_cache_tracks", "genres_json");
        let result = self.get_albums_metadata_page_inner(
            offset,
            limit,
            search,
            sort_by,
            sort_dir,
            include_qobuz_downloads,
            exclude_network_folders,
            plex_attached,
            remote_attached,
            plex_has_genres_json,
            remote_has_genres_json,
            &remote_filter,
            group_mode,
        );
        if plex_attached {
            let _ = self.conn.execute("DETACH DATABASE plex_cache", []);
        }
        if remote_attached {
            let _ = self.conn.execute("DETACH DATABASE remote_cache", []);
        }
        result
    }

    /// ATTACH `path` under `alias`, reporting whether the union may use it.
    ///
    /// DETACH first, defensively: a stale attachment left by a previous call
    /// (or by another user of this connection) makes the new ATTACH fail, and
    /// the failure mode of that is a silently local-only library.
    ///
    /// A failure is NON-FATAL by design. The alternative — refusing to list any
    /// albums because one mirror is missing — turns "your Jellyfin server is
    /// off" into "your music library is empty".
    fn attach_best_effort(&self, alias: &str, path: Option<&std::path::Path>) -> bool {
        let Some(path) = path.filter(|p| p.exists()) else {
            return false;
        };
        let _ = self.conn.execute(&format!("DETACH DATABASE {alias}"), []);
        let path_str = path.to_string_lossy().replace('\'', "''");
        self.conn
            .execute(&format!("ATTACH DATABASE '{path_str}' AS {alias}"), [])
            .is_ok()
    }

    fn attached_has_column(&self, schema: &str, table: &str, column: &str) -> bool {
        let sql = format!("PRAGMA {schema}.table_info({table})");
        let Ok(mut stmt) = self.conn.prepare(&sql) else {
            return false;
        };
        let Ok(rows) = stmt.query_map([], |row| row.get::<_, String>(1)) else {
            return false;
        };
        let found = rows.filter_map(Result::ok).any(|name| name == column);
        found
    }

    /// Resolve a folder cover for an album that has no `artwork_path` in the
    /// index — e.g. an offline-cached (CMAF) album whose downloader wrote a
    /// `cover.jpg` into the track folder but didn't backfill `artwork_path`.
    /// Looks up a representative track for the metadata group, derives its
    /// containing folder (the path itself when it is a directory, as for CMAF
    /// bundles, else the parent dir), and returns `<folder>/cover.jpg` when
    /// that file exists. Frontend-agnostic (no `tauri::State`).
    pub fn resolve_album_cover_fallback(&self, group_key: &str) -> Option<String> {
        // Common on-disk cover filenames (the offline-cache writes cover.jpg;
        // ripped/local folders often use folder.jpg / front.*).
        const NAMES: [&str; 6] = [
            "cover.jpg",
            "cover.png",
            "folder.jpg",
            "Folder.jpg",
            "front.jpg",
            "front.png",
        ];
        let expr = crate::album_grouping::metadata_group_key_sql_expression();
        // Match by the metadata group key AND the raw folder key — the album
        // id depends on the Albums view's grouping mode (album|artist in
        // Metadata mode, the folder path in Folder mode). The OR keeps the
        // lookup correct under either.
        // Scan several tracks, not just one: a CMAF album keeps each track in
        // its own folder, and only some may carry a cover.jpg.
        let query = format!(
            "SELECT file_path FROM local_tracks WHERE ({expr}) = ?1 OR album_group_key = ?1 LIMIT 12"
        );
        let mut stmt = self.conn.prepare(&query).ok()?;
        let paths: Vec<String> = stmt
            .query_map(rusqlite::params![group_key], |row| row.get::<_, String>(0))
            .ok()?
            .filter_map(Result::ok)
            .collect();
        for fp in &paths {
            let p = std::path::Path::new(fp);
            // The track folder: the path itself for a CMAF bundle dir, else
            // the parent of the audio file.
            let Some(folder) = (if p.is_dir() {
                Some(p.to_path_buf())
            } else {
                p.parent().map(|x| x.to_path_buf())
            }) else {
                continue;
            };
            // Check the folder and its parent (covers multi-disc layouts where
            // the art sits one level up).
            let dirs = [
                Some(folder.clone()),
                folder.parent().map(|x| x.to_path_buf()),
            ];
            for dir in dirs.into_iter().flatten() {
                for name in NAMES {
                    let cover = dir.join(name);
                    if cover.is_file() {
                        return Some(cover.to_string_lossy().into_owned());
                    }
                }
            }
        }
        None
    }

    fn get_albums_metadata_page_inner(
        &self,
        offset: u64,
        limit: u64,
        search: Option<&str>,
        sort_by: &str,
        sort_dir: &str,
        include_qobuz_downloads: bool,
        exclude_network_folders: bool,
        plex_attached: bool,
        remote_attached: bool,
        plex_has_genres_json: bool,
        remote_has_genres_json: bool,
        remote_filter: &str,
        group_mode: crate::album_grouping::AlbumGroupMode,
    ) -> Result<crate::models::AlbumsMetadataPage, LibraryError> {
        let source_filter = if include_qobuz_downloads {
            ""
        } else {
            "AND (source IS NULL OR source != 'qobuz_download')"
        };

        let network_filter = if exclude_network_folders {
            "AND NOT EXISTS (
                SELECT 1 FROM library_folders nf
                WHERE nf.is_network = 1
                AND local_tracks.file_path LIKE nf.path || '%'
            )"
        } else {
            ""
        };

        // ORDER BY clause is built from a validated allowlist so user
        // input never reaches the SQL string directly. The unknown-album
        // sentinel always sorts last regardless of mode.
        let order_clause = match (sort_by, sort_dir) {
            ("title", "asc") => "(group_key = '__unknown_album__'), title COLLATE NOCASE",
            ("title", "desc") => "(group_key = '__unknown_album__'), title COLLATE NOCASE DESC",
            ("year", "asc") => "(group_key = '__unknown_album__'), year IS NULL, year ASC, title COLLATE NOCASE",
            ("year", "desc") => "(group_key = '__unknown_album__'), year IS NULL, year DESC, title COLLATE NOCASE",
            ("artist", "desc") => "(group_key = '__unknown_album__'), artist COLLATE NOCASE DESC, title COLLATE NOCASE",
            // Default = artist asc
            _ => "(group_key = '__unknown_album__'), artist COLLATE NOCASE, title COLLATE NOCASE",
        };

        let group_key_expr = crate::album_grouping::group_key_sql_expression(group_mode);

        let search_pattern = search.unwrap_or("").trim();
        let has_search: i64 = if search_pattern.is_empty() { 0 } else { 1 };
        let search_like = format!("%{}%", search_pattern);

        // When Plex is attached, the plex_aggregated CTE is appended
        // and the filtered set is built from the UNION of local + plex.
        // Both CTEs produce the same column shape so the UNION ALL is
        // straightforward; types are normalised via CAST in the plex
        // arm (plex stores duration_ms / sampling_rate_hz as INTEGER
        // while local uses REAL for sample_rate and seconds-INTEGER
        // for duration).
        let plex_genres_expr = if plex_has_genres_json {
            "genres_json"
        } else {
            "'[]'"
        };
        let plex_cte = if plex_attached {
            format!(
                r#",
            plex_aggregated AS (
                SELECT
                    -- `album_key` is populated by plex/mod.rs::plex_album_key()
                    -- which already returns `"plex:<hash>"`. Only the
                    -- rating_key fallback needs the prefix added.
                    COALESCE(album_key, 'plex:' || rating_key) AS group_key,
                    COALESCE(album, 'Unknown Album') AS title,
                    CASE WHEN COUNT(DISTINCT artist) > 1
                         THEN 'Various Artists'
                         ELSE COALESCE(MIN(artist), 'Unknown Artist')
                    END AS artist,
                    GROUP_CONCAT(DISTINCT artist) AS all_artists,
                    MIN(year) AS year,
                    CAST(NULL AS TEXT) AS catalog_number,
                    MAX(CASE WHEN artwork_path IS NOT NULL THEN artwork_path END) AS artwork,
                    COUNT(*) AS track_count,
                    CAST(SUM(COALESCE(duration_ms, 0)) / 1000 AS INTEGER) AS total_duration,
                    -- Plex stream-level `codec` is often missing when the
                    -- server hasn't fully analyzed a track. The `container`
                    -- field is populated for the same media and usually
                    -- carries the same value ("flac", "mp3", etc.), so it
                    -- works as a fallback. Without it, any album where
                    -- Plex didn't expose codec on every track ends up
                    -- labeled "Unknown" in the UI even though the file
                    -- format is known via container. Local CTE is not
                    -- affected — local indexing always writes a non-null
                    -- format string.
                    COALESCE(MAX(codec), MAX(container)) AS format,
                    -- Plex frequently omits bitDepth from its Media/Stream
                    -- XML for older releases; the aggregated row inherits
                    -- the gap as NULL. When the format ends up lossless
                    -- and sample rate sits at CD range (<= 48 kHz),
                    -- default to 16 — that's the universal CD-Audio /
                    -- redbook assumption that virtually every lossless
                    -- album at that rate matches. Higher rates leave the
                    -- field NULL (could be 24, could be 32) and the UI
                    -- falls back to its "--" placeholder; the per-track
                    -- view shows the real value when the user clicks in.
                    COALESCE(
                        MAX(bit_depth),
                        CASE
                            WHEN LOWER(COALESCE(MAX(codec), MAX(container))) IN
                                 ('flac', 'alac', 'wav', 'aiff', 'ape')
                                 AND MAX(sampling_rate_hz) <= 48000
                            THEN 16
                            ELSE NULL
                        END
                    ) AS bit_depth,
                    CAST(MAX(sampling_rate_hz) AS REAL) AS sample_rate,
                    json_group_array(json_array(
                        COALESCE(title, ''),
                        CAST(COALESCE(duration_ms, 0) / 1000 AS INTEGER)
                    )) AS identity_tracks,
                    json_array('plex') AS source_words,
                    json_group_array(json(COALESCE(
                        NULLIF({plex_genres_expr}, '[]'),
                        CASE WHEN genre IS NULL OR TRIM(genre) = ''
                             THEN '[]' ELSE json_array(TRIM(genre)) END
                    ))) AS genre_sets,
                    CAST(NULL AS TEXT) AS source_folders,
                    'plex' AS source
                FROM plex_cache.plex_cache_tracks
                GROUP BY COALESCE(album_key, 'plex:' || rating_key)
            )"#
            )
        } else {
            String::new()
        };

        // The SHARED remote mirror: Jellyfin, Subsonic, and whatever comes
        // next. ONE arm for all of them because the table carries a `source`
        // column — that is the entire reason it exists.
        //
        // The group key is `<source>:<album id>`, matching Plex's `plex:<hash>`
        // convention: it namespaces two servers that happen to use the same
        // album id, and it is what lets a source `claim` an album card that
        // arrives with no source word (which is every card that round-trips
        // through a QML string property).
        //
        // Unlike Plex there is no bit-depth guess here. Both protocols report
        // it directly — Jellyfin in MediaSources, Subsonic as an OpenSubsonic
        // field — and the mappers already fold "not applicable" (Jellyfin's
        // null, Subsonic's 0) to NULL. Inventing 16 for a lossless CD-rate row
        // would be guessing where the server answered.
        let remote_genres_expr = if remote_has_genres_json {
            "genres_json"
        } else {
            "'[]'"
        };
        let remote_cte = if remote_attached {
            format!(
                r#",
            remote_aggregated AS (
                SELECT
                    source || ':' || album_id AS group_key,
                    COALESCE(NULLIF(TRIM(album), ''), 'Unknown Album') AS title,
                    CASE WHEN COUNT(DISTINCT album_artist) > 1
                         THEN 'Various Artists'
                         ELSE COALESCE(NULLIF(MIN(album_artist), ''), MIN(artist), 'Unknown Artist')
                    END AS artist,
                    GROUP_CONCAT(DISTINCT artist) AS all_artists,
                    MIN(year) AS year,
                    CAST(NULL AS TEXT) AS catalog_number,
                    COALESCE(
                        MAX(CASE WHEN collection_artwork_token IS NOT NULL
                                      AND TRIM(collection_artwork_token) != ''
                                 THEN collection_artwork_token END),
                        MAX(CASE WHEN artwork_token IS NOT NULL
                                      AND TRIM(artwork_token) != ''
                                 THEN artwork_token END)
                    ) AS artwork,
                    COUNT(*) AS track_count,
                    CAST(SUM(COALESCE(duration_ms, 0)) / 1000 AS INTEGER) AS total_duration,
                    COALESCE(MAX(container), MAX(codec)) AS format,
                    MAX(bit_depth) AS bit_depth,
                    CAST(MAX(sample_rate_hz) AS REAL) AS sample_rate,
                    json_group_array(json_array(
                        COALESCE(title, ''),
                        CAST(COALESCE(duration_ms, 0) / 1000 AS INTEGER)
                    )) AS identity_tracks,
                    json_group_array(DISTINCT source) AS source_words,
                    json_group_array(json(COALESCE(
                        NULLIF({remote_genres_expr}, '[]'),
                        CASE WHEN genre IS NULL OR TRIM(genre) = ''
                             THEN '[]' ELSE json_array(TRIM(genre)) END
                    ))) AS genre_sets,
                    CAST(NULL AS TEXT) AS source_folders,
                    source AS source
                FROM remote_cache.remote_cache_tracks
                WHERE album_id != '' AND {remote_filter}
                GROUP BY source, album_id
            )"#
            )
        } else {
            String::new()
        };

        let unioned_clause = match (plex_attached, remote_attached) {
            (true, true) => {
                "SELECT * FROM aggregated \
                 UNION ALL SELECT * FROM plex_aggregated \
                 UNION ALL SELECT * FROM remote_aggregated"
            }
            (true, false) => "SELECT * FROM aggregated UNION ALL SELECT * FROM plex_aggregated",
            (false, true) => "SELECT * FROM aggregated UNION ALL SELECT * FROM remote_aggregated",
            (false, false) => "SELECT * FROM aggregated",
        };

        let query = format!(
            r#"
            WITH grouped AS (
                SELECT
                    {group_key} AS group_key,
                    -- Prefer `album` (metadata tag) over
                    -- `album_group_title` (scan-time snapshot, which
                    -- falls back to folder name if metadata is
                    -- missing). Fixes #411 — when album metadata is
                    -- valid, the folder name was winning because
                    -- COALESCE returned `album_group_title` first.
                    COALESCE(
                        NULLIF(NULLIF(TRIM(album), ''), 'Unknown Album'),
                        album_group_title,
                        'Unknown Album'
                    ) AS title,
                    COALESCE(album_artist, artist, 'Unknown Artist') AS artist,
                    year,
                    catalog_number,
                    artwork_path,
                    duration_secs,
                    format,
                    bit_depth,
                    sample_rate,
                    album_group_key AS source_folder,
                    COALESCE(source, 'user') AS source,
                    artist AS track_artist,
                    COALESCE(title, '') AS track_title,
                    COALESCE(
                        NULLIF(genres_json, '[]'),
                        CASE WHEN genre IS NULL OR TRIM(genre) = ''
                             THEN '[]' ELSE json_array(TRIM(genre)) END
                    ) AS genres_json
                FROM local_tracks
                WHERE 1=1 {source_filter} {network_filter}
            ),
            aggregated AS (
                SELECT
                    group_key,
                    CASE WHEN group_key = '__unknown_album__'
                         THEN 'Unknown Album'
                         ELSE MIN(title)
                    END AS title,
                    CASE WHEN COUNT(DISTINCT artist) > 1
                         THEN 'Various Artists'
                         ELSE MIN(artist)
                    END AS artist,
                    GROUP_CONCAT(DISTINCT track_artist) AS all_artists,
                    MIN(year) AS year,
                    MIN(catalog_number) AS catalog_number,
                    MAX(CASE WHEN artwork_path IS NOT NULL THEN artwork_path END) AS artwork,
                    COUNT(*) AS track_count,
                    SUM(duration_secs) AS total_duration,
                    MAX(format) AS format,
                    MAX(bit_depth) AS bit_depth,
                    MAX(sample_rate) AS sample_rate,
                    json_group_array(json_array(
                        track_title,
                        CAST(COALESCE(duration_secs, 0) AS INTEGER)
                    )) AS identity_tracks,
                    json_group_array(DISTINCT source) AS source_words,
                    json_group_array(json(genres_json)) AS genre_sets,
                    GROUP_CONCAT(DISTINCT source_folder) AS source_folders,
                    MAX(source) AS source
                FROM grouped
                GROUP BY group_key
            ){plex_cte}{remote_cte},
            filtered AS (
                SELECT * FROM ({unioned_clause})
                WHERE ?1 = 0 OR (title LIKE ?2 OR artist LIKE ?2)
            )
            SELECT
                group_key, title, artist, all_artists, year, catalog_number,
                artwork, track_count, total_duration, format, bit_depth,
                sample_rate, identity_tracks, source_words, genre_sets, source_folders, source,
                COUNT(*) OVER () AS total
            FROM filtered
            ORDER BY {order_clause}
            LIMIT ?3 OFFSET ?4
            "#,
            group_key = group_key_expr,
            source_filter = source_filter,
            network_filter = network_filter,
            plex_cte = plex_cte,
            unioned_clause = unioned_clause,
            order_clause = order_clause,
        );

        let mut stmt = self
            .conn
            .prepare(&query)
            .map_err(|e| LibraryError::Database(e.to_string()))?;

        let rows = stmt
            .query_map(
                rusqlite::params![has_search, search_like, limit as i64, offset as i64],
                |row| {
                    let group_key: String = row.get(0)?;
                    let title: String = row.get(1)?;
                    let artist: String = row.get(2)?;
                    let all_artists: String = row.get::<_, Option<String>>(3)?.unwrap_or_default();
                    let artwork_path: Option<String> = row.get(6)?;
                    let identity_json = row
                        .get::<_, Option<String>>(12)?
                        .unwrap_or_else(|| "[]".to_string());
                    let identity_tracks =
                        serde_json::from_str::<Vec<(String, u64)>>(&identity_json)
                            .unwrap_or_default()
                            .into_iter()
                            .map(|(title, duration_secs)| crate::models::AlbumTrackEvidence {
                                title,
                                duration_secs,
                            })
                            .collect();
                    let sources = serde_json::from_str::<Vec<String>>(
                        &row.get::<_, Option<String>>(13)?
                            .unwrap_or_else(|| "[]".to_string()),
                    )
                    .unwrap_or_default();
                    let genres =
                        Self::genres_from_sets_json(row.get::<_, Option<String>>(14)?.as_deref());
                    let source_folders: Option<String> = row.get(15)?;
                    let source = row
                        .get::<_, Option<String>>(16)?
                        .unwrap_or_else(|| "user".to_string());
                    let total: u64 = row.get::<_, i64>(17)? as u64;

                    Ok((
                        LocalAlbum {
                            id: group_key,
                            title,
                            artist,
                            all_artists,
                            year: row.get(4)?,
                            catalog_number: row.get(5)?,
                            genres,
                            artwork_path,
                            artwork_source: None,
                            track_count: row.get(7)?,
                            total_duration_secs: row.get::<_, i64>(8)? as u64,
                            format: Self::parse_format(
                                &row.get::<_, Option<String>>(9)?.unwrap_or_default(),
                            ),
                            bit_depth: row.get(10)?,
                            sample_rate: row.get::<_, Option<f64>>(11)?.unwrap_or(44100.0),
                            directory_path: String::new(),
                            source_folders,
                            sources: if sources.is_empty() {
                                vec![source.clone()]
                            } else {
                                sources
                            },
                            source,
                            identity_tracks,
                        },
                        total,
                    ))
                },
            )
            .map_err(|e| LibraryError::Database(e.to_string()))?;

        let mut albums = Vec::new();
        let mut total: u64 = 0;
        for row_result in rows {
            let (album, t) = row_result.map_err(|e| LibraryError::Database(e.to_string()))?;
            total = t;
            albums.push(album);
        }

        // Empty page (offset past the end or filter matches nothing).
        // The window-function trick gives us total only on returned
        // rows, so when there are none we have to ask separately.
        if albums.is_empty() {
            total = self.count_albums_metadata_for_page(
                search,
                include_qobuz_downloads,
                exclude_network_folders,
                plex_attached,
                remote_attached,
                remote_filter,
                group_mode,
            )?;
        }

        Ok(crate::models::AlbumsMetadataPage { albums, total })
    }

    /// Companion to `get_albums_metadata_page` — total count of albums
    /// matching the same filter. Used when the page is empty (so the
    /// window-function-derived total isn't available) or when the
    /// frontend wants to know the count before requesting any page.
    /// Honours the same Plex attachment state as the caller used.
    fn count_albums_metadata_for_page(
        &self,
        search: Option<&str>,
        include_qobuz_downloads: bool,
        exclude_network_folders: bool,
        plex_attached: bool,
        remote_attached: bool,
        remote_filter: &str,
        group_mode: crate::album_grouping::AlbumGroupMode,
    ) -> Result<u64, LibraryError> {
        let source_filter = if include_qobuz_downloads {
            ""
        } else {
            "AND (source IS NULL OR source != 'qobuz_download')"
        };
        let network_filter = if exclude_network_folders {
            "AND NOT EXISTS (
                SELECT 1 FROM library_folders nf
                WHERE nf.is_network = 1
                AND local_tracks.file_path LIKE nf.path || '%'
            )"
        } else {
            ""
        };
        let group_key_expr = crate::album_grouping::group_key_sql_expression(group_mode);
        let search_pattern = search.unwrap_or("").trim();
        let has_search: i64 = if search_pattern.is_empty() { 0 } else { 1 };
        let search_like = format!("%{}%", search_pattern);

        let plex_cte = if plex_attached {
            r#",
            plex_aggregated AS (
                SELECT
                    -- `album_key` is populated by plex/mod.rs::plex_album_key()
                    -- which already returns `"plex:<hash>"`. Only the
                    -- rating_key fallback needs the prefix added.
                    COALESCE(album_key, 'plex:' || rating_key) AS group_key,
                    COALESCE(album, 'Unknown Album') AS title,
                    COALESCE(MIN(artist), 'Unknown Artist') AS artist
                FROM plex_cache.plex_cache_tracks
                GROUP BY COALESCE(album_key, 'plex:' || rating_key)
            )"#
        } else {
            ""
        };
        // Grouped and filtered EXACTLY as the page query groups and filters
        // it — same key, same `album_id != ''` guard. A count that disagrees
        // with the page it counts is worse than no count: the grid renders a
        // scrollbar for rows that are not there.
        let remote_cte = if remote_attached {
            format!(
                r#",
            remote_aggregated AS (
                SELECT
                    source || ':' || album_id AS group_key,
                    COALESCE(NULLIF(TRIM(album), ''), 'Unknown Album') AS title,
                    COALESCE(NULLIF(MIN(album_artist), ''), MIN(artist), 'Unknown Artist') AS artist
                FROM remote_cache.remote_cache_tracks
                WHERE album_id != '' AND {remote_filter}
                GROUP BY source, album_id
            )"#
            )
        } else {
            String::new()
        };
        let unioned_clause = match (plex_attached, remote_attached) {
            (true, true) => {
                "SELECT * FROM aggregated \
                 UNION ALL SELECT * FROM plex_aggregated \
                 UNION ALL SELECT * FROM remote_aggregated"
            }
            (true, false) => "SELECT * FROM aggregated UNION ALL SELECT * FROM plex_aggregated",
            (false, true) => "SELECT * FROM aggregated UNION ALL SELECT * FROM remote_aggregated",
            (false, false) => "SELECT * FROM aggregated",
        };

        let query = format!(
            r#"
            WITH grouped AS (
                SELECT
                    {group_key} AS group_key,
                    -- Prefer `album` (metadata tag) over
                    -- `album_group_title` (scan-time snapshot, which
                    -- falls back to folder name if metadata is
                    -- missing). Fixes #411 — when album metadata is
                    -- valid, the folder name was winning because
                    -- COALESCE returned `album_group_title` first.
                    COALESCE(
                        NULLIF(NULLIF(TRIM(album), ''), 'Unknown Album'),
                        album_group_title,
                        'Unknown Album'
                    ) AS title,
                    COALESCE(album_artist, artist, 'Unknown Artist') AS artist,
                    artist AS track_artist
                FROM local_tracks
                WHERE 1=1 {source_filter} {network_filter}
            ),
            aggregated AS (
                SELECT
                    group_key,
                    CASE WHEN group_key = '__unknown_album__'
                         THEN 'Unknown Album'
                         ELSE MIN(title)
                    END AS title,
                    CASE WHEN COUNT(DISTINCT artist) > 1
                         THEN 'Various Artists'
                         ELSE MIN(artist)
                    END AS artist
                FROM grouped
                GROUP BY group_key
            ){plex_cte}{remote_cte}
            SELECT COUNT(*)
            FROM ({unioned_clause})
            WHERE ?1 = 0 OR (title LIKE ?2 OR artist LIKE ?2)
            "#,
            group_key = group_key_expr,
            source_filter = source_filter,
            network_filter = network_filter,
            plex_cte = plex_cte,
            remote_cte = remote_cte,
            unioned_clause = unioned_clause,
        );

        let total: i64 = self
            .conn
            .query_row(&query, rusqlite::params![has_search, search_like], |row| {
                row.get(0)
            })
            .map_err(|e| LibraryError::Database(e.to_string()))?;
        Ok(total as u64)
    }

    /// Get tracks for a metadata-grouped album. The `metadata_key`
    /// matches what [`Self::get_albums_metadata_grouped`] returns for
    /// the album's `id` field.
    pub fn get_album_tracks_metadata(
        &self,
        metadata_key: &str,
    ) -> Result<Vec<LocalTrack>, LibraryError> {
        let group_key_expr = crate::album_grouping::metadata_group_key_sql_expression();
        let sql = format!(
            "SELECT {cols} FROM local_tracks
             WHERE {group_key} = ?
             ORDER BY disc_number, track_number, title",
            cols = Self::TRACK_COLUMNS,
            group_key = group_key_expr,
        );

        let mut stmt = self
            .conn
            .prepare(&sql)
            .map_err(|e| LibraryError::Database(e.to_string()))?;

        let rows = stmt
            .query_map(params![metadata_key], |row| Self::row_to_track(row))
            .map_err(|e| LibraryError::Database(e.to_string()))?;

        let mut tracks = Vec::new();
        for track in rows {
            tracks.push(track.map_err(|e| LibraryError::Database(e.to_string()))?);
        }
        Ok(tracks)
    }

    /// Get all artists
    pub fn get_artists(&self) -> Result<Vec<LocalArtist>, LibraryError> {
        self.get_artists_with_filter(true, false)
    }

    /// Get all artists with filter options
    /// This filters directly in SQL to avoid N+1 query patterns
    pub fn get_artists_with_filter(
        &self,
        include_qobuz_downloads: bool,
        exclude_network_folders: bool,
    ) -> Result<Vec<LocalArtist>, LibraryError> {
        let source_filter = if include_qobuz_downloads {
            ""
        } else {
            "AND (source IS NULL OR source != 'qobuz_download')"
        };

        let network_filter = if exclude_network_folders {
            "AND NOT EXISTS (
                SELECT 1 FROM library_folders nf
                WHERE nf.is_network = 1
                AND local_tracks.file_path LIKE nf.path || '%'
            )"
        } else {
            ""
        };

        let query = format!(
            r#"
            SELECT
                COALESCE(album_artist, artist) as name,
                COUNT(DISTINCT COALESCE(album_group_key, album || '|' || COALESCE(album_artist, artist))) as album_count,
                COUNT(*) as track_count
            FROM local_tracks
            WHERE 1=1 {} {}
            GROUP BY name
            ORDER BY name
        "#,
            source_filter, network_filter
        );

        let mut stmt = self
            .conn
            .prepare(&query)
            .map_err(|e| LibraryError::Database(e.to_string()))?;

        let rows = stmt
            .query_map([], |row| {
                Ok(LocalArtist {
                    name: row.get(0)?,
                    album_count: row.get(1)?,
                    track_count: row.get(2)?,
                })
            })
            .map_err(|e| LibraryError::Database(e.to_string()))?;

        let mut artists = Vec::new();
        for artist in rows {
            artists.push(artist.map_err(|e| LibraryError::Database(e.to_string()))?);
        }
        Ok(artists)
    }

    /// Get album groups without artwork (for Discogs fetching)
    pub fn get_albums_without_artwork(
        &self,
    ) -> Result<Vec<(String, String, String)>, LibraryError> {
        let mut stmt = self
            .conn
            .prepare(
                r#"
            SELECT
                group_key,
                MIN(title) as title,
                CASE
                    WHEN COUNT(DISTINCT artist) > 1 THEN 'Various Artists'
                    ELSE MIN(artist)
                END as artist
            FROM (
                SELECT
                    COALESCE(album_group_key, album || '|' || COALESCE(album_artist, artist)) as group_key,
                    COALESCE(album_group_title, album) as title,
                    COALESCE(album_artist, artist) as artist,
                    artwork_path
                FROM local_tracks
                WHERE artwork_path IS NULL OR artwork_path = ''
            )
            GROUP BY group_key
            ORDER BY artist, title
        "#,
            )
            .map_err(|e| LibraryError::Database(e.to_string()))?;

        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .map_err(|e| LibraryError::Database(e.to_string()))?;

        let mut albums = Vec::new();
        for album in rows {
            albums.push(album.map_err(|e| LibraryError::Database(e.to_string()))?);
        }
        Ok(albums)
    }

    /// Update artwork path for all tracks in an album
    pub fn update_album_artwork(
        &self,
        album: &str,
        artist: &str,
        artwork_path: &str,
    ) -> Result<(), LibraryError> {
        self.conn
            .execute(
                r#"
            UPDATE local_tracks
            SET artwork_path = ?
            WHERE album = ? AND COALESCE(album_artist, artist) = ?
        "#,
                params![artwork_path, album, artist],
            )
            .map_err(|e| LibraryError::Database(e.to_string()))?;
        Ok(())
    }

    /// Update artwork path for all tracks in an album group.
    ///
    /// **Deprecated**: this was used inside the scan loop to backfill
    /// artwork across tracks in the same group, but it pisses every
    /// track's individual artwork in the process — destroying unique
    /// per-track embedded covers. Per-track artwork is now resolved
    /// individually at scan time. Kept compilable for any caller that
    /// might still exist; do not introduce new callers.
    #[deprecated(
        note = "Was destructive in scan loop; per-track artwork is resolved during scan instead"
    )]
    pub fn update_album_group_artwork(
        &self,
        group_key: &str,
        artwork_path: &str,
    ) -> Result<(), LibraryError> {
        self.conn
            .execute(
                r#"
            UPDATE local_tracks
            SET artwork_path = ?
            WHERE COALESCE(album_group_key, album || '|' || COALESCE(album_artist, artist)) = ?
        "#,
                params![artwork_path, group_key],
            )
            .map_err(|e| LibraryError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn update_album_group_metadata(
        &mut self,
        group_key: &str,
        album_title: &str,
        album_artist: &str,
        year: Option<u32>,
        genre: Option<&str>,
        catalog_number: Option<&str>,
        track_artist_match: Option<&str>,
        track_updates: &[AlbumTrackUpdate],
    ) -> Result<(), LibraryError> {
        let tx = self
            .conn
            .transaction()
            .map_err(|e| LibraryError::Database(e.to_string()))?;

        let normalized_album_artist = {
            let trimmed = album_artist.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        };

        tx.execute(
            r#"
            UPDATE local_tracks
            SET
                album = ?1,
                album_group_title = ?2,
                album_artist = ?3,
                year = ?4,
                genre = ?5,
                catalog_number = ?6
            WHERE COALESCE(album_group_key, album || '|' || COALESCE(album_artist, artist)) = ?7
            "#,
            params![
                album_title.trim(),
                album_title.trim(),
                normalized_album_artist,
                year,
                genre.map(|s| s.trim()).filter(|s| !s.is_empty()),
                catalog_number.map(|s| s.trim()).filter(|s| !s.is_empty()),
                group_key
            ],
        )
        .map_err(|e| LibraryError::Database(e.to_string()))?;

        if let Some(match_artist) = track_artist_match {
            let match_trim = match_artist.trim();
            if !match_trim.is_empty() && !album_artist.trim().is_empty() {
                tx.execute(
                    r#"
                    UPDATE local_tracks
                    SET artist = ?1
                    WHERE COALESCE(album_group_key, album || '|' || COALESCE(album_artist, artist)) = ?2
                      AND artist = ?3
                    "#,
                    params![album_artist.trim(), group_key, match_trim],
                )
                .map_err(|e| LibraryError::Database(e.to_string()))?;
            }
        }

        {
            let mut stmt = tx
                .prepare("UPDATE local_tracks SET title = ?1, disc_number = ?2, track_number = ?3 WHERE id = ?4")
                .map_err(|e| LibraryError::Database(e.to_string()))?;

            for update in track_updates {
                stmt.execute(params![
                    update.title.trim(),
                    update.disc_number,
                    update.track_number,
                    update.id
                ])
                .map_err(|e| LibraryError::Database(e.to_string()))?;
            }
        }

        tx.commit()
            .map_err(|e| LibraryError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn update_tracks_metadata_by_id(
        &mut self,
        updates: &[TrackMetadataUpdateFull],
    ) -> Result<(), LibraryError> {
        let tx = self
            .conn
            .transaction()
            .map_err(|e| LibraryError::Database(e.to_string()))?;

        {
            let mut stmt = tx
                .prepare(
                    r#"
                    UPDATE local_tracks
                    SET
                        title = ?1,
                        artist = ?2,
                        album = ?3,
                        album_artist = ?4,
                        album_group_title = ?5,
                        track_number = ?6,
                        disc_number = ?7,
                        year = ?8,
                        genre = ?9,
                        catalog_number = ?10
                    WHERE id = ?11
                    "#,
                )
                .map_err(|e| LibraryError::Database(e.to_string()))?;

            for update in updates {
                stmt.execute(params![
                    update.title.trim(),
                    update.artist.trim(),
                    update.album.trim(),
                    update.album_artist.as_ref().map(|s| s.trim().to_string()),
                    update.album_group_title.trim(),
                    update.track_number,
                    update.disc_number,
                    update.year,
                    update.genre.as_ref().map(|s| s.trim().to_string()),
                    update.catalog_number.as_ref().map(|s| s.trim().to_string()),
                    update.id
                ])
                .map_err(|e| LibraryError::Database(e.to_string()))?;
            }
        }

        tx.commit()
            .map_err(|e| LibraryError::Database(e.to_string()))?;
        Ok(())
    }

    /// Commit one metadata-editor draft and its optional front cover as a
    /// single exact-row transaction. A stale/deleted row aborts the entire
    /// draft instead of leaving tags, index metadata and artwork out of sync.
    pub fn update_tracks_metadata_and_artwork_by_id(
        &mut self,
        updates: &[TrackMetadataUpdateFull],
        artwork_path: Option<&str>,
    ) -> Result<(), LibraryError> {
        if updates.is_empty() {
            return Err(LibraryError::Database(
                "Metadata update requires at least one track".to_string(),
            ));
        }
        if artwork_path.is_some_and(|path| path.trim().is_empty()) {
            return Err(LibraryError::Database(
                "Artwork path cannot be blank".to_string(),
            ));
        }
        let unique = updates
            .iter()
            .map(|update| update.id)
            .collect::<std::collections::HashSet<_>>();
        if unique.len() != updates.len() {
            return Err(LibraryError::Database(
                "Metadata update contains duplicate track ids".to_string(),
            ));
        }
        let tx = self
            .conn
            .transaction()
            .map_err(|error| LibraryError::Database(error.to_string()))?;
        {
            let mut statement = tx
                .prepare(
                    r#"
                    UPDATE local_tracks
                    SET
                        title = ?1,
                        artist = ?2,
                        album = ?3,
                        album_artist = ?4,
                        album_group_title = ?5,
                        track_number = ?6,
                        disc_number = ?7,
                        year = ?8,
                        genre = ?9,
                        catalog_number = ?10,
                        artwork_path = COALESCE(?11, artwork_path)
                    WHERE id = ?12
                    "#,
                )
                .map_err(|error| LibraryError::Database(error.to_string()))?;
            for update in updates {
                let changed = statement
                    .execute(params![
                        update.title.trim(),
                        update.artist.trim(),
                        update.album.trim(),
                        update
                            .album_artist
                            .as_ref()
                            .map(|value| value.trim().to_string()),
                        update.album_group_title.trim(),
                        update.track_number,
                        update.disc_number,
                        update.year,
                        update.genre.as_ref().map(|value| value.trim().to_string()),
                        update
                            .catalog_number
                            .as_ref()
                            .map(|value| value.trim().to_string()),
                        artwork_path,
                        update.id,
                    ])
                    .map_err(|error| LibraryError::Database(error.to_string()))?;
                if changed != 1 {
                    return Err(LibraryError::Database(format!(
                        "Metadata target track {} no longer exists",
                        update.id
                    )));
                }
            }
        }
        tx.commit()
            .map_err(|error| LibraryError::Database(error.to_string()))?;
        Ok(())
    }

    /// Apply an editor-selected cover only to the snapshotted physical rows.
    /// Album-group-wide artwork updates are intentionally avoided because a
    /// multi-disc collection may carry a different cover on every disc.
    pub fn update_tracks_artwork_by_id(
        &mut self,
        ids: &[i64],
        artwork_path: &str,
    ) -> Result<(), LibraryError> {
        if ids.is_empty() || artwork_path.trim().is_empty() {
            return Err(LibraryError::Database(
                "Artwork update requires track ids and a path".to_string(),
            ));
        }
        let unique = ids
            .iter()
            .copied()
            .collect::<std::collections::HashSet<_>>();
        if unique.len() != ids.len() {
            return Err(LibraryError::Database(
                "Artwork update contains duplicate track ids".to_string(),
            ));
        }
        let tx = self
            .conn
            .transaction()
            .map_err(|error| LibraryError::Database(error.to_string()))?;
        {
            let mut statement = tx
                .prepare("UPDATE local_tracks SET artwork_path = ?1 WHERE id = ?2")
                .map_err(|error| LibraryError::Database(error.to_string()))?;
            for id in ids {
                let changed = statement
                    .execute(params![artwork_path, id])
                    .map_err(|error| LibraryError::Database(error.to_string()))?;
                if changed != 1 {
                    return Err(LibraryError::Database(format!(
                        "Artwork target track {id} no longer exists"
                    )));
                }
            }
        }
        tx.commit()
            .map_err(|error| LibraryError::Database(error.to_string()))?;
        Ok(())
    }

    pub fn find_album_group_key(
        &self,
        album: &str,
        artist: &str,
    ) -> Result<Option<String>, LibraryError> {
        self.conn
            .query_row(
                r#"
            SELECT COALESCE(album_group_key, album || '|' || COALESCE(album_artist, artist))
            FROM local_tracks
            WHERE album = ? AND COALESCE(album_artist, artist) = ?
            LIMIT 1
        "#,
                params![album, artist],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| LibraryError::Database(e.to_string()))
    }

    /// Search tracks by title, artist, or album
    pub fn search(&self, query: &str, limit: u32) -> Result<Vec<LocalTrack>, LibraryError> {
        self.search_with_filter(query, limit, true, false)
    }

    /// Search tracks with filter options
    /// This filters directly in SQL to avoid post-query filtering overhead
    pub fn search_with_filter(
        &self,
        query: &str,
        limit: u32,
        include_qobuz_downloads: bool,
        exclude_network_folders: bool,
    ) -> Result<Vec<LocalTrack>, LibraryError> {
        let pattern = format!("%{}%", query);

        let source_filter = if include_qobuz_downloads {
            ""
        } else {
            "AND (source IS NULL OR source != 'qobuz_download')"
        };

        let network_filter = if exclude_network_folders {
            "AND NOT EXISTS (
                SELECT 1 FROM library_folders nf
                WHERE nf.is_network = 1
                AND local_tracks.file_path LIKE nf.path || '%'
            )"
        } else {
            ""
        };

        // limit = 0 means no limit (fetch all)
        let limit_clause = if limit == 0 {
            String::new()
        } else {
            format!("LIMIT {}", limit)
        };

        // ORDER BY matches the album-grouped browsing the Tracks tab uses
        // by default. Sorting in SQLite is sub-100ms for 100K rows; doing it
        // in JS with localeCompare on the same volume blocks the main thread
        // for several seconds per pass.
        let sql = format!(
            "SELECT {} FROM local_tracks \
             WHERE (title LIKE ?1 OR artist LIKE ?1 OR album LIKE ?1) \
             {} {} \
             ORDER BY album COLLATE NOCASE, \
                      COALESCE(album_artist, artist) COLLATE NOCASE, \
                      disc_number, \
                      track_number, \
                      title COLLATE NOCASE \
             {}",
            Self::TRACK_COLUMNS,
            source_filter,
            network_filter,
            limit_clause
        );

        let mut stmt = self
            .conn
            .prepare(&sql)
            .map_err(|e| LibraryError::Database(e.to_string()))?;

        let rows = stmt
            .query_map(params![&pattern], |row| Self::row_to_track(row))
            .map_err(|e| LibraryError::Database(e.to_string()))?;

        let mut tracks = Vec::new();
        for track in rows {
            tracks.push(track.map_err(|e| LibraryError::Database(e.to_string()))?);
        }
        Ok(tracks)
    }

    /// Paged variant of `search_with_filter` — the performant path for the
    /// Tracks tab. Returns one page (`LIMIT`/`OFFSET`) in the `sort` order,
    /// so the frontend never materializes the whole table (the documented
    /// ~16K-row freeze). An empty `query` matches everything.
    pub fn search_with_filter_page(
        &self,
        query: &str,
        offset: u64,
        limit: u64,
        include_qobuz_downloads: bool,
        exclude_network_folders: bool,
        sort: &str,
    ) -> Result<Vec<LocalTrack>, LibraryError> {
        self.search_with_filter_page_faceted(
            query,
            offset,
            limit,
            include_qobuz_downloads,
            exclude_network_folders,
            sort,
            &[],
            false,
            &[],
            &[],
        )
    }

    pub fn search_with_filter_page_faceted(
        &self,
        query: &str,
        offset: u64,
        limit: u64,
        include_qobuz_downloads: bool,
        exclude_network_folders: bool,
        sort: &str,
        formats: &[String],
        other_formats: bool,
        quality_tiers: &[String],
        source_buckets: &[String],
    ) -> Result<Vec<LocalTrack>, LibraryError> {
        let pattern = format!("%{}%", query);
        let source_filter = if include_qobuz_downloads {
            ""
        } else {
            "AND (source IS NULL OR source != 'qobuz_download')"
        };
        let network_filter = if exclude_network_folders {
            "AND NOT EXISTS (
                SELECT 1 FROM library_folders nf
                WHERE nf.is_network = 1
                AND local_tracks.file_path LIKE nf.path || '%'
            )"
        } else {
            ""
        };
        let media_filter = track_media_filter_sql(
            "format",
            "bit_depth",
            "sample_rate",
            formats,
            other_formats,
            quality_tiers,
        );
        let source_bucket_filter = if source_buckets.is_empty() {
            String::new()
        } else {
            let mut values = Vec::new();
            if source_buckets.iter().any(|value| value == "local") {
                values.push("COALESCE(source,'local') NOT IN ('qobuz_download','qobuz_purchase')");
            }
            if source_buckets.iter().any(|value| value == "offline") {
                values.push("source IN ('qobuz_download','qobuz_purchase')");
            }
            if values.is_empty() {
                "AND 0".to_string()
            } else {
                format!("AND ({})", values.join(" OR "))
            }
        };
        // ORDER BY clause is built from a validated allowlist so user
        // input never reaches the SQL string directly. NULL years always
        // sort last regardless of direction; the fallback ("default" or
        // any unknown key) is the historical album-grouped order. Every
        // explicit key ends in `id` so LIMIT/OFFSET pagination is
        // deterministic across ties (mass ties are real: a batch scan
        // shares one indexed_at).
        let order_clause = match sort {
            "title-asc" => "title COLLATE NOCASE, artist COLLATE NOCASE, id",
            "title-desc" => "title COLLATE NOCASE DESC, artist COLLATE NOCASE, id",
            "artist-asc" => "COALESCE(album_artist, artist) COLLATE NOCASE, album COLLATE NOCASE, disc_number, track_number, id",
            "artist-desc" => "COALESCE(album_artist, artist) COLLATE NOCASE DESC, album COLLATE NOCASE, disc_number, track_number, id",
            // Internal Tracks grouping order. Group headers display the
            // performing artist, so album_artist must not lead this query.
            "group-artist" => "artist COLLATE NOCASE, album COLLATE NOCASE, title COLLATE NOCASE, id",
            "year-desc" => "year IS NULL, year DESC, album COLLATE NOCASE, disc_number, track_number, id",
            "year-asc" => "year IS NULL, year ASC, album COLLATE NOCASE, disc_number, track_number, id",
            "added-desc" => "indexed_at DESC, album COLLATE NOCASE, disc_number, track_number, id",
            // Default = the pre-sort hardcoded order (album-grouped).
            _ => "album COLLATE NOCASE, \
                  COALESCE(album_artist, artist) COLLATE NOCASE, \
                  disc_number, \
                  track_number, \
                  title COLLATE NOCASE, \
                  id",
        };
        let sql = format!(
            "SELECT {} FROM local_tracks \
             WHERE (title LIKE ?1 OR artist LIKE ?1 OR album LIKE ?1) \
             {} {} {} {} \
             ORDER BY {} \
             LIMIT ?2 OFFSET ?3",
            Self::TRACK_COLUMNS,
            source_filter,
            network_filter,
            media_filter,
            source_bucket_filter,
            order_clause,
        );
        let mut stmt = self
            .conn
            .prepare(&sql)
            .map_err(|e| LibraryError::Database(e.to_string()))?;
        let rows = stmt
            .query_map(params![&pattern, limit as i64, offset as i64], |row| {
                Self::row_to_track(row)
            })
            .map_err(|e| LibraryError::Database(e.to_string()))?;
        let mut tracks = Vec::new();
        for track in rows {
            tracks.push(track.map_err(|e| LibraryError::Database(e.to_string()))?);
        }
        Ok(tracks)
    }

    /// Cheap total local-track count — the Tracks-tab badge number without
    /// materializing the (potentially 16K-row) table. Mirrors the Tracks tab
    /// filter (include_qobuz_downloads = true, no network exclusion, no search)
    /// so the badge equals the unfiltered list length.
    pub fn count_all_local_tracks(&self) -> Result<u64, LibraryError> {
        let n: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM local_tracks", [], |row| row.get(0))
            .map_err(|e| LibraryError::Database(e.to_string()))?;
        Ok(n as u64)
    }

    /// Get library statistics
    pub fn get_stats(&self, include_qobuz_downloads: bool) -> Result<LibraryStats, LibraryError> {
        let source_filter = if include_qobuz_downloads {
            ""
        } else {
            "WHERE (source IS NULL OR source != 'qobuz_download')"
        };

        let sql = format!(
            r#"
            SELECT
                COUNT(*) as track_count,
                COUNT(DISTINCT COALESCE(album_group_key, album || '|' || COALESCE(album_artist, artist))) as album_count,
                COUNT(DISTINCT COALESCE(album_artist, artist)) as artist_count,
                COALESCE(SUM(duration_secs), 0) as total_duration,
                COALESCE(SUM(file_size_bytes), 0) as total_size
            FROM local_tracks
            {}
        "#,
            source_filter
        );

        let mut stmt = self
            .conn
            .prepare(&sql)
            .map_err(|e| LibraryError::Database(e.to_string()))?;

        stmt.query_row([], |row| {
            Ok(LibraryStats {
                track_count: row.get(0)?,
                album_count: row.get(1)?,
                artist_count: row.get(2)?,
                total_duration_secs: row.get::<_, i64>(3)? as u64,
                total_size_bytes: row.get::<_, i64>(4)? as u64,
            })
        })
        .map_err(|e| LibraryError::Database(e.to_string()))
    }

    // === Helpers ===

    /// Convert a database row to LocalTrack
    /// Column list for SELECT queries (avoids fragile SELECT * with positional indices)
    const TRACK_COLUMNS: &'static str = "id, file_path, title, artist, album, album_artist, \
         track_number, disc_number, year, genre, genres_json, duration_secs, format, \
         bit_depth, sample_rate, channels, file_size_bytes, \
         cue_file_path, cue_start_secs, cue_end_secs, artwork_path, \
         last_modified, indexed_at, album_group_key, album_group_title, \
         source, qobuz_track_id, catalog_number, is_network_mount, \
         isrc, musicbrainz_recording_id, musicbrainz_track_id, \
         musicbrainz_release_id, musicbrainz_release_group_id, musicbrainz_artist_id";

    fn track_genres_json(track: &LocalTrack) -> String {
        let mut genres = Vec::<String>::new();
        for value in track.genres.iter().chain(track.genre.iter()) {
            let value = value.trim();
            if !value.is_empty()
                && !genres
                    .iter()
                    .any(|existing| existing.eq_ignore_ascii_case(value))
            {
                genres.push(value.to_string());
            }
        }
        serde_json::to_string(&genres).unwrap_or_else(|_| "[]".to_string())
    }

    fn genres_from_json(raw: Option<&str>, primary: Option<&str>) -> Vec<String> {
        let mut genres = raw
            .and_then(|value| serde_json::from_str::<Vec<String>>(value).ok())
            .unwrap_or_default();
        if genres.is_empty() {
            if let Some(value) = primary.filter(|value| !value.trim().is_empty()) {
                genres.push(value.trim().to_string());
            }
        }
        genres
    }

    fn genres_from_sets_json(raw: Option<&str>) -> Vec<String> {
        let sets = raw
            .and_then(|value| serde_json::from_str::<Vec<Vec<String>>>(value).ok())
            .unwrap_or_default();
        let mut genres = Vec::<String>::new();
        for value in sets.into_iter().flatten() {
            let value = value.trim();
            if !value.is_empty()
                && !genres
                    .iter()
                    .any(|existing| existing.eq_ignore_ascii_case(value))
            {
                genres.push(value.to_string());
            }
        }
        genres.sort_by_key(|value| value.to_lowercase());
        genres
    }

    fn row_to_track(row: &rusqlite::Row) -> rusqlite::Result<LocalTrack> {
        let genre: Option<String> = row.get(9)?;
        let genres = Self::genres_from_json(
            row.get::<_, Option<String>>(10)?.as_deref(),
            genre.as_deref(),
        );
        Ok(LocalTrack {
            id: row.get(0)?,           // id
            file_path: row.get(1)?,    // file_path
            title: row.get(2)?,        // title
            artist: row.get(3)?,       // artist
            album: row.get(4)?,        // album
            album_artist: row.get(5)?, // album_artist
            track_number: row.get(6)?, // track_number
            disc_number: row.get(7)?,  // disc_number
            year: row.get(8)?,         // year
            genre,
            genres,
            duration_secs: row.get::<_, i64>(11)? as u64, // duration_secs
            format: Self::parse_format(&row.get::<_, String>(12)?), // format
            bit_depth: row.get(13)?,                      // bit_depth
            sample_rate: row.get::<_, f64>(14)?,          // sample_rate
            channels: row.get(15)?,                       // channels
            file_size_bytes: row.get::<_, i64>(16)? as u64, // file_size_bytes
            cue_file_path: row.get(17)?,                  // cue_file_path
            cue_start_secs: row.get(18)?,                 // cue_start_secs
            cue_end_secs: row.get(19)?,                   // cue_end_secs
            artwork_path: row.get(20)?,                   // artwork_path
            collection_artwork_path: None,
            last_modified: row.get(21)?, // last_modified
            indexed_at: row.get(22)?,    // indexed_at
            album_group_key: row.get::<_, Option<String>>(23)?.unwrap_or_default(), // album_group_key
            album_group_title: row.get::<_, Option<String>>(24)?.unwrap_or_default(), // album_group_title
            source: row.get(25).ok().flatten(),                                       // source
            qobuz_track_id: row.get(26).ok().flatten(), // qobuz_track_id
            catalog_number: row.get(27).ok().flatten(), // catalog_number
            isrc: row.get(29).ok().flatten(),
            musicbrainz_recording_id: row.get(30).ok().flatten(),
            musicbrainz_track_id: row.get(31).ok().flatten(),
            musicbrainz_release_id: row.get(32).ok().flatten(),
            musicbrainz_release_group_id: row.get(33).ok().flatten(),
            musicbrainz_artist_id: row.get(34).ok().flatten(),
            is_network_mount: row
                .get::<_, Option<i64>>(28)
                .ok()
                .flatten()
                .map(|v| v != 0)
                .unwrap_or(false),
        })
    }

    /// Parse format string to AudioFormat
    /// Inverse of `AudioFormat`'s `Display` (`models.rs:24`), which is what
    /// the scanner stores. It MUST answer every variant that `Display` can
    /// write: an unlisted one folds to `Unknown` silently and the format is
    /// destroyed on the way out of the DB, not on the way in.
    ///
    /// `"DSD"` was exactly that hole. `MetadataExtractor::detect_format` has
    /// mapped `.dsf`/`.dff` to `AudioFormat::Dsd` since DSD landed
    /// (`metadata.rs:824`) and the rows on disk say `DSD` — but every read
    /// handed back `Unknown`, so a DSD track showed "UNKNOWN" as its format,
    /// took the CD quality badge (depth 1 is a KNOWN depth < 24) and printed
    /// "1-bit / 2822.4 kHz". The now-playing bar was right all along because
    /// it reads the decoder, not the row.
    fn parse_format(s: &str) -> AudioFormat {
        match s.to_uppercase().as_str() {
            "FLAC" => AudioFormat::Flac,
            "ALAC" => AudioFormat::Alac,
            "WAV" => AudioFormat::Wav,
            "AIFF" => AudioFormat::Aiff,
            "APE" => AudioFormat::Ape,
            "MP3" => AudioFormat::Mp3,
            "DSD" => AudioFormat::Dsd,
            _ => AudioFormat::Unknown,
        }
    }
}

/// Allowlisted SQL for the Local Library quality/format funnel. Values never
/// enter SQL; callers can only turn these fixed predicates on or off.
fn track_media_filter_sql(
    format_col: &str,
    depth_col: &str,
    rate_col: &str,
    formats: &[String],
    other_formats: bool,
    quality_tiers: &[String],
) -> String {
    let known = ["flac", "alac", "ape", "wav", "mp3", "aac"];
    let selected = known
        .iter()
        .copied()
        .filter(|value| {
            formats
                .iter()
                .any(|candidate| candidate.eq_ignore_ascii_case(value))
        })
        .collect::<Vec<_>>();
    let mut clauses = Vec::new();
    if !selected.is_empty() || other_formats {
        let mut formats_sql = selected
            .into_iter()
            .map(|value| format!("LOWER({format_col})='{value}'"))
            .collect::<Vec<_>>();
        if other_formats {
            formats_sql.push(format!(
                "LOWER({format_col}) NOT IN ('flac','alac','ape','wav','mp3','aac')"
            ));
        }
        clauses.push(format!("AND ({})", formats_sql.join(" OR ")));
    }
    let hires = quality_tiers.iter().any(|value| value == "hires");
    let cd = quality_tiers.iter().any(|value| value == "cd");
    let lossy = quality_tiers.iter().any(|value| value == "lossy");
    if hires || cd || lossy {
        let khz = format!(
            "CASE WHEN COALESCE({rate_col},0)>=1000 THEN COALESCE({rate_col},0)/1000.0 ELSE COALESCE({rate_col},0) END"
        );
        let mut quality = Vec::new();
        if hires {
            quality.push(format!(
                "(LOWER({format_col}) IN ('dsd','dsf','dff') OR COALESCE({depth_col},0)>=24)"
            ));
        }
        if cd {
            quality.push(format!(
                "(LOWER({format_col}) NOT IN ('mp3','dsd','dsf','dff') AND (({depth_col} IS NOT NULL AND {depth_col}<24) OR ({depth_col} IS NULL AND {khz}>=44.1)))"
            ));
        }
        if lossy {
            quality.push(format!("LOWER({format_col})='mp3'"));
        }
        clauses.push(format!("AND ({})", quality.join(" OR ")));
    }
    clauses.join(" ")
}

/// Escape `%`, `_` and `\` characters so the input can be embedded as a
/// LIKE pattern fragment. Pair with `LIKE ?n || '/%' ESCAPE '\'` at the
/// SQL site. Used by [`LibraryDatabase::list_folder_children`] and
/// [`LibraryDatabase::list_folder_tracks`] to defend against
/// pattern-injection on filesystem paths that legitimately contain
/// metacharacters (a track named `100%.flac`, a folder containing
/// `_intro_`, etc.).
fn escape_like_pattern(input: &str) -> String {
    input
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

/// Library statistics
#[derive(Debug, Clone, serde::Serialize)]
pub struct LibraryStats {
    pub track_count: u32,
    pub album_count: u32,
    pub artist_count: u32,
    pub total_duration_secs: u64,
    pub total_size_bytes: u64,
}

/// Library folder with metadata
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryFolder {
    pub id: i64,
    pub path: String,
    pub alias: Option<String>,
    pub enabled: bool,
    pub is_network: bool,
    pub network_fs_type: Option<String>,
    pub user_override_network: bool,
    pub last_scan: Option<i64>,
}

/// Playlist local settings (enhances remote Qobuz playlists)
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PlaylistSettings {
    pub qobuz_playlist_id: u64,
    pub custom_artwork_path: Option<String>,
    pub sort_by: String,
    pub sort_order: String,
    pub last_search_query: Option<String>,
    pub notes: Option<String>,
    pub hidden: bool,
    pub position: i32,
    pub has_local_content: LocalContentStatus,
    pub is_favorite: bool,
    pub folder_id: Option<String>, // ID of the folder this playlist belongs to (null = root)
    pub created_at: i64,
    pub updated_at: i64,
}

/// Status of local content availability for a playlist
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalContentStatus {
    Unknown,
    No,
    SomeLocal,
    AllLocal,
}

impl Default for LocalContentStatus {
    fn default() -> Self {
        Self::Unknown
    }
}

impl LocalContentStatus {
    pub fn from_str(s: &str) -> Self {
        match s {
            "no" => Self::No,
            "some_local" => Self::SomeLocal,
            "all_local" => Self::AllLocal,
            _ => Self::Unknown,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::No => "no",
            Self::SomeLocal => "some_local",
            Self::AllLocal => "all_local",
        }
    }
}

/// Playlist statistics
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PlaylistStats {
    pub qobuz_playlist_id: u64,
    pub play_count: u32,
    pub last_played_at: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Playlist folder for organizing playlists locally
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PlaylistFolder {
    pub id: String,
    pub name: String,
    pub icon_type: String,   // "preset" or "custom"
    pub icon_preset: String, // lucide icon name
    pub icon_color: String,  // hex color
    pub custom_image_path: Option<String>,
    pub is_hidden: bool,
    pub position: i32,
    pub created_at: i64,
    pub updated_at: i64,
}

impl Default for PlaylistSettings {
    fn default() -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        Self {
            qobuz_playlist_id: 0,
            custom_artwork_path: None,
            sort_by: "default".to_string(),
            sort_order: "asc".to_string(),
            last_search_query: None,
            notes: None,
            hidden: false,
            position: 0,
            has_local_content: LocalContentStatus::Unknown,
            is_favorite: false,
            folder_id: None,
            created_at: now,
            updated_at: now,
        }
    }
}

impl Default for PlaylistStats {
    fn default() -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        Self {
            qobuz_playlist_id: 0,
            play_count: 0,
            last_played_at: None,
            created_at: now,
            updated_at: now,
        }
    }
}

impl LibraryDatabase {
    // === Playlist Settings ===

    /// Get playlist settings by Qobuz playlist ID
    pub fn get_playlist_settings(
        &self,
        qobuz_playlist_id: u64,
    ) -> Result<Option<PlaylistSettings>, LibraryError> {
        let result = self.conn.query_row(
            "SELECT qobuz_playlist_id, custom_artwork_path, sort_by, sort_order,
                    last_search_query, notes, hidden, position, has_local_content, is_favorite, folder_id, created_at, updated_at
             FROM playlist_settings WHERE qobuz_playlist_id = ?1",
            params![qobuz_playlist_id as i64],
            |row| {
                Ok(PlaylistSettings {
                    qobuz_playlist_id: row.get::<_, i64>(0)? as u64,
                    custom_artwork_path: row.get(1)?,
                    sort_by: row.get(2)?,
                    sort_order: row.get(3)?,
                    last_search_query: row.get(4)?,
                    notes: row.get(5)?,
                    hidden: row.get::<_, i32>(6)? != 0,
                    position: row.get(7)?,
                    has_local_content: LocalContentStatus::from_str(&row.get::<_, Option<String>>(8)?.unwrap_or_default()),
                    is_favorite: row.get::<_, i32>(9).unwrap_or(0) != 0,
                    folder_id: row.get(10)?,
                    created_at: row.get(11)?,
                    updated_at: row.get(12)?,
                })
            },
        ).optional()
        .map_err(|e| LibraryError::Database(format!("Failed to get playlist settings: {}", e)))?;

        Ok(result)
    }

    /// Save or update playlist settings
    pub fn save_playlist_settings(&self, settings: &PlaylistSettings) -> Result<(), LibraryError> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);

        self.conn.execute(
            "INSERT INTO playlist_settings
                (qobuz_playlist_id, custom_artwork_path, sort_by, sort_order,
                 last_search_query, notes, hidden, position, has_local_content, is_favorite, folder_id, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
             ON CONFLICT(qobuz_playlist_id) DO UPDATE SET
                custom_artwork_path = excluded.custom_artwork_path,
                sort_by = excluded.sort_by,
                sort_order = excluded.sort_order,
                last_search_query = excluded.last_search_query,
                notes = excluded.notes,
                hidden = excluded.hidden,
                position = excluded.position,
                has_local_content = excluded.has_local_content,
                is_favorite = excluded.is_favorite,
                folder_id = excluded.folder_id,
                updated_at = excluded.updated_at",
            params![
                settings.qobuz_playlist_id as i64,
                &settings.custom_artwork_path,
                &settings.sort_by,
                &settings.sort_order,
                &settings.last_search_query,
                &settings.notes,
                settings.hidden as i32,
                settings.position,
                settings.has_local_content.as_str(),
                settings.is_favorite as i32,
                &settings.folder_id,
                settings.created_at,
                now,
            ],
        ).map_err(|e| LibraryError::Database(format!("Failed to save playlist settings: {}", e)))?;

        Ok(())
    }

    /// Update just the sort settings for a playlist
    pub fn update_playlist_sort(
        &self,
        qobuz_playlist_id: u64,
        sort_by: &str,
        sort_order: &str,
    ) -> Result<(), LibraryError> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);

        // First check if settings exist, if not create default
        let existing = self.get_playlist_settings(qobuz_playlist_id)?;
        if existing.is_none() {
            let mut settings = PlaylistSettings::default();
            settings.qobuz_playlist_id = qobuz_playlist_id;
            settings.sort_by = sort_by.to_string();
            settings.sort_order = sort_order.to_string();
            return self.save_playlist_settings(&settings);
        }

        self.conn
            .execute(
                "UPDATE playlist_settings SET sort_by = ?1, sort_order = ?2, updated_at = ?3
             WHERE qobuz_playlist_id = ?4",
                params![sort_by, sort_order, now, qobuz_playlist_id as i64],
            )
            .map_err(|e| {
                LibraryError::Database(format!("Failed to update playlist sort: {}", e))
            })?;

        Ok(())
    }

    /// Update custom artwork path for a playlist
    pub fn update_playlist_artwork(
        &self,
        qobuz_playlist_id: u64,
        artwork_path: Option<&str>,
    ) -> Result<(), LibraryError> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);

        // First check if settings exist, if not create default
        let existing = self.get_playlist_settings(qobuz_playlist_id)?;
        if existing.is_none() {
            let mut settings = PlaylistSettings::default();
            settings.qobuz_playlist_id = qobuz_playlist_id;
            settings.custom_artwork_path = artwork_path.map(|s| s.to_string());
            return self.save_playlist_settings(&settings);
        }

        self.conn
            .execute(
                "UPDATE playlist_settings SET custom_artwork_path = ?1, updated_at = ?2
             WHERE qobuz_playlist_id = ?3",
                params![artwork_path, now, qobuz_playlist_id as i64],
            )
            .map_err(|e| {
                LibraryError::Database(format!("Failed to update playlist artwork: {}", e))
            })?;

        Ok(())
    }

    /// Update last search query for a playlist
    pub fn update_playlist_search_query(
        &self,
        qobuz_playlist_id: u64,
        query: Option<&str>,
    ) -> Result<(), LibraryError> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);

        // First check if settings exist, if not create default
        let existing = self.get_playlist_settings(qobuz_playlist_id)?;
        if existing.is_none() {
            let mut settings = PlaylistSettings::default();
            settings.qobuz_playlist_id = qobuz_playlist_id;
            settings.last_search_query = query.map(|s| s.to_string());
            return self.save_playlist_settings(&settings);
        }

        self.conn
            .execute(
                "UPDATE playlist_settings SET last_search_query = ?1, updated_at = ?2
             WHERE qobuz_playlist_id = ?3",
                params![query, now, qobuz_playlist_id as i64],
            )
            .map_err(|e| {
                LibraryError::Database(format!("Failed to update playlist search query: {}", e))
            })?;

        Ok(())
    }

    /// Delete playlist settings
    pub fn delete_playlist_settings(&self, qobuz_playlist_id: u64) -> Result<(), LibraryError> {
        self.conn
            .execute(
                "DELETE FROM playlist_settings WHERE qobuz_playlist_id = ?1",
                params![qobuz_playlist_id as i64],
            )
            .map_err(|e| {
                LibraryError::Database(format!("Failed to delete playlist settings: {}", e))
            })?;

        Ok(())
    }

    /// Get all playlist settings (for syncing/export)
    pub fn get_all_playlist_settings(&self) -> Result<Vec<PlaylistSettings>, LibraryError> {
        let mut stmt = self.conn.prepare(
            "SELECT qobuz_playlist_id, custom_artwork_path, sort_by, sort_order,
                    last_search_query, notes, hidden, position, has_local_content, is_favorite, folder_id, created_at, updated_at
             FROM playlist_settings ORDER BY position ASC, updated_at DESC"
        ).map_err(|e| LibraryError::Database(format!("Failed to prepare statement: {}", e)))?;

        let settings = stmt
            .query_map([], |row| {
                Ok(PlaylistSettings {
                    qobuz_playlist_id: row.get::<_, i64>(0)? as u64,
                    custom_artwork_path: row.get(1)?,
                    sort_by: row.get(2)?,
                    sort_order: row.get(3)?,
                    last_search_query: row.get(4)?,
                    notes: row.get(5)?,
                    hidden: row.get::<_, i32>(6)? != 0,
                    position: row.get(7)?,
                    has_local_content: LocalContentStatus::from_str(
                        &row.get::<_, Option<String>>(8)?.unwrap_or_default(),
                    ),
                    is_favorite: row.get::<_, i32>(9).unwrap_or(0) != 0,
                    folder_id: row.get(10)?,
                    created_at: row.get(11)?,
                    updated_at: row.get(12)?,
                })
            })
            .map_err(|e| {
                LibraryError::Database(format!("Failed to query playlist settings: {}", e))
            })?;

        settings.collect::<Result<Vec<_>, _>>().map_err(|e| {
            LibraryError::Database(format!("Failed to collect playlist settings: {}", e))
        })
    }

    /// Update hidden status for a playlist
    pub fn set_playlist_hidden(
        &self,
        qobuz_playlist_id: u64,
        hidden: bool,
    ) -> Result<(), LibraryError> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);

        // First check if settings exist, if not create default
        let existing = self.get_playlist_settings(qobuz_playlist_id)?;
        if existing.is_none() {
            let mut settings = PlaylistSettings::default();
            settings.qobuz_playlist_id = qobuz_playlist_id;
            settings.hidden = hidden;
            return self.save_playlist_settings(&settings);
        }

        self.conn
            .execute(
                "UPDATE playlist_settings SET hidden = ?1, updated_at = ?2
             WHERE qobuz_playlist_id = ?3",
                params![hidden as i32, now, qobuz_playlist_id as i64],
            )
            .map_err(|e| {
                LibraryError::Database(format!("Failed to update playlist hidden: {}", e))
            })?;

        Ok(())
    }

    /// Update favorite status for a playlist
    pub fn set_playlist_favorite(
        &self,
        qobuz_playlist_id: u64,
        favorite: bool,
    ) -> Result<(), LibraryError> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);

        // First check if settings exist, if not create default
        let existing = self.get_playlist_settings(qobuz_playlist_id)?;
        if existing.is_none() {
            let mut settings = PlaylistSettings::default();
            settings.qobuz_playlist_id = qobuz_playlist_id;
            settings.is_favorite = favorite;
            return self.save_playlist_settings(&settings);
        }

        self.conn
            .execute(
                "UPDATE playlist_settings SET is_favorite = ?1, updated_at = ?2
             WHERE qobuz_playlist_id = ?3",
                params![favorite as i32, now, qobuz_playlist_id as i64],
            )
            .map_err(|e| {
                LibraryError::Database(format!("Failed to update playlist favorite: {}", e))
            })?;

        Ok(())
    }

    /// Get all playlist IDs that are marked as favorites
    pub fn get_favorite_playlist_ids(&self) -> Result<Vec<u64>, LibraryError> {
        let mut stmt = self.conn.prepare(
            "SELECT qobuz_playlist_id FROM playlist_settings WHERE is_favorite = 1 ORDER BY updated_at DESC"
        ).map_err(|e| LibraryError::Database(format!("Failed to prepare statement: {}", e)))?;

        let ids = stmt
            .query_map([], |row| Ok(row.get::<_, i64>(0)? as u64))
            .map_err(|e| {
                LibraryError::Database(format!("Failed to query favorite playlists: {}", e))
            })?;

        ids.collect::<Result<Vec<_>, _>>().map_err(|e| {
            LibraryError::Database(format!("Failed to collect favorite playlist IDs: {}", e))
        })
    }

    /// Record that a Qobuz playlist (by its SOURCE id) was copied into the
    /// user's library. Idempotent — re-copying the same source is a no-op.
    pub fn mark_playlist_copied(&self, qobuz_playlist_id: u64) -> Result<(), LibraryError> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        self.conn
            .execute(
                "INSERT OR IGNORE INTO copied_playlists (qobuz_playlist_id, copied_at) VALUES (?1, ?2)",
                params![qobuz_playlist_id as i64, now],
            )
            .map_err(|e| {
                LibraryError::Database(format!("Failed to mark playlist copied: {}", e))
            })?;
        Ok(())
    }

    /// Whether a Qobuz playlist (by its SOURCE id) has already been copied into
    /// the user's library — used to hide the Copy button on its detail view.
    pub fn is_playlist_copied(&self, qobuz_playlist_id: u64) -> Result<bool, LibraryError> {
        let count: i64 = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM copied_playlists WHERE qobuz_playlist_id = ?1",
                params![qobuz_playlist_id as i64],
                |row| row.get(0),
            )
            .map_err(|e| {
                LibraryError::Database(format!("Failed to check copied playlist: {}", e))
            })?;
        Ok(count > 0)
    }

    /// Update position for a playlist
    pub fn set_playlist_position(
        &self,
        qobuz_playlist_id: u64,
        position: i32,
    ) -> Result<(), LibraryError> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);

        // First check if settings exist, if not create default
        let existing = self.get_playlist_settings(qobuz_playlist_id)?;
        if existing.is_none() {
            let mut settings = PlaylistSettings::default();
            settings.qobuz_playlist_id = qobuz_playlist_id;
            settings.position = position;
            return self.save_playlist_settings(&settings);
        }

        self.conn
            .execute(
                "UPDATE playlist_settings SET position = ?1, updated_at = ?2
             WHERE qobuz_playlist_id = ?3",
                params![position, now, qobuz_playlist_id as i64],
            )
            .map_err(|e| {
                LibraryError::Database(format!("Failed to update playlist position: {}", e))
            })?;

        Ok(())
    }

    /// Bulk reorder playlists by setting positions
    pub fn reorder_playlists(&self, playlist_ids: &[u64]) -> Result<(), LibraryError> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);

        for (index, &playlist_id) in playlist_ids.iter().enumerate() {
            // Ensure settings exist first
            let existing = self.get_playlist_settings(playlist_id)?;
            if existing.is_none() {
                let mut settings = PlaylistSettings::default();
                settings.qobuz_playlist_id = playlist_id;
                settings.position = index as i32;
                self.save_playlist_settings(&settings)?;
            } else {
                self.conn
                    .execute(
                        "UPDATE playlist_settings SET position = ?1, updated_at = ?2
                     WHERE qobuz_playlist_id = ?3",
                        params![index as i32, now, playlist_id as i64],
                    )
                    .map_err(|e| {
                        LibraryError::Database(format!("Failed to reorder playlists: {}", e))
                    })?;
            }
        }

        Ok(())
    }

    // === Playlist Stats ===

    /// Get playlist stats
    pub fn get_playlist_stats(
        &self,
        qobuz_playlist_id: u64,
    ) -> Result<Option<PlaylistStats>, LibraryError> {
        let result = self
            .conn
            .query_row(
                "SELECT qobuz_playlist_id, play_count, last_played_at, created_at, updated_at
             FROM playlist_stats WHERE qobuz_playlist_id = ?1",
                params![qobuz_playlist_id as i64],
                |row| {
                    Ok(PlaylistStats {
                        qobuz_playlist_id: row.get::<_, i64>(0)? as u64,
                        play_count: row.get::<_, i32>(1)? as u32,
                        last_played_at: row.get(2)?,
                        created_at: row.get(3)?,
                        updated_at: row.get(4)?,
                    })
                },
            )
            .optional()
            .map_err(|e| LibraryError::Database(format!("Failed to get playlist stats: {}", e)))?;

        Ok(result)
    }

    /// Increment play count and update last_played_at for a playlist
    pub fn increment_playlist_play_count(
        &self,
        qobuz_playlist_id: u64,
    ) -> Result<PlaylistStats, LibraryError> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);

        // Try to update existing, if none exists, insert new
        let existing = self.get_playlist_stats(qobuz_playlist_id)?;

        if let Some(mut stats) = existing {
            stats.play_count += 1;
            stats.last_played_at = Some(now);
            stats.updated_at = now;

            self.conn.execute(
                "UPDATE playlist_stats SET play_count = ?1, last_played_at = ?2, updated_at = ?3
                 WHERE qobuz_playlist_id = ?4",
                params![stats.play_count as i32, now, now, qobuz_playlist_id as i64],
            ).map_err(|e| LibraryError::Database(format!("Failed to increment play count: {}", e)))?;

            Ok(stats)
        } else {
            let stats = PlaylistStats {
                qobuz_playlist_id,
                play_count: 1,
                last_played_at: Some(now),
                created_at: now,
                updated_at: now,
            };

            self.conn.execute(
                "INSERT INTO playlist_stats (qobuz_playlist_id, play_count, last_played_at, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![qobuz_playlist_id as i64, 1, now, now, now],
            ).map_err(|e| LibraryError::Database(format!("Failed to create playlist stats: {}", e)))?;

            Ok(stats)
        }
    }

    /// Get all playlist stats (for sorting by play count)
    pub fn get_all_playlist_stats(&self) -> Result<Vec<PlaylistStats>, LibraryError> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT qobuz_playlist_id, play_count, last_played_at, created_at, updated_at
             FROM playlist_stats ORDER BY play_count DESC",
            )
            .map_err(|e| LibraryError::Database(format!("Failed to prepare statement: {}", e)))?;

        let stats = stmt
            .query_map([], |row| {
                Ok(PlaylistStats {
                    qobuz_playlist_id: row.get::<_, i64>(0)? as u64,
                    play_count: row.get::<_, i32>(1)? as u32,
                    last_played_at: row.get(2)?,
                    created_at: row.get(3)?,
                    updated_at: row.get(4)?,
                })
            })
            .map_err(|e| {
                LibraryError::Database(format!("Failed to query playlist stats: {}", e))
            })?;

        stats
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| LibraryError::Database(format!("Failed to collect playlist stats: {}", e)))
    }

    // === Playlist Folders ===

    /// Create a new playlist folder
    pub fn create_playlist_folder(
        &self,
        name: &str,
        icon_type: Option<&str>,
        icon_preset: Option<&str>,
        icon_color: Option<&str>,
    ) -> Result<PlaylistFolder, LibraryError> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);

        let id = uuid::Uuid::new_v4().to_string();

        // Get the next position
        let max_position: i32 = self
            .conn
            .query_row(
                "SELECT COALESCE(MAX(position), -1) FROM playlist_folders",
                [],
                |row| row.get(0),
            )
            .unwrap_or(-1);

        let folder = PlaylistFolder {
            id: id.clone(),
            name: name.to_string(),
            icon_type: icon_type.unwrap_or("preset").to_string(),
            icon_preset: icon_preset.unwrap_or("folder").to_string(),
            icon_color: icon_color.unwrap_or("#6366f1").to_string(),
            custom_image_path: None,
            is_hidden: false,
            position: max_position + 1,
            created_at: now,
            updated_at: now,
        };

        self.conn.execute(
            "INSERT INTO playlist_folders (id, name, icon_type, icon_preset, icon_color, custom_image_path, is_hidden, position, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                &folder.id,
                &folder.name,
                &folder.icon_type,
                &folder.icon_preset,
                &folder.icon_color,
                &folder.custom_image_path,
                folder.is_hidden as i32,
                folder.position,
                folder.created_at,
                folder.updated_at,
            ],
        ).map_err(|e| LibraryError::Database(format!("Failed to create playlist folder: {}", e)))?;

        Ok(folder)
    }

    /// Get all playlist folders
    pub fn get_all_playlist_folders(&self) -> Result<Vec<PlaylistFolder>, LibraryError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, icon_type, icon_preset, icon_color, custom_image_path, is_hidden, position, created_at, updated_at
             FROM playlist_folders ORDER BY position ASC"
        ).map_err(|e| LibraryError::Database(format!("Failed to prepare statement: {}", e)))?;

        let folders = stmt
            .query_map([], |row| {
                Ok(PlaylistFolder {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    icon_type: row.get(2)?,
                    icon_preset: row.get(3)?,
                    icon_color: row.get(4)?,
                    custom_image_path: row.get(5)?,
                    is_hidden: row.get::<_, i32>(6)? != 0,
                    position: row.get(7)?,
                    created_at: row.get(8)?,
                    updated_at: row.get(9)?,
                })
            })
            .map_err(|e| {
                LibraryError::Database(format!("Failed to query playlist folders: {}", e))
            })?;

        folders.collect::<Result<Vec<_>, _>>().map_err(|e| {
            LibraryError::Database(format!("Failed to collect playlist folders: {}", e))
        })
    }

    /// Get a playlist folder by ID
    pub fn get_playlist_folder(
        &self,
        folder_id: &str,
    ) -> Result<Option<PlaylistFolder>, LibraryError> {
        let result = self.conn.query_row(
            "SELECT id, name, icon_type, icon_preset, icon_color, custom_image_path, is_hidden, position, created_at, updated_at
             FROM playlist_folders WHERE id = ?1",
            params![folder_id],
            |row| {
                Ok(PlaylistFolder {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    icon_type: row.get(2)?,
                    icon_preset: row.get(3)?,
                    icon_color: row.get(4)?,
                    custom_image_path: row.get(5)?,
                    is_hidden: row.get::<_, i32>(6)? != 0,
                    position: row.get(7)?,
                    created_at: row.get(8)?,
                    updated_at: row.get(9)?,
                })
            },
        ).optional()
        .map_err(|e| LibraryError::Database(format!("Failed to get playlist folder: {}", e)))?;

        Ok(result)
    }

    /// Update a playlist folder
    pub fn update_playlist_folder(
        &self,
        folder_id: &str,
        name: Option<&str>,
        icon_type: Option<&str>,
        icon_preset: Option<&str>,
        icon_color: Option<&str>,
        custom_image_path: Option<Option<&str>>,
        is_hidden: Option<bool>,
    ) -> Result<PlaylistFolder, LibraryError> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);

        // Get existing folder
        let existing = self
            .get_playlist_folder(folder_id)?
            .ok_or_else(|| LibraryError::Database("Folder not found".to_string()))?;

        let new_name = name.unwrap_or(&existing.name);
        let new_icon_type = icon_type.unwrap_or(&existing.icon_type);
        let new_icon_preset = icon_preset.unwrap_or(&existing.icon_preset);
        let new_icon_color = icon_color.unwrap_or(&existing.icon_color);
        let new_custom_image_path =
            custom_image_path.unwrap_or(existing.custom_image_path.as_deref());
        let new_is_hidden = is_hidden.unwrap_or(existing.is_hidden);

        self.conn.execute(
            "UPDATE playlist_folders SET name = ?1, icon_type = ?2, icon_preset = ?3, icon_color = ?4,
             custom_image_path = ?5, is_hidden = ?6, updated_at = ?7 WHERE id = ?8",
            params![
                new_name,
                new_icon_type,
                new_icon_preset,
                new_icon_color,
                new_custom_image_path,
                new_is_hidden as i32,
                now,
                folder_id,
            ],
        ).map_err(|e| LibraryError::Database(format!("Failed to update playlist folder: {}", e)))?;

        self.get_playlist_folder(folder_id)?
            .ok_or_else(|| LibraryError::Database("Folder not found after update".to_string()))
    }

    /// Delete a playlist folder (playlists return to root via ON DELETE SET NULL)
    pub fn delete_playlist_folder(&self, folder_id: &str) -> Result<(), LibraryError> {
        self.conn
            .execute(
                "DELETE FROM playlist_folders WHERE id = ?1",
                params![folder_id],
            )
            .map_err(|e| {
                LibraryError::Database(format!("Failed to delete playlist folder: {}", e))
            })?;

        Ok(())
    }

    /// Reorder playlist folders
    pub fn reorder_playlist_folders(&self, folder_ids: &[String]) -> Result<(), LibraryError> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);

        for (position, folder_id) in folder_ids.iter().enumerate() {
            self.conn
                .execute(
                    "UPDATE playlist_folders SET position = ?1, updated_at = ?2 WHERE id = ?3",
                    params![position as i32, now, folder_id],
                )
                .map_err(|e| LibraryError::Database(format!("Failed to reorder folder: {}", e)))?;
        }

        Ok(())
    }

    /// Move a playlist to a folder (or root if folder_id is None)
    pub fn move_playlist_to_folder(
        &self,
        qobuz_playlist_id: u64,
        folder_id: Option<&str>,
    ) -> Result<(), LibraryError> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);

        // First check if settings exist, if not create default
        let existing = self.get_playlist_settings(qobuz_playlist_id)?;
        if existing.is_none() {
            let mut settings = PlaylistSettings::default();
            settings.qobuz_playlist_id = qobuz_playlist_id;
            settings.folder_id = folder_id.map(|s| s.to_string());
            return self.save_playlist_settings(&settings);
        }

        self.conn.execute(
            "UPDATE playlist_settings SET folder_id = ?1, updated_at = ?2 WHERE qobuz_playlist_id = ?3",
            params![folder_id, now, qobuz_playlist_id as i64],
        ).map_err(|e| LibraryError::Database(format!("Failed to move playlist to folder: {}", e)))?;

        Ok(())
    }

    /// Get playlists in a specific folder (or root if folder_id is None)
    pub fn get_playlists_in_folder(
        &self,
        folder_id: Option<&str>,
    ) -> Result<Vec<u64>, LibraryError> {
        if let Some(fid) = folder_id {
            let mut stmt = self.conn.prepare(
                "SELECT qobuz_playlist_id FROM playlist_settings WHERE folder_id = ?1 ORDER BY position ASC"
            ).map_err(|e| LibraryError::Database(format!("Failed to prepare statement: {}", e)))?;

            let ids = stmt
                .query_map(params![fid], |row| Ok(row.get::<_, i64>(0)? as u64))
                .map_err(|e| {
                    LibraryError::Database(format!("Failed to query playlists in folder: {}", e))
                })?;

            ids.collect::<Result<Vec<_>, _>>().map_err(|e| {
                LibraryError::Database(format!("Failed to collect playlist IDs: {}", e))
            })
        } else {
            let mut stmt = self.conn.prepare(
                "SELECT qobuz_playlist_id FROM playlist_settings WHERE folder_id IS NULL ORDER BY position ASC"
            ).map_err(|e| LibraryError::Database(format!("Failed to prepare statement: {}", e)))?;

            let ids = stmt
                .query_map([], |row| Ok(row.get::<_, i64>(0)? as u64))
                .map_err(|e| {
                    LibraryError::Database(format!("Failed to query playlists in folder: {}", e))
                })?;

            ids.collect::<Result<Vec<_>, _>>().map_err(|e| {
                LibraryError::Database(format!("Failed to collect playlist IDs: {}", e))
            })
        }
    }

    // === Playlist Local Tracks ===

    /// Add a local track to a playlist
    pub fn add_local_track_to_playlist(
        &self,
        qobuz_playlist_id: u64,
        local_track_id: i64,
        position: i32,
    ) -> Result<(), LibraryError> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);

        self.conn
            .execute(
                "INSERT OR REPLACE INTO playlist_local_tracks
                (qobuz_playlist_id, local_track_id, position, added_at)
             VALUES (?1, ?2, ?3, ?4)",
                params![qobuz_playlist_id as i64, local_track_id, position, now],
            )
            .map_err(|e| {
                LibraryError::Database(format!("Failed to add local track to playlist: {}", e))
            })?;

        Ok(())
    }

    /// Remove a local track from a playlist
    pub fn remove_local_track_from_playlist(
        &self,
        qobuz_playlist_id: u64,
        local_track_id: i64,
    ) -> Result<(), LibraryError> {
        self.conn
            .execute(
                "DELETE FROM playlist_local_tracks
             WHERE qobuz_playlist_id = ?1 AND local_track_id = ?2",
                params![qobuz_playlist_id as i64, local_track_id],
            )
            .map_err(|e| {
                LibraryError::Database(format!("Failed to remove local track from playlist: {}", e))
            })?;

        Ok(())
    }

    // === Playlist Plex Tracks ===

    /// Add a Plex track to a playlist, identified by its Plex rating key.
    /// The rating key is stored verbatim so the pairing survives Plex
    /// cache rebuilds.
    pub fn add_plex_track_to_playlist(
        &self,
        qobuz_playlist_id: u64,
        plex_rating_key: &str,
        position: i32,
    ) -> Result<(), LibraryError> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);

        self.conn
            .execute(
                "INSERT OR REPLACE INTO playlist_plex_tracks
                (qobuz_playlist_id, plex_rating_key, position, added_at)
             VALUES (?1, ?2, ?3, ?4)",
                params![qobuz_playlist_id as i64, plex_rating_key, position, now],
            )
            .map_err(|e| {
                LibraryError::Database(format!("Failed to add Plex track to playlist: {}", e))
            })?;

        Ok(())
    }

    /// Attach a Jellyfin/Subsonic track to a Qobuz playlist by its server
    /// item id — the Plex pattern, source-qualified. Re-adding MOVES the row
    /// to the new slot rather than duplicating it (INSERT OR REPLACE on the
    /// UNIQUE key, edge E4).
    pub fn add_remote_track_to_playlist(
        &self,
        qobuz_playlist_id: u64,
        source: &str,
        item_id: &str,
        position: i32,
    ) -> Result<(), LibraryError> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);

        self.conn
            .execute(
                "INSERT OR REPLACE INTO playlist_remote_tracks
                (qobuz_playlist_id, source, item_id, position, added_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
                params![qobuz_playlist_id as i64, source, item_id, position, now],
            )
            .map_err(|e| {
                LibraryError::Database(format!("Failed to add remote track to playlist: {}", e))
            })?;

        Ok(())
    }

    /// Remove a Jellyfin/Subsonic track from a playlist.
    pub fn remove_remote_track_from_playlist(
        &self,
        qobuz_playlist_id: u64,
        source: &str,
        item_id: &str,
    ) -> Result<(), LibraryError> {
        self.conn
            .execute(
                "DELETE FROM playlist_remote_tracks
                  WHERE qobuz_playlist_id = ?1 AND source = ?2 AND item_id = ?3",
                params![qobuz_playlist_id as i64, source, item_id],
            )
            .map_err(|e| {
                LibraryError::Database(format!(
                    "Failed to remove remote track from playlist: {}",
                    e
                ))
            })?;
        Ok(())
    }

    /// `(source, item_id, position)` of every remote sidecar row of one
    /// playlist, position ASC.
    pub fn get_playlist_remote_tracks_with_position(
        &self,
        qobuz_playlist_id: u64,
    ) -> Result<Vec<(String, String, i32)>, LibraryError> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT source, item_id, position FROM playlist_remote_tracks
                  WHERE qobuz_playlist_id = ?1
                  ORDER BY position ASC, added_at ASC, id ASC",
            )
            .map_err(|e| LibraryError::Database(format!("Failed to prepare query: {}", e)))?;
        let rows = stmt
            .query_map(params![qobuz_playlist_id as i64], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i32>(2)?,
                ))
            })
            .map_err(|e| LibraryError::Database(format!("Failed to query: {}", e)))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(
                row.map_err(|e| LibraryError::Database(format!("Failed to read row: {}", e)))?,
            );
        }
        Ok(out)
    }

    /// Number of remote sidecar rows of one playlist.
    pub fn get_playlist_remote_track_count(
        &self,
        qobuz_playlist_id: u64,
    ) -> Result<u32, LibraryError> {
        self.conn
            .query_row(
                "SELECT COUNT(*) FROM playlist_remote_tracks WHERE qobuz_playlist_id = ?1",
                params![qobuz_playlist_id as i64],
                |row| row.get::<_, u32>(0),
            )
            .map_err(|e| LibraryError::Database(format!("Failed to count remote tracks: {}", e)))
    }

    /// Remove a Plex track from a playlist.
    pub fn remove_plex_track_from_playlist(
        &self,
        qobuz_playlist_id: u64,
        plex_rating_key: &str,
    ) -> Result<(), LibraryError> {
        self.conn
            .execute(
                "DELETE FROM playlist_plex_tracks
             WHERE qobuz_playlist_id = ?1 AND plex_rating_key = ?2",
                params![qobuz_playlist_id as i64, plex_rating_key],
            )
            .map_err(|e| {
                LibraryError::Database(format!("Failed to remove Plex track from playlist: {}", e))
            })?;

        Ok(())
    }

    /// Get all Plex tracks in a playlist with their stored position.
    /// Returns (rating_key, position) pairs. The caller is responsible
    /// for hydrating metadata from the Plex cache.
    pub fn get_playlist_plex_tracks_with_position(
        &self,
        qobuz_playlist_id: u64,
    ) -> Result<Vec<(String, i32)>, LibraryError> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT plex_rating_key, position
                 FROM playlist_plex_tracks
                 WHERE qobuz_playlist_id = ?1
                 ORDER BY position ASC",
            )
            .map_err(|e| LibraryError::Database(format!("Failed to prepare query: {}", e)))?;

        let rows = stmt
            .query_map(params![qobuz_playlist_id as i64], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i32>(1)?))
            })
            .map_err(|e| {
                LibraryError::Database(format!("Failed to query playlist plex tracks: {}", e))
            })?;

        rows.collect::<Result<Vec<_>, _>>().map_err(|e| {
            LibraryError::Database(format!("Failed to collect playlist plex tracks: {}", e))
        })
    }

    /// Get count of Plex tracks in a playlist
    pub fn get_playlist_plex_track_count(
        &self,
        qobuz_playlist_id: u64,
    ) -> Result<u32, LibraryError> {
        let count: u32 = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM playlist_plex_tracks WHERE qobuz_playlist_id = ?1",
                params![qobuz_playlist_id as i64],
                |row| row.get(0),
            )
            .map_err(|e| {
                LibraryError::Database(format!("Failed to count playlist plex tracks: {}", e))
            })?;

        Ok(count)
    }

    /// Get all local tracks in a playlist
    pub fn get_playlist_local_tracks(
        &self,
        qobuz_playlist_id: u64,
    ) -> Result<Vec<LocalTrack>, LibraryError> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT t.id, t.file_path, t.title, t.artist, t.album, t.album_artist,
                    t.album_group_key, t.album_group_title, t.track_number, t.disc_number,
                    t.year, t.genre, t.genres_json, t.duration_secs, t.format, t.bit_depth, t.sample_rate,
                    t.channels, t.file_size_bytes, t.cue_file_path, t.cue_start_secs,
                    t.cue_end_secs, t.artwork_path, t.last_modified, t.indexed_at, t.source,
                    t.qobuz_track_id, t.is_network_mount, plt.position
             FROM playlist_local_tracks plt
             JOIN local_tracks t ON plt.local_track_id = t.id
             WHERE plt.qobuz_playlist_id = ?1
             ORDER BY plt.position ASC",
            )
            .map_err(|e| LibraryError::Database(format!("Failed to prepare statement: {}", e)))?;

        let tracks = stmt
            .query_map(params![qobuz_playlist_id as i64], |row| {
                Ok(LocalTrack {
                    id: row.get(0)?,
                    file_path: row.get(1)?,
                    title: row.get(2)?,
                    artist: row.get(3)?,
                    album: row.get(4)?,
                    album_artist: row.get(5)?,
                    album_group_key: row.get::<_, Option<String>>(6)?.unwrap_or_default(),
                    album_group_title: row.get::<_, Option<String>>(7)?.unwrap_or_default(),
                    track_number: row.get(8)?,
                    disc_number: row.get(9)?,
                    year: row.get(10)?,
                    genre: row.get(11)?,
                    genres: Self::genres_from_json(
                        row.get::<_, Option<String>>(12)?.as_deref(),
                        row.get::<_, Option<String>>(11)?.as_deref(),
                    ),
                    catalog_number: None,
                    duration_secs: row.get::<_, i64>(13)? as u64,
                    format: Self::parse_format(&row.get::<_, String>(14)?),
                    bit_depth: row.get(15)?,
                    sample_rate: row.get::<_, f64>(16)?,
                    channels: row.get(17)?,
                    file_size_bytes: row.get::<_, i64>(18)? as u64,
                    cue_file_path: row.get(19)?,
                    cue_start_secs: row.get(20)?,
                    cue_end_secs: row.get(21)?,
                    artwork_path: row.get(22)?,
                    collection_artwork_path: None,
                    last_modified: row.get(23)?,
                    indexed_at: row.get(24)?,
                    source: row.get(25)?,
                    qobuz_track_id: row.get(26)?,
                    isrc: None,
                    musicbrainz_recording_id: None,
                    musicbrainz_track_id: None,
                    musicbrainz_release_id: None,
                    musicbrainz_release_group_id: None,
                    musicbrainz_artist_id: None,
                    is_network_mount: row.get::<_, i64>(27)? != 0,
                })
            })
            .map_err(|e| {
                LibraryError::Database(format!("Failed to query playlist local tracks: {}", e))
            })?;

        tracks.collect::<Result<Vec<_>, _>>().map_err(|e| {
            LibraryError::Database(format!("Failed to collect playlist local tracks: {}", e))
        })
    }

    /// Get all local tracks in a playlist with their positions (for mixed ordering)
    pub fn get_playlist_local_tracks_with_position(
        &self,
        qobuz_playlist_id: u64,
    ) -> Result<Vec<crate::PlaylistLocalTrack>, LibraryError> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT t.id, t.file_path, t.title, t.artist, t.album, t.album_artist,
                    t.album_group_key, t.album_group_title, t.track_number, t.disc_number,
                    t.year, t.genre, t.genres_json, t.duration_secs, t.format, t.bit_depth, t.sample_rate,
                    t.channels, t.file_size_bytes, t.cue_file_path, t.cue_start_secs,
                    t.cue_end_secs, t.artwork_path, t.last_modified, t.indexed_at, t.source,
                    t.qobuz_track_id, t.is_network_mount, plt.position
             FROM playlist_local_tracks plt
             JOIN local_tracks t ON plt.local_track_id = t.id
             WHERE plt.qobuz_playlist_id = ?1
             ORDER BY plt.position ASC",
            )
            .map_err(|e| LibraryError::Database(format!("Failed to prepare statement: {}", e)))?;

        let tracks = stmt
            .query_map(params![qobuz_playlist_id as i64], |row| {
                Ok(crate::PlaylistLocalTrack {
                    track: LocalTrack {
                        id: row.get(0)?,
                        file_path: row.get(1)?,
                        title: row.get(2)?,
                        artist: row.get(3)?,
                        album: row.get(4)?,
                        album_artist: row.get(5)?,
                        album_group_key: row.get::<_, Option<String>>(6)?.unwrap_or_default(),
                        album_group_title: row.get::<_, Option<String>>(7)?.unwrap_or_default(),
                        track_number: row.get(8)?,
                        disc_number: row.get(9)?,
                        year: row.get(10)?,
                        genre: row.get(11)?,
                        genres: Self::genres_from_json(
                            row.get::<_, Option<String>>(12)?.as_deref(),
                            row.get::<_, Option<String>>(11)?.as_deref(),
                        ),
                        catalog_number: None,
                        duration_secs: row.get::<_, i64>(13)? as u64,
                        format: Self::parse_format(&row.get::<_, String>(14)?),
                        bit_depth: row.get(15)?,
                        sample_rate: row.get::<_, f64>(16)?,
                        channels: row.get(17)?,
                        file_size_bytes: row.get::<_, i64>(18)? as u64,
                        cue_file_path: row.get(19)?,
                        cue_start_secs: row.get(20)?,
                        cue_end_secs: row.get(21)?,
                        artwork_path: row.get(22)?,
                        collection_artwork_path: None,
                        last_modified: row.get(23)?,
                        indexed_at: row.get(24)?,
                        source: row.get(25)?,
                        qobuz_track_id: row.get(26)?,
                        isrc: None,
                        musicbrainz_recording_id: None,
                        musicbrainz_track_id: None,
                        musicbrainz_release_id: None,
                        musicbrainz_release_group_id: None,
                        musicbrainz_artist_id: None,
                        is_network_mount: row.get::<_, i64>(27)? != 0,
                    },
                    playlist_position: row.get(28)?,
                })
            })
            .map_err(|e| {
                LibraryError::Database(format!(
                    "Failed to query playlist local tracks with position: {}",
                    e
                ))
            })?;

        tracks.collect::<Result<Vec<_>, _>>().map_err(|e| {
            LibraryError::Database(format!(
                "Failed to collect playlist local tracks with position: {}",
                e
            ))
        })
    }

    /// Get count of local tracks in a playlist
    pub fn get_playlist_local_track_count(
        &self,
        qobuz_playlist_id: u64,
    ) -> Result<u32, LibraryError> {
        let count: u32 = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM playlist_local_tracks WHERE qobuz_playlist_id = ?1",
                params![qobuz_playlist_id as i64],
                |row| row.get(0),
            )
            .map_err(|e| {
                LibraryError::Database(format!("Failed to count playlist local tracks: {}", e))
            })?;

        Ok(count)
    }

    /// Get local track counts for all playlists.
    ///
    /// "Local" here is the user-facing sense — anything that isn't a Qobuz
    /// server track. That includes file-system local tracks (user / qobuz
    /// purchases / offline-cached downloads, all in local_tracks) plus
    /// Plex tracks (in a parallel playlist_plex_tracks table). The two
    /// sums are merged per playlist so the sidebar's hasLocalContent
    /// indicator picks up Plex content too.
    pub fn get_all_playlist_local_track_counts(
        &self,
    ) -> Result<std::collections::HashMap<u64, u32>, LibraryError> {
        let mut result: std::collections::HashMap<u64, u32> = std::collections::HashMap::new();

        let mut stmt = self
            .conn
            .prepare(
                "SELECT qobuz_playlist_id, COUNT(*) as count
             FROM playlist_local_tracks
             GROUP BY qobuz_playlist_id",
            )
            .map_err(|e| LibraryError::Database(format!("Failed to prepare query: {}", e)))?;

        let rows = stmt
            .query_map([], |row| {
                let playlist_id: i64 = row.get(0)?;
                let count: u32 = row.get(1)?;
                Ok((playlist_id as u64, count))
            })
            .map_err(|e| LibraryError::Database(format!("Failed to query: {}", e)))?;

        for row in rows {
            let (playlist_id, count) =
                row.map_err(|e| LibraryError::Database(format!("Failed to read row: {}", e)))?;
            result.insert(playlist_id, count);
        }

        let mut plex_stmt = self
            .conn
            .prepare(
                "SELECT qobuz_playlist_id, COUNT(*) as count
             FROM playlist_plex_tracks
             GROUP BY qobuz_playlist_id",
            )
            .map_err(|e| LibraryError::Database(format!("Failed to prepare query: {}", e)))?;

        let plex_rows = plex_stmt
            .query_map([], |row| {
                let playlist_id: i64 = row.get(0)?;
                let count: u32 = row.get(1)?;
                Ok((playlist_id as u64, count))
            })
            .map_err(|e| LibraryError::Database(format!("Failed to query: {}", e)))?;

        for row in plex_rows {
            let (playlist_id, count) =
                row.map_err(|e| LibraryError::Database(format!("Failed to read row: {}", e)))?;
            *result.entry(playlist_id).or_insert(0) += count;
        }

        let mut remote_stmt = self
            .conn
            .prepare(
                "SELECT qobuz_playlist_id, COUNT(*) as count
             FROM playlist_remote_tracks
             GROUP BY qobuz_playlist_id",
            )
            .map_err(|e| LibraryError::Database(format!("Failed to prepare query: {}", e)))?;

        let remote_rows = remote_stmt
            .query_map([], |row| {
                let playlist_id: i64 = row.get(0)?;
                let count: u32 = row.get(1)?;
                Ok((playlist_id as u64, count))
            })
            .map_err(|e| LibraryError::Database(format!("Failed to query: {}", e)))?;

        for row in remote_rows {
            let (playlist_id, count) =
                row.map_err(|e| LibraryError::Database(format!("Failed to read row: {}", e)))?;
            *result.entry(playlist_id).or_insert(0) += count;
        }

        Ok(result)
    }

    /// Update position of a local track in a playlist
    pub fn update_local_track_position(
        &self,
        qobuz_playlist_id: u64,
        local_track_id: i64,
        new_position: i32,
    ) -> Result<(), LibraryError> {
        self.conn
            .execute(
                "UPDATE playlist_local_tracks SET position = ?1
             WHERE qobuz_playlist_id = ?2 AND local_track_id = ?3",
                params![new_position, qobuz_playlist_id as i64, local_track_id],
            )
            .map_err(|e| {
                LibraryError::Database(format!("Failed to update local track position: {}", e))
            })?;

        Ok(())
    }

    /// Clear all local tracks from a playlist
    pub fn clear_playlist_local_tracks(&self, qobuz_playlist_id: u64) -> Result<(), LibraryError> {
        self.conn
            .execute(
                "DELETE FROM playlist_local_tracks WHERE qobuz_playlist_id = ?1",
                params![qobuz_playlist_id as i64],
            )
            .map_err(|e| {
                LibraryError::Database(format!("Failed to clear playlist local tracks: {}", e))
            })?;

        Ok(())
    }

    // === Sidecar position lifecycle (mixed "carrete" playlists) ===

    /// Next append position for a local/Plex sidecar add to a Qobuz playlist.
    ///
    /// Tauri's convention is `qobuz_count + sidecar_count` — append after the
    /// whole merged list — but that formula re-issues positions after a
    /// removal (stored positions keep their gaps while counts shrink; Tauri
    /// bug T3), which collides in the absolute-slot interleave and silently
    /// loses rows. The fix-forward rule is the MAX of both worlds:
    ///
    /// `max(qobuz_count + local_count + plex_count, MAX(position) + 1)`
    ///
    /// computed across BOTH sidecar tables, so an add always lands after the
    /// merged end AND past every stored position. Batch adds take this once
    /// and assign `next + i` per row.
    pub fn next_playlist_sidecar_position(
        &self,
        qobuz_playlist_id: u64,
        qobuz_track_count: u32,
    ) -> Result<i32, LibraryError> {
        let local_count = self.get_playlist_local_track_count(qobuz_playlist_id)?;
        let plex_count = self.get_playlist_plex_track_count(qobuz_playlist_id)?;
        let remote_count = self.get_playlist_remote_track_count(qobuz_playlist_id)?;
        let max_pos: Option<i32> = self
            .conn
            .query_row(
                "SELECT MAX(p) FROM (
                    SELECT MAX(position) AS p FROM playlist_local_tracks
                     WHERE qobuz_playlist_id = ?1
                    UNION ALL
                    SELECT MAX(position) AS p FROM playlist_plex_tracks
                     WHERE qobuz_playlist_id = ?1
                    UNION ALL
                    SELECT MAX(position) AS p FROM playlist_remote_tracks
                     WHERE qobuz_playlist_id = ?1
                )",
                params![qobuz_playlist_id as i64],
                |row| row.get(0),
            )
            .map_err(|e| {
                LibraryError::Database(format!("Failed to read max sidecar position: {}", e))
            })?;
        let count_based = (qobuz_track_count + local_count + plex_count + remote_count) as i32;
        Ok(count_based.max(max_pos.map(|p| p + 1).unwrap_or(0)))
    }

    /// One-shot healing for sidecar position collisions (mixed playlists).
    ///
    /// Positions are absolute slots in the merged interleave and have no
    /// UNIQUE constraint; the legacy Slint picker/drag wrote them 0-based per
    /// batch, and Tauri's create-and-add writes local AND plex rows 0-based
    /// in parallel — both produce duplicate positions, which a Map-based
    /// merge collapses (silent row loss, edges E1/E2). This walks both
    /// tables in stable order (local table first, then plex; within a table
    /// position ASC, added_at ASC, rowid ASC — the first claimant of a
    /// contested slot keeps it, matching the merge's local-first emit) and
    /// renumbers every LATER claimant into the append region:
    /// `max(qobuz_track_count + sidecar_count, MAX(position) + 1)` onward.
    ///
    /// Non-colliding rows are NEVER touched — drift is normal (edge E7);
    /// this is collision repair, not renormalization. Returns one
    /// "kind ref: old -> new" description per moved row for the caller to
    /// log; empty = nothing healed. Idempotent.
    pub fn heal_playlist_sidecar_positions(
        &self,
        qobuz_playlist_id: u64,
        qobuz_track_count: u32,
    ) -> Result<Vec<String>, LibraryError> {
        // (kind, rowid, ref-description, position) in stable claim order.
        let mut rows: Vec<(&'static str, i64, String, i32)> = Vec::new();
        {
            let mut stmt = self
                .conn
                .prepare(
                    "SELECT id, local_track_id, position FROM playlist_local_tracks
                     WHERE qobuz_playlist_id = ?1
                     ORDER BY position ASC, added_at ASC, id ASC",
                )
                .map_err(|e| {
                    LibraryError::Database(format!("Failed to prepare heal query: {}", e))
                })?;
            let mapped = stmt
                .query_map(params![qobuz_playlist_id as i64], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i32>(2)?,
                    ))
                })
                .map_err(|e| LibraryError::Database(format!("Failed to query heal rows: {}", e)))?;
            for r in mapped {
                let (rowid, track, pos) = r.map_err(|e| {
                    LibraryError::Database(format!("Failed to read heal row: {}", e))
                })?;
                rows.push(("local", rowid, track.to_string(), pos));
            }
        }
        {
            let mut stmt = self
                .conn
                .prepare(
                    "SELECT id, plex_rating_key, position FROM playlist_plex_tracks
                     WHERE qobuz_playlist_id = ?1
                     ORDER BY position ASC, added_at ASC, id ASC",
                )
                .map_err(|e| {
                    LibraryError::Database(format!("Failed to prepare heal query: {}", e))
                })?;
            let mapped = stmt
                .query_map(params![qobuz_playlist_id as i64], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i32>(2)?,
                    ))
                })
                .map_err(|e| LibraryError::Database(format!("Failed to query heal rows: {}", e)))?;
            for r in mapped {
                let (rowid, key, pos) = r.map_err(|e| {
                    LibraryError::Database(format!("Failed to read heal row: {}", e))
                })?;
                rows.push(("plex", rowid, key, pos));
            }
        }
        {
            let mut stmt = self
                .conn
                .prepare(
                    "SELECT id, source || ':' || item_id, position FROM playlist_remote_tracks
                     WHERE qobuz_playlist_id = ?1
                     ORDER BY position ASC, added_at ASC, id ASC",
                )
                .map_err(|e| {
                    LibraryError::Database(format!("Failed to prepare heal query: {}", e))
                })?;
            let mapped = stmt
                .query_map(params![qobuz_playlist_id as i64], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i32>(2)?,
                    ))
                })
                .map_err(|e| LibraryError::Database(format!("Failed to query heal rows: {}", e)))?;
            for r in mapped {
                let (rowid, key, pos) = r.map_err(|e| {
                    LibraryError::Database(format!("Failed to read heal row: {}", e))
                })?;
                rows.push(("remote", rowid, key, pos));
            }
        }
        if rows.is_empty() {
            return Ok(Vec::new());
        }
        let max_pos = rows.iter().map(|r| r.3).max().unwrap_or(-1);
        let sidecar_total = rows.len();
        let mut seen = std::collections::HashSet::new();
        let mut moves: Vec<(&'static str, i64, String, i32)> = Vec::new();
        for row in rows {
            if !seen.insert(row.3) {
                moves.push(row);
            }
        }
        if moves.is_empty() {
            return Ok(Vec::new());
        }
        let mut next = ((qobuz_track_count as i32) + sidecar_total as i32).max(max_pos + 1);
        let mut healed = Vec::with_capacity(moves.len());
        for (kind, rowid, reference, old) in moves {
            let sql = match kind {
                "local" => "UPDATE playlist_local_tracks SET position = ?1 WHERE id = ?2",
                "plex" => "UPDATE playlist_plex_tracks SET position = ?1 WHERE id = ?2",
                _ => "UPDATE playlist_remote_tracks SET position = ?1 WHERE id = ?2",
            };
            self.conn.execute(sql, params![next, rowid]).map_err(|e| {
                LibraryError::Database(format!("Failed to heal sidecar position: {}", e))
            })?;
            healed.push(format!("{kind} {reference}: {old} -> {next}"));
            next += 1;
        }
        Ok(healed)
    }

    // === Playlist Custom Track Order ===

    /// Get custom track order for a playlist
    /// Returns Vec of (track_id, is_local, custom_position)
    pub fn get_playlist_custom_order(
        &self,
        qobuz_playlist_id: u64,
    ) -> Result<Vec<(i64, bool, i32)>, LibraryError> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT track_id, is_local, custom_position
             FROM playlist_track_custom_order
             WHERE qobuz_playlist_id = ?1
             ORDER BY custom_position ASC",
            )
            .map_err(|e| {
                LibraryError::Database(format!("Failed to prepare custom order query: {}", e))
            })?;

        let rows = stmt
            .query_map(params![qobuz_playlist_id as i64], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i32>(1)? != 0,
                    row.get::<_, i32>(2)?,
                ))
            })
            .map_err(|e| LibraryError::Database(format!("Failed to query custom order: {}", e)))?;

        let mut result = Vec::new();
        for row in rows {
            result.push(row.map_err(|e| {
                LibraryError::Database(format!("Failed to read custom order row: {}", e))
            })?);
        }
        Ok(result)
    }

    /// Initialize custom order for a playlist from a list of track IDs
    /// This sets up the initial order based on the current track arrangement
    pub fn init_playlist_custom_order(
        &self,
        qobuz_playlist_id: u64,
        track_ids: &[(i64, bool)], // (track_id, is_local)
    ) -> Result<(), LibraryError> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        // Clear existing custom order
        self.conn
            .execute(
                "DELETE FROM playlist_track_custom_order WHERE qobuz_playlist_id = ?1",
                params![qobuz_playlist_id as i64],
            )
            .map_err(|e| {
                LibraryError::Database(format!("Failed to clear existing custom order: {}", e))
            })?;

        // Insert new order
        let mut stmt = self
            .conn
            .prepare(
                "INSERT INTO playlist_track_custom_order
             (qobuz_playlist_id, track_id, is_local, custom_position, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )
            .map_err(|e| {
                LibraryError::Database(format!("Failed to prepare custom order insert: {}", e))
            })?;

        for (position, (track_id, is_local)) in track_ids.iter().enumerate() {
            stmt.execute(params![
                qobuz_playlist_id as i64,
                *track_id,
                *is_local as i32,
                position as i32,
                now,
                now,
            ])
            .map_err(|e| LibraryError::Database(format!("Failed to insert custom order: {}", e)))?;
        }

        Ok(())
    }

    /// Set entire custom order for a playlist (batch update)
    pub fn set_playlist_custom_order(
        &self,
        qobuz_playlist_id: u64,
        orders: &[(i64, bool, i32)], // (track_id, is_local, position)
    ) -> Result<(), LibraryError> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        // Clear existing custom order
        self.conn
            .execute(
                "DELETE FROM playlist_track_custom_order WHERE qobuz_playlist_id = ?1",
                params![qobuz_playlist_id as i64],
            )
            .map_err(|e| {
                LibraryError::Database(format!("Failed to clear existing custom order: {}", e))
            })?;

        // Insert new order
        let mut stmt = self
            .conn
            .prepare(
                "INSERT INTO playlist_track_custom_order
             (qobuz_playlist_id, track_id, is_local, custom_position, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )
            .map_err(|e| {
                LibraryError::Database(format!("Failed to prepare custom order insert: {}", e))
            })?;

        for (track_id, is_local, position) in orders {
            stmt.execute(params![
                qobuz_playlist_id as i64,
                *track_id,
                *is_local as i32,
                *position,
                now,
                now,
            ])
            .map_err(|e| LibraryError::Database(format!("Failed to insert custom order: {}", e)))?;
        }

        Ok(())
    }

    /// Move a single track to a new position (reorders other tracks accordingly)
    pub fn move_playlist_track(
        &self,
        qobuz_playlist_id: u64,
        track_id: i64,
        is_local: bool,
        new_position: i32,
    ) -> Result<(), LibraryError> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        // Get current position of the track
        let current_position: Option<i32> = self
            .conn
            .query_row(
                "SELECT custom_position FROM playlist_track_custom_order
             WHERE qobuz_playlist_id = ?1 AND track_id = ?2 AND is_local = ?3",
                params![qobuz_playlist_id as i64, track_id, is_local as i32],
                |row| row.get(0),
            )
            .ok();

        let current_position = match current_position {
            Some(pos) => pos,
            None => {
                // Track not in custom order yet, just insert it
                self.conn.execute(
                    "INSERT INTO playlist_track_custom_order
                     (qobuz_playlist_id, track_id, is_local, custom_position, created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![qobuz_playlist_id as i64, track_id, is_local as i32, new_position, now, now],
                ).map_err(|e| LibraryError::Database(format!("Failed to insert track position: {}", e)))?;
                return Ok(());
            }
        };

        if current_position == new_position {
            return Ok(());
        }

        // Shift other tracks to make room
        if new_position < current_position {
            // Moving up: shift tracks between new_position and current_position down
            self.conn
                .execute(
                    "UPDATE playlist_track_custom_order
                 SET custom_position = custom_position + 1, updated_at = ?4
                 WHERE qobuz_playlist_id = ?1
                   AND custom_position >= ?2
                   AND custom_position < ?3",
                    params![
                        qobuz_playlist_id as i64,
                        new_position,
                        current_position,
                        now
                    ],
                )
                .map_err(|e| LibraryError::Database(format!("Failed to shift tracks: {}", e)))?;
        } else {
            // Moving down: shift tracks between current_position and new_position up
            self.conn
                .execute(
                    "UPDATE playlist_track_custom_order
                 SET custom_position = custom_position - 1, updated_at = ?4
                 WHERE qobuz_playlist_id = ?1
                   AND custom_position > ?2
                   AND custom_position <= ?3",
                    params![
                        qobuz_playlist_id as i64,
                        current_position,
                        new_position,
                        now
                    ],
                )
                .map_err(|e| LibraryError::Database(format!("Failed to shift tracks: {}", e)))?;
        }

        // Update the track's position
        self.conn
            .execute(
                "UPDATE playlist_track_custom_order
             SET custom_position = ?3, updated_at = ?5
             WHERE qobuz_playlist_id = ?1 AND track_id = ?2 AND is_local = ?4",
                params![
                    qobuz_playlist_id as i64,
                    track_id,
                    new_position,
                    is_local as i32,
                    now
                ],
            )
            .map_err(|e| {
                LibraryError::Database(format!("Failed to update track position: {}", e))
            })?;

        Ok(())
    }

    /// Check if a playlist has custom order defined
    pub fn has_playlist_custom_order(&self, qobuz_playlist_id: u64) -> Result<bool, LibraryError> {
        let count: i32 = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM playlist_track_custom_order WHERE qobuz_playlist_id = ?1",
                params![qobuz_playlist_id as i64],
                |row| row.get(0),
            )
            .map_err(|e| LibraryError::Database(format!("Failed to check custom order: {}", e)))?;

        Ok(count > 0)
    }

    /// Clear custom order for a playlist
    pub fn clear_playlist_custom_order(&self, qobuz_playlist_id: u64) -> Result<(), LibraryError> {
        self.conn
            .execute(
                "DELETE FROM playlist_track_custom_order WHERE qobuz_playlist_id = ?1",
                params![qobuz_playlist_id as i64],
            )
            .map_err(|e| LibraryError::Database(format!("Failed to clear custom order: {}", e)))?;

        Ok(())
    }

    // === Album Settings ===

    /// Get album settings
    pub fn get_album_settings(
        &self,
        album_group_key: &str,
    ) -> Result<Option<crate::AlbumSettings>, LibraryError> {
        let result = self
            .conn
            .query_row(
                "SELECT album_group_key, hidden, created_at, updated_at
             FROM album_settings WHERE album_group_key = ?1",
                params![album_group_key],
                |row| {
                    Ok(crate::AlbumSettings {
                        album_group_key: row.get(0)?,
                        hidden: row.get::<_, i32>(1)? != 0,
                        created_at: row.get(2)?,
                        updated_at: row.get(3)?,
                    })
                },
            )
            .optional()
            .map_err(|e| LibraryError::Database(format!("Failed to get album settings: {}", e)))?;

        Ok(result)
    }

    /// Set album hidden status
    pub fn set_album_hidden(
        &self,
        album_group_key: &str,
        hidden: bool,
    ) -> Result<(), LibraryError> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);

        self.conn
            .execute(
                "INSERT INTO album_settings (album_group_key, hidden, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(album_group_key) DO UPDATE SET
                hidden = excluded.hidden,
                updated_at = excluded.updated_at",
                params![album_group_key, hidden as i32, now, now],
            )
            .map_err(|e| LibraryError::Database(format!("Failed to set album hidden: {}", e)))?;

        Ok(())
    }

    /// Get all hidden albums
    pub fn get_hidden_albums(&self) -> Result<Vec<String>, LibraryError> {
        let mut stmt = self
            .conn
            .prepare("SELECT album_group_key FROM album_settings WHERE hidden = 1")
            .map_err(|e| LibraryError::Database(e.to_string()))?;

        let rows = stmt
            .query_map([], |row| row.get(0))
            .map_err(|e| LibraryError::Database(e.to_string()))?;

        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| LibraryError::Database(e.to_string()))
    }

    // === Qobuz Downloads Integration ===

    /// All offline-copy rows: `source = 'qobuz_download'` with a real Qobuz
    /// id — the same set the Local Library "Offline" source filter shows.
    /// Read-only; used by the offline favorites rail (B9) to find favorite
    /// tracks that are playable without Qobuz.
    pub fn get_qobuz_download_tracks(&self) -> Result<Vec<LocalTrack>, LibraryError> {
        let sql = format!(
            "SELECT {} FROM local_tracks \
             WHERE source = 'qobuz_download' AND qobuz_track_id IS NOT NULL \
             ORDER BY artist COLLATE NOCASE, album COLLATE NOCASE, track_number",
            Self::TRACK_COLUMNS
        );
        let mut stmt = self
            .conn
            .prepare(&sql)
            .map_err(|e| LibraryError::Database(e.to_string()))?;

        let rows = stmt
            .query_map([], |row| Self::row_to_track(row))
            .map_err(|e| LibraryError::Database(e.to_string()))?;

        let mut tracks = Vec::new();
        for track in rows {
            tracks.push(track.map_err(|e| LibraryError::Database(e.to_string()))?);
        }
        Ok(tracks)
    }

    /// Check if a track exists by Qobuz track ID
    pub fn track_exists_by_qobuz_id(&self, qobuz_track_id: u64) -> Result<bool, LibraryError> {
        let count: i64 = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM local_tracks WHERE qobuz_track_id = ?1",
                params![qobuz_track_id as i64],
                |row| row.get(0),
            )
            .map_err(|e| LibraryError::Database(e.to_string()))?;
        Ok(count > 0)
    }

    /// Repair a track by file_path - restores both qobuz_track_id and source
    /// This handles tracks that were damaged by scanner's INSERT OR REPLACE
    /// Returns true if the track was found and updated
    /// Note: Database source field remains 'qobuz_download' for compatibility
    pub fn repair_qobuz_cached_track_by_path(
        &self,
        qobuz_track_id: u64,
        file_path: &str,
    ) -> Result<bool, LibraryError> {
        let updated = self
            .conn
            .execute(
                "UPDATE local_tracks
             SET source = 'qobuz_download', qobuz_track_id = ?1
             WHERE file_path = ?2 AND (source IS NULL OR source != 'qobuz_download')",
                params![qobuz_track_id as i64, file_path],
            )
            .map_err(|e| {
                LibraryError::Database(format!("Failed to repair cached track by path: {}", e))
            })?;
        Ok(updated > 0)
    }

    /// Check if a track exists by file path (for repair matching)
    pub fn track_exists_by_path(&self, file_path: &str) -> Result<bool, LibraryError> {
        let count: i64 = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM local_tracks WHERE file_path = ?1",
                params![file_path],
                |row| row.get(0),
            )
            .map_err(|e| LibraryError::Database(e.to_string()))?;
        Ok(count > 0)
    }

    /// Insert a Qobuz cached track into the library
    /// Note: Database source field remains 'qobuz_download' for compatibility
    pub fn insert_qobuz_cached_track_direct(
        &self,
        track_id: u64,
        title: &str,
        artist: &str,
        album: Option<&str>,
        duration_secs: u64,
        file_path: &str,
        bit_depth: Option<u32>,
        sample_rate: Option<f64>,
        track_number: Option<u32>,
        disc_number: Option<u32>,
    ) -> Result<(), LibraryError> {
        use std::time::SystemTime;

        // Get file size if file exists
        let file_size_bytes = std::fs::metadata(file_path)
            .map(|m| m.len() as i64)
            .unwrap_or(0);

        // Get current timestamp
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        self.conn.execute(
            r#"
            INSERT INTO local_tracks (
                file_path, title, artist, album, album_artist,
                track_number, disc_number, year, duration_secs,
                format, bit_depth, sample_rate, channels,
                file_size_bytes, last_modified, indexed_at,
                source, qobuz_track_id
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, 'qobuz_download', ?17)
            "#,
            params![
                file_path,
                title,
                artist,
                album.unwrap_or("Unknown Album"),
                artist, // Use artist as album_artist for proper grouping
                track_number.map(|v| v as i64),
                disc_number.map(|v| v as i64),
                None::<u32>, // year
                duration_secs as i64,
                "flac", // Default format for downloads
                bit_depth.map(|v| v as i64),
                sample_rate.unwrap_or(44100.0),
                2, // Assume stereo
                file_size_bytes,
                now,
                now,
                track_id as i64,
            ],
        )
        .map_err(|e| LibraryError::Database(format!("Failed to insert Qobuz cached track: {}", e)))?;
        Ok(())
    }

    /// Insert a Qobuz cached track with full metadata and album grouping
    /// Note: Database source field remains 'qobuz_download' for compatibility
    pub fn insert_qobuz_cached_track_with_grouping(
        &self,
        track_id: u64,
        title: &str,
        artist: &str,
        album: Option<&str>,
        album_artist: Option<&str>,
        track_number: Option<u32>,
        disc_number: Option<u32>,
        year: Option<u32>,
        duration_secs: u64,
        file_path: &str,
        album_group_key: &str,
        album_group_title: &str,
        bit_depth: Option<u32>,
        sample_rate: Option<f64>,
        artwork_path: Option<&str>,
    ) -> Result<(), LibraryError> {
        use std::time::SystemTime;

        // First, remove any existing entry for this qobuz_track_id to prevent duplicates
        let _ = self.remove_qobuz_cached_track(track_id);

        // Get file size if file exists
        let file_size_bytes = std::fs::metadata(file_path)
            .map(|m| m.len() as i64)
            .unwrap_or(0);

        // Get current timestamp
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        self.conn.execute(
            r#"
            INSERT INTO local_tracks (
                file_path, title, artist, album, album_artist,
                track_number, disc_number, year, duration_secs,
                format, bit_depth, sample_rate, channels,
                file_size_bytes, last_modified, indexed_at,
                album_group_key, album_group_title,
                artwork_path,
                source, qobuz_track_id
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, 'qobuz_download', ?20)
            "#,
            params![
                file_path,
                title,
                artist,
                album.unwrap_or("Unknown Album"),
                album_artist.unwrap_or(artist),
                track_number.map(|v| v as i64),
                disc_number.map(|v| v as i64),
                year.map(|v| v as i64),
                duration_secs as i64,
                "flac",
                bit_depth.map(|v| v as i64),
                sample_rate.unwrap_or(44100.0),
                2, // Assume stereo
                file_size_bytes,
                now,
                now,
                album_group_key,
                album_group_title,
                artwork_path,
                track_id as i64,
            ],
        )
        .map_err(|e| LibraryError::Database(format!("Failed to insert Qobuz cached track: {}", e)))?;
        Ok(())
    }

    /// Remove a Qobuz cached track from the library by track_id
    /// Note: Database source field remains 'qobuz_download' for compatibility
    pub fn remove_qobuz_cached_track(&self, qobuz_track_id: u64) -> Result<(), LibraryError> {
        self.conn
            .execute(
                "DELETE FROM local_tracks WHERE qobuz_track_id = ?1 AND source = 'qobuz_download'",
                params![qobuz_track_id as i64],
            )
            .map_err(|e| {
                LibraryError::Database(format!("Failed to remove Qobuz cached track: {}", e))
            })?;
        Ok(())
    }

    /// Remove all Qobuz cached tracks from the library
    /// Note: Database source field remains 'qobuz_download' for compatibility
    pub fn remove_all_qobuz_cached_tracks(&self) -> Result<usize, LibraryError> {
        let count = self
            .conn
            .execute(
                "DELETE FROM local_tracks WHERE source = 'qobuz_download'",
                [],
            )
            .map_err(|e| {
                LibraryError::Database(format!("Failed to remove all Qobuz cached tracks: {}", e))
            })?;
        Ok(count)
    }

    // === Artist Images Management ===

    /// Get cached artist image
    pub fn get_artist_image(
        &self,
        artist_name: &str,
    ) -> Result<Option<crate::ArtistImageInfo>, LibraryError> {
        let result = self.conn.query_row(
            "SELECT artist_name, image_url, source, custom_image_path, canonical_name FROM artist_images WHERE artist_name = ?1",
            params![artist_name],
            |row| {
                Ok(crate::ArtistImageInfo {
                    artist_name: row.get(0)?,
                    image_url: row.get(1)?,
                    source: row.get(2)?,
                    custom_image_path: row.get(3)?,
                    canonical_name: row.get(4)?,
                })
            }
        ).optional()
        .map_err(|e| LibraryError::Database(format!("Failed to get artist image: {}", e)))?;
        Ok(result)
    }

    /// Whether an artist-image lookup (positive or negative) was completed
    /// within `max_age_secs`.
    ///
    /// Negative rows deliberately keep `image_url` NULL.  Remembering those
    /// misses is what prevents every visit to a long Artists rail from
    /// repeating the same Qobuz/Last.fm/Discogs requests.  Custom artwork is
    /// a positive answer regardless of the remote lookup timestamp.
    pub fn artist_image_resolution_is_fresh(
        &self,
        artist_name: &str,
        max_age_secs: i64,
    ) -> Result<bool, LibraryError> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        let cutoff = now.saturating_sub(max_age_secs.max(0));
        self.conn
            .query_row(
                "SELECT EXISTS(
                     SELECT 1 FROM artist_images
                      WHERE artist_name = ?1
                        AND (custom_image_path IS NOT NULL OR fetched_at >= ?2)
                 )",
                params![artist_name, cutoff],
                |row| row.get::<_, i64>(0),
            )
            .map(|value| value != 0)
            .map_err(|e| {
                LibraryError::Database(format!("Failed to query artist image freshness: {e}"))
            })
    }

    /// Get all custom artist images (for bulk lookup)
    pub fn get_all_custom_artist_images(
        &self,
    ) -> Result<std::collections::HashMap<String, String>, LibraryError> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT artist_name, custom_image_path FROM artist_images WHERE custom_image_path IS NOT NULL",
            )
            .map_err(|e| LibraryError::Database(format!("Failed to prepare query: {}", e)))?;

        let rows = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|e| {
                LibraryError::Database(format!("Failed to query custom artist images: {}", e))
            })?;

        let mut map = std::collections::HashMap::new();
        for row in rows {
            if let Ok((artist_name, custom_image_path)) = row {
                map.insert(artist_name, custom_image_path);
            }
        }
        Ok(map)
    }

    /// Bulk-load every cached artist image (custom path preferred, else the
    /// fetched Qobuz URL) keyed by artist_name. Lets a UI seed the rail with
    /// previously-fetched portraits on revisit without re-hitting Qobuz.
    /// (The Tauri batch command `library_get_artist_images` was never
    /// registered; this is the corrected one-pass reader.)
    pub fn get_all_artist_image_urls(
        &self,
    ) -> Result<std::collections::HashMap<String, String>, LibraryError> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT artist_name, custom_image_path, image_url FROM artist_images \
                 WHERE custom_image_path IS NOT NULL OR image_url IS NOT NULL",
            )
            .map_err(|e| LibraryError::Database(format!("Failed to prepare query: {}", e)))?;

        let rows = stmt
            .query_map([], |row| {
                let custom: Option<String> = row.get(1)?;
                let url: Option<String> = row.get(2)?;
                Ok((row.get::<_, String>(0)?, custom.or(url)))
            })
            .map_err(|e| LibraryError::Database(format!("Failed to query artist images: {}", e)))?;

        let mut map = std::collections::HashMap::new();
        for row in rows.flatten() {
            let (name, maybe_path) = row;
            if let Some(path) = maybe_path {
                map.insert(name, path);
            }
        }
        Ok(map)
    }

    /// Get all canonical artist names mapping (for bulk lookup)
    pub fn get_all_canonical_names(
        &self,
    ) -> Result<std::collections::HashMap<String, String>, LibraryError> {
        let mut stmt = self.conn.prepare(
            "SELECT artist_name, canonical_name FROM artist_images WHERE canonical_name IS NOT NULL"
        ).map_err(|e| LibraryError::Database(format!("Failed to prepare query: {}", e)))?;

        let rows = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|e| {
                LibraryError::Database(format!("Failed to query canonical names: {}", e))
            })?;

        let mut map = std::collections::HashMap::new();
        for row in rows {
            if let Ok((artist_name, canonical_name)) = row {
                map.insert(artist_name, canonical_name);
            }
        }
        Ok(map)
    }

    /// Cache artist image with optional canonical name
    pub fn cache_artist_image(
        &self,
        artist_name: &str,
        image_url: Option<&str>,
        source: &str,
        custom_image_path: Option<&str>,
    ) -> Result<(), LibraryError> {
        self.cache_artist_image_with_canonical(
            artist_name,
            image_url,
            source,
            custom_image_path,
            None,
        )
    }

    /// Cache artist image with canonical name from Qobuz/Discogs
    pub fn cache_artist_image_with_canonical(
        &self,
        artist_name: &str,
        image_url: Option<&str>,
        source: &str,
        custom_image_path: Option<&str>,
        canonical_name: Option<&str>,
    ) -> Result<(), LibraryError> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        self.conn.execute(
            "INSERT INTO artist_images
             (artist_name, image_url, source, custom_image_path, canonical_name, fetched_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(artist_name) DO UPDATE SET
                 image_url = excluded.image_url,
                 source = excluded.source,
                 custom_image_path = COALESCE(excluded.custom_image_path,
                                               artist_images.custom_image_path),
                 canonical_name = COALESCE(excluded.canonical_name,
                                           artist_images.canonical_name),
                 fetched_at = excluded.fetched_at,
                 updated_at = excluded.updated_at",
            params![artist_name, image_url, source, custom_image_path, canonical_name, now, now],
        )
        .map_err(|e| LibraryError::Database(format!("Failed to cache artist image: {}", e)))?;
        Ok(())
    }

    // === Custom Album Covers ===

    /// Set a custom album cover
    pub fn set_custom_album_cover(
        &self,
        album_id: &str,
        custom_image_path: &str,
    ) -> Result<(), LibraryError> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        self.conn
            .execute(
                "INSERT OR REPLACE INTO custom_album_covers (album_id, custom_image_path, created_at)
                 VALUES (?1, ?2, ?3)",
                params![album_id, custom_image_path, now],
            )
            .map_err(|e| {
                LibraryError::Database(format!("Failed to set custom album cover: {}", e))
            })?;
        Ok(())
    }

    /// Get custom album cover path for a single album
    pub fn get_custom_album_cover(&self, album_id: &str) -> Result<Option<String>, LibraryError> {
        let mut stmt = self
            .conn
            .prepare("SELECT custom_image_path FROM custom_album_covers WHERE album_id = ?1")
            .map_err(|e| LibraryError::Database(format!("Failed to prepare query: {}", e)))?;

        let result = stmt
            .query_row(params![album_id], |row| row.get::<_, String>(0))
            .optional()
            .map_err(|e| {
                LibraryError::Database(format!("Failed to query custom album cover: {}", e))
            })?;

        Ok(result)
    }

    /// Remove a custom album cover
    pub fn remove_custom_album_cover(&self, album_id: &str) -> Result<(), LibraryError> {
        self.conn
            .execute(
                "DELETE FROM custom_album_covers WHERE album_id = ?1",
                params![album_id],
            )
            .map_err(|e| {
                LibraryError::Database(format!("Failed to remove custom album cover: {}", e))
            })?;
        Ok(())
    }

    /// Get all custom album covers (album_id -> file_path)
    pub fn get_all_custom_album_covers(
        &self,
    ) -> Result<std::collections::HashMap<String, String>, LibraryError> {
        let mut stmt = self
            .conn
            .prepare("SELECT album_id, custom_image_path FROM custom_album_covers")
            .map_err(|e| LibraryError::Database(format!("Failed to prepare query: {}", e)))?;

        let rows = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|e| {
                LibraryError::Database(format!("Failed to query custom album covers: {}", e))
            })?;

        let mut map = std::collections::HashMap::new();
        for row in rows {
            if let Ok((album_id, path)) = row {
                map.insert(album_id, path);
            }
        }
        Ok(map)
    }

    // === Offline Mode: Local Content Detection ===

    /// Check if a track exists locally by Qobuz track ID
    pub fn has_local_track_by_qobuz_id(&self, qobuz_track_id: u64) -> Result<bool, LibraryError> {
        let count: i64 = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM local_tracks WHERE qobuz_track_id = ?1",
                params![qobuz_track_id as i64],
                |row| row.get(0),
            )
            .map_err(|e| LibraryError::Database(e.to_string()))?;
        Ok(count > 0)
    }

    /// Check if a track exists locally by title, artist, and album (fuzzy match)
    pub fn has_local_track_by_metadata(
        &self,
        title: &str,
        artist: &str,
        album: &str,
    ) -> Result<bool, LibraryError> {
        // Normalize strings for comparison
        let title_lower = title.to_lowercase();
        let artist_lower = artist.to_lowercase();
        let album_lower = album.to_lowercase();

        let count: i64 = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM local_tracks
                 WHERE LOWER(title) = ?1 AND LOWER(artist) = ?2 AND LOWER(album) = ?3",
                params![title_lower, artist_lower, album_lower],
                |row| row.get(0),
            )
            .map_err(|e| LibraryError::Database(e.to_string()))?;
        Ok(count > 0)
    }

    /// Get local track ID by Qobuz track ID (for downloaded tracks)
    pub fn get_local_track_id_by_qobuz_id(
        &self,
        qobuz_track_id: u64,
    ) -> Result<Option<i64>, LibraryError> {
        self.conn
            .query_row(
                "SELECT id FROM local_tracks WHERE qobuz_track_id = ?1",
                params![qobuz_track_id as i64],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| LibraryError::Database(e.to_string()))
    }

    /// Get local track ID by metadata (title, artist, album)
    pub fn get_local_track_id_by_metadata(
        &self,
        title: &str,
        artist: &str,
        album: &str,
    ) -> Result<Option<i64>, LibraryError> {
        let title_lower = title.to_lowercase();
        let artist_lower = artist.to_lowercase();
        let album_lower = album.to_lowercase();

        self.conn
            .query_row(
                "SELECT id FROM local_tracks
                 WHERE LOWER(title) = ?1 AND LOWER(artist) = ?2 AND LOWER(album) = ?3
                 LIMIT 1",
                params![title_lower, artist_lower, album_lower],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| LibraryError::Database(e.to_string()))
    }

    /// Batch check which track IDs have local copies
    /// Returns a set of Qobuz track IDs that have local versions
    pub fn get_tracks_with_local_copies(
        &self,
        qobuz_track_ids: &[u64],
    ) -> Result<std::collections::HashSet<u64>, LibraryError> {
        use std::collections::HashSet;

        if qobuz_track_ids.is_empty() {
            return Ok(HashSet::new());
        }

        // Build placeholders for IN clause
        let placeholders: Vec<String> = (1..=qobuz_track_ids.len())
            .map(|i| format!("?{}", i))
            .collect();
        let placeholders_str = placeholders.join(",");

        let query = format!(
            "SELECT DISTINCT qobuz_track_id FROM local_tracks WHERE qobuz_track_id IN ({})",
            placeholders_str
        );

        let mut stmt = self
            .conn
            .prepare(&query)
            .map_err(|e| LibraryError::Database(e.to_string()))?;

        let params: Vec<rusqlite::types::Value> = qobuz_track_ids
            .iter()
            .map(|&id| rusqlite::types::Value::Integer(id as i64))
            .collect();

        let rows = stmt
            .query_map(rusqlite::params_from_iter(params.iter()), |row| {
                row.get::<_, i64>(0)
            })
            .map_err(|e| LibraryError::Database(e.to_string()))?;

        let mut result = HashSet::new();
        for row in rows {
            if let Ok(id) = row {
                result.insert(id as u64);
            }
        }

        Ok(result)
    }

    /// Update the has_local_content status for a playlist
    pub fn update_playlist_local_content_status(
        &self,
        qobuz_playlist_id: u64,
        status: LocalContentStatus,
    ) -> Result<(), LibraryError> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);

        // First check if settings exist, if not create default
        let existing = self.get_playlist_settings(qobuz_playlist_id)?;
        if existing.is_none() {
            let mut settings = PlaylistSettings::default();
            settings.qobuz_playlist_id = qobuz_playlist_id;
            settings.has_local_content = status;
            return self.save_playlist_settings(&settings);
        }

        self.conn
            .execute(
                "UPDATE playlist_settings SET has_local_content = ?1, updated_at = ?2
             WHERE qobuz_playlist_id = ?3",
                params![status.as_str(), now, qobuz_playlist_id as i64],
            )
            .map_err(|e| {
                LibraryError::Database(format!(
                    "Failed to update playlist local content status: {}",
                    e
                ))
            })?;

        Ok(())
    }

    /// Get playlists filtered by local content status
    pub fn get_playlists_by_local_content(
        &self,
        include_partial: bool,
    ) -> Result<Vec<PlaylistSettings>, LibraryError> {
        let query = if include_partial {
            "SELECT qobuz_playlist_id, custom_artwork_path, sort_by, sort_order,
                    last_search_query, notes, hidden, position, has_local_content, is_favorite, folder_id, created_at, updated_at
             FROM playlist_settings
             WHERE has_local_content IN ('some_local', 'all_local')
             ORDER BY position ASC, updated_at DESC"
        } else {
            "SELECT qobuz_playlist_id, custom_artwork_path, sort_by, sort_order,
                    last_search_query, notes, hidden, position, has_local_content, is_favorite, folder_id, created_at, updated_at
             FROM playlist_settings
             WHERE has_local_content = 'all_local'
             ORDER BY position ASC, updated_at DESC"
        };

        let mut stmt = self
            .conn
            .prepare(query)
            .map_err(|e| LibraryError::Database(format!("Failed to prepare statement: {}", e)))?;

        let settings = stmt
            .query_map([], |row| {
                Ok(PlaylistSettings {
                    qobuz_playlist_id: row.get::<_, i64>(0)? as u64,
                    custom_artwork_path: row.get(1)?,
                    sort_by: row.get(2)?,
                    sort_order: row.get(3)?,
                    last_search_query: row.get(4)?,
                    notes: row.get(5)?,
                    hidden: row.get::<_, i32>(6)? != 0,
                    position: row.get(7)?,
                    has_local_content: LocalContentStatus::from_str(
                        &row.get::<_, Option<String>>(8)?.unwrap_or_default(),
                    ),
                    is_favorite: row.get::<_, i32>(9).unwrap_or(0) != 0,
                    folder_id: row.get(10)?,
                    created_at: row.get(11)?,
                    updated_at: row.get(12)?,
                })
            })
            .map_err(|e| {
                LibraryError::Database(format!("Failed to query playlists by local content: {}", e))
            })?;

        settings
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| LibraryError::Database(format!("Failed to collect playlists: {}", e)))
    }

    // ── Downloaded Purchases Registry ──

    /// Record a track as downloaded on this computer with its format.
    pub fn mark_purchase_downloaded(
        &self,
        track_id: i64,
        album_id: Option<&str>,
        file_path: &str,
        format_id: i64,
    ) -> Result<(), LibraryError> {
        self.conn
            .execute(
                "INSERT OR REPLACE INTO downloaded_purchases (track_id, format_id, album_id, file_path, downloaded_at)
                 VALUES (?1, ?2, ?3, ?4, datetime('now'))",
                rusqlite::params![track_id, format_id, album_id, file_path],
            )
            .map_err(|e| {
                LibraryError::Database(format!("Failed to mark purchase downloaded: {}", e))
            })?;
        Ok(())
    }

    /// Every registered download of ONE purchased track: `(format_id,
    /// file_path)`, newest first. This is the playback resolver's read
    /// (`purchase_playback_qt`): it decides per format and probes each path
    /// itself with the bounded reachability probe, so this accessor neither
    /// stats nor prunes — the prune stays with `get_downloaded_purchase_track_ids`
    /// so there is exactly one writer.
    pub fn get_downloaded_purchase_files(
        &self,
        track_id: i64,
    ) -> Result<Vec<(i64, String)>, LibraryError> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT format_id, file_path FROM downloaded_purchases
                 WHERE track_id = ?1 ORDER BY downloaded_at DESC",
            )
            .map_err(|e| LibraryError::Database(format!("Failed to prepare statement: {}", e)))?;
        let rows = stmt
            .query_map(rusqlite::params![track_id], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|e| {
                LibraryError::Database(format!("Failed to query downloaded purchase files: {}", e))
            })?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| LibraryError::Database(format!("Failed to collect purchase files: {}", e)))
    }

    /// The DISTINCT folders the registered downloads of one purchased album
    /// sit in (a folder per format, e.g. `…/Album [DSF][DSD128]` and
    /// `…/Album [FLAC][16-bit,44.1kHz]`), newest first. No stat, no prune.
    pub fn get_downloaded_purchase_folders(
        &self,
        album_id: &str,
    ) -> Result<Vec<String>, LibraryError> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT file_path FROM downloaded_purchases
                 WHERE album_id = ?1 ORDER BY downloaded_at DESC",
            )
            .map_err(|e| LibraryError::Database(format!("Failed to prepare statement: {}", e)))?;
        let paths = stmt
            .query_map(rusqlite::params![album_id], |row| row.get::<_, String>(0))
            .map_err(|e| {
                LibraryError::Database(format!("Failed to query purchase folders: {}", e))
            })?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| LibraryError::Database(format!("Failed to collect folders: {}", e)))?;
        let mut folders: Vec<String> = Vec::new();
        for p in paths {
            if let Some(dir) = std::path::Path::new(&p).parent() {
                let dir = dir.to_string_lossy().into_owned();
                if !folders.contains(&dir) {
                    folders.push(dir);
                }
            }
        }
        Ok(folders)
    }

    /// Remove ONE downloaded purchase record (e.g. the user deleted the file).
    ///
    /// The table's primary key is `(track_id, format_id)` — the same purchased
    /// track may be downloaded in several formats at once, and the whole
    /// "downloaded" UI is format-scoped because of it. This delete is therefore
    /// keyed by the full pair. It previously deleted by `track_id` alone, which
    /// silently dropped the OTHER formats' rows; that never fired because nothing
    /// called it, and it is corrected here before Qt gives it a caller. The stale
    /// prune in `get_downloaded_purchase_track_ids` already deletes by the pair.
    pub fn remove_downloaded_purchase(
        &self,
        track_id: i64,
        format_id: i64,
    ) -> Result<(), LibraryError> {
        self.conn
            .execute(
                "DELETE FROM downloaded_purchases WHERE track_id = ?1 AND format_id = ?2",
                rusqlite::params![track_id, format_id],
            )
            .map_err(|e| {
                LibraryError::Database(format!("Failed to remove downloaded purchase: {}", e))
            })?;
        Ok(())
    }

    /// Remove EVERY downloaded record for a track, across all formats.
    ///
    /// Split out from `remove_downloaded_purchase` so that erasing all formats is
    /// something a caller has to ask for by name rather than something it gets by
    /// accident from an under-specified key.
    pub fn remove_downloaded_purchase_all_formats(
        &self,
        track_id: i64,
    ) -> Result<(), LibraryError> {
        self.conn
            .execute(
                "DELETE FROM downloaded_purchases WHERE track_id = ?1",
                [track_id],
            )
            .map_err(|e| {
                LibraryError::Database(format!("Failed to remove downloaded purchase: {}", e))
            })?;
        Ok(())
    }

    /// Count DISTINCT downloaded tracks per purchased album id.
    ///
    /// This is what makes the Albums tab's "downloaded" mark and its
    /// "Hide downloaded" filter work at all. The reference derived an album's
    /// downloaded state from `album.tracks.items` in the purchases response, but
    /// `getUserPurchases?type=albums` carries no nested tracks page (measured
    /// against a live account, contract §2.5b), so that predicate is unsatisfiable
    /// and the filter can never hide anything. The local registry is the only
    /// place that knows, and `album_id` — written since the table was created and
    /// never read until now — is the column that answers it. There is already an
    /// index on it (`idx_downloaded_purchases_album`).
    ///
    /// Counting is format-AGNOSTIC (`DISTINCT track_id`), matching the reference's
    /// list-level rule: a track downloaded in two formats counts once. Rows with a
    /// NULL `album_id` are skipped — they cannot be attributed to an album.
    ///
    /// **Rows whose file no longer exists on disk are excluded**, the same way
    /// `get_downloaded_purchase_track_ids` excludes them. Without that, an album
    /// whose files the user deleted keeps reporting as downloaded — and, worse,
    /// stays HIDDEN behind "Hide downloaded" — until some unrelated call happens
    /// to prune first. Making the answer depend on which accessor ran last is the
    /// kind of order-coupling nobody remembers six months later, so this query
    /// stands on its own.
    ///
    /// Unlike the track-ids accessor this one only FILTERS; it does not delete.
    /// Pruning is left to that accessor so there is exactly one writer, and a
    /// read taken while a download is mid-flight cannot race it.
    ///
    /// The caller compares each count against the album's `tracks_count`; this
    /// method deliberately does not decide "fully downloaded", because only the
    /// caller holds the purchase metadata.
    pub fn get_downloaded_purchase_album_counts(
        &self,
    ) -> Result<std::collections::HashMap<String, u32>, LibraryError> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT album_id, track_id, file_path FROM downloaded_purchases
                 WHERE album_id IS NOT NULL",
            )
            .map_err(|e| LibraryError::Database(format!("Failed to prepare statement: {}", e)))?;

        let rows: Vec<(String, i64, String)> = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .map_err(|e| {
                LibraryError::Database(format!("Failed to query purchase album counts: {}", e))
            })?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| {
                LibraryError::Database(format!("Failed to collect album counts: {}", e))
            })?;

        // DISTINCT over (album_id, track_id) is done here rather than in SQL so
        // the existence check can drop a row before it is counted.
        let mut seen: std::collections::HashSet<(String, i64)> = std::collections::HashSet::new();
        let mut counts: std::collections::HashMap<String, u32> = std::collections::HashMap::new();

        for (album_id, track_id, file_path) in rows {
            if probe_default(std::path::Path::new(&file_path)) != Reach::Present {
                continue;
            }
            if seen.insert((album_id.clone(), track_id)) {
                *counts.entry(album_id).or_insert(0) += 1;
            }
        }

        Ok(counts)
    }

    /// Get all downloaded track IDs for fast lookup (any format).
    /// Automatically removes stale entries only when the filesystem positively
    /// answers that the file is missing. Timeouts and I/O errors are treated as
    /// unreachable and never prune durable ownership/download state.
    pub fn get_downloaded_purchase_track_ids(&self) -> Result<Vec<i64>, LibraryError> {
        let mut stmt = self
            .conn
            .prepare("SELECT track_id, format_id, file_path FROM downloaded_purchases")
            .map_err(|e| LibraryError::Database(format!("Failed to prepare statement: {}", e)))?;

        let rows: Vec<(i64, i64, String)> = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .map_err(|e| {
                LibraryError::Database(format!("Failed to query downloaded purchases: {}", e))
            })?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| LibraryError::Database(format!("Failed to collect rows: {}", e)))?;

        let mut stale: Vec<(i64, i64)> = Vec::new();
        let mut valid_ids: Vec<i64> = Vec::new();

        for (track_id, format_id, file_path) in &rows {
            match probe_default(std::path::Path::new(file_path)) {
                Reach::Present => valid_ids.push(*track_id),
                Reach::Missing => stale.push((*track_id, *format_id)),
                Reach::Unreachable => {}
            }
        }

        // Remove stale entries where the file no longer exists
        if !stale.is_empty() {
            log::info!(
                "Removing {} stale downloaded_purchases entries (files deleted)",
                stale.len()
            );
            for (track_id, format_id) in &stale {
                let _ = self.conn.execute(
                    "DELETE FROM downloaded_purchases WHERE track_id = ?1 AND format_id = ?2",
                    rusqlite::params![track_id, format_id],
                );
            }
        }

        valid_ids.sort_unstable();
        valid_ids.dedup();
        Ok(valid_ids)
    }

    /// Get all downloaded (track_id, format_id) pairs for building per-format lookup.
    pub fn get_downloaded_purchase_formats(&self) -> Result<Vec<(i64, i64)>, LibraryError> {
        let mut stmt = self
            .conn
            .prepare("SELECT track_id, format_id FROM downloaded_purchases")
            .map_err(|e| LibraryError::Database(format!("Failed to prepare statement: {}", e)))?;

        let rows = stmt
            .query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)))
            .map_err(|e| {
                LibraryError::Database(format!("Failed to query downloaded purchases: {}", e))
            })?;

        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| LibraryError::Database(format!("Failed to collect formats: {}", e)))
    }
}

#[cfg(test)]
mod track_page_order_tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn artist_group_orders_by_the_track_artist_not_album_artist() {
        let tmp = TempDir::new().unwrap();
        let db = LibraryDatabase::open(&tmp.path().join("library.db")).unwrap();
        for (path, title, artist, album_artist, album) in [
            (
                "/music/zed.flac",
                "First",
                "Zed Performer",
                "Alpha Album Artist",
                "A Album",
            ),
            (
                "/music/alpha.flac",
                "Second",
                "Alpha Performer",
                "Zed Album Artist",
                "Z Album",
            ),
        ] {
            let track = LocalTrack {
                file_path: path.to_string(),
                title: title.to_string(),
                artist: artist.to_string(),
                album_artist: Some(album_artist.to_string()),
                album: album.to_string(),
                album_group_key: path.to_string(),
                album_group_title: album.to_string(),
                ..Default::default()
            };
            db.insert_track(&track).unwrap();
        }

        let grouped = db
            .search_with_filter_page("", 0, 10, true, false, "group-artist")
            .unwrap();
        assert_eq!(grouped[0].artist, "Alpha Performer");

        let album_artist_sorted = db
            .search_with_filter_page("", 0, 10, true, false, "artist-asc")
            .unwrap();
        assert_eq!(album_artist_sorted[0].artist, "Zed Performer");
    }
}

#[cfg(test)]
mod local_genre_and_filter_tests {
    use super::*;
    use tempfile::TempDir;

    fn fixture() -> (TempDir, LibraryDatabase) {
        let tmp = TempDir::new().unwrap();
        let db = LibraryDatabase::open(&tmp.path().join("library.db")).unwrap();
        (tmp, db)
    }

    fn insert(
        db: &LibraryDatabase,
        name: &str,
        format: AudioFormat,
        depth: Option<u32>,
        genres: &[&str],
    ) {
        let row = LocalTrack {
            file_path: format!("/music/Album/{name}"),
            title: name.to_string(),
            artist: "Fixture Artist".into(),
            album_artist: Some("Fixture Artist".into()),
            album: "Fixture Album".into(),
            album_group_key: "/music/Album".into(),
            album_group_title: "Fixture Album".into(),
            track_number: Some(if name.starts_with('a') { 1 } else { 2 }),
            genre: genres.first().map(|value| value.to_string()),
            genres: genres.iter().map(|value| value.to_string()).collect(),
            format,
            bit_depth: depth,
            sample_rate: 44_100.0,
            ..Default::default()
        };
        db.insert_track(&row).unwrap();
    }

    #[test]
    fn folder_album_unions_all_track_genres_case_insensitively() {
        let (_tmp, db) = fixture();
        insert(
            &db,
            "a.flac",
            AudioFormat::Flac,
            Some(24),
            &["Progressive Rock", "Art Rock"],
        );
        insert(
            &db,
            "b.mp3",
            AudioFormat::Mp3,
            None,
            &["art rock", "Psychedelic"],
        );
        let albums = db.get_albums_with_full_filter(false, true, false).unwrap();
        assert_eq!(albums.len(), 1);
        assert_eq!(
            albums[0].genres,
            ["Art Rock", "Progressive Rock", "Psychedelic"]
        );
    }

    #[test]
    fn local_format_and_quality_filters_run_before_page_limits() {
        let (_tmp, db) = fixture();
        insert(&db, "a.flac", AudioFormat::Flac, Some(24), &["Rock"]);
        insert(&db, "b.mp3", AudioFormat::Mp3, None, &["Rock"]);
        let rows = db
            .search_with_filter_page_faceted(
                "",
                0,
                1,
                true,
                false,
                "title-asc",
                &["mp3".to_string()],
                false,
                &["lossy".to_string()],
                &["local".to_string()],
            )
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].title, "b.mp3");
        assert_eq!(rows[0].genres, ["Rock"]);
    }
}

#[cfg(test)]
mod metadata_grouping_tests {
    use super::*;
    use tempfile::TempDir;

    fn fresh_db() -> (TempDir, LibraryDatabase) {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("library.db");
        let db = LibraryDatabase::open(&path).unwrap();
        (tmp, db)
    }

    fn insert_track_for_test(
        db: &LibraryDatabase,
        file_path: &str,
        album: Option<&str>,
        album_artist: Option<&str>,
        artist: &str,
        album_group_key: &str,
    ) {
        let mut t = LocalTrack::default();
        t.file_path = file_path.to_string();
        t.title = format!("Track at {}", file_path);
        t.album = album.unwrap_or("").to_string();
        t.album_artist = album_artist.map(String::from);
        t.artist = artist.to_string();
        t.album_group_key = album_group_key.to_string();
        t.album_group_title = album.unwrap_or("").to_string();
        db.insert_track(&t).unwrap();
    }

    #[test]
    fn metadata_group_merges_tracks_across_folders_with_same_album() {
        let (_tmp, db) = fresh_db();
        // Two folders, same album metadata -> one metadata group.
        insert_track_for_test(
            &db,
            "/m/Bjork/Vespertine/01.flac",
            Some("Vespertine"),
            Some("Bjork"),
            "Bjork",
            "/m/Bjork/Vespertine",
        );
        insert_track_for_test(
            &db,
            "/m/Bjork/Vespertine/02.flac",
            Some("Vespertine"),
            Some("Bjork"),
            "Bjork",
            "/m/Bjork/Vespertine",
        );
        insert_track_for_test(
            &db,
            "/m/mix/cd/track-from-vespertine.flac",
            Some("Vespertine"),
            Some("Bjork"),
            "Bjork",
            "/m/mix/cd",
        );

        let albums = db
            .get_albums_metadata_grouped(
                false,
                true,
                false,
                crate::album_grouping::AlbumGroupMode::Metadata,
            )
            .unwrap();
        let vespertine = albums
            .iter()
            .find(|a| a.title == "Vespertine")
            .expect("Vespertine group");
        assert_eq!(vespertine.track_count, 3);
    }

    #[test]
    fn metadata_group_falls_back_to_folder_when_album_missing() {
        let (_tmp, db) = fresh_db();
        // Empty album tag -> use folder grouping.
        insert_track_for_test(&db, "/m/folder/01.flac", None, None, "A", "/m/folder");
        insert_track_for_test(&db, "/m/folder/02.flac", None, None, "B", "/m/folder");

        let albums = db
            .get_albums_metadata_grouped(
                false,
                true,
                false,
                crate::album_grouping::AlbumGroupMode::Metadata,
            )
            .unwrap();
        assert_eq!(albums.len(), 1, "single folder fallback group");
        assert_eq!(albums[0].track_count, 2);
        assert_eq!(albums[0].artist, "Various Artists");
    }

    #[test]
    fn metadata_group_orphan_bucket_when_no_album_no_folder() {
        let (_tmp, db) = fresh_db();
        // No album tag AND no folder key -> orphan bucket.
        insert_track_for_test(&db, "/m/ghost/01.flac", None, None, "X", "");
        insert_track_for_test(&db, "/m/ghost/02.flac", None, None, "Y", "");

        let albums = db
            .get_albums_metadata_grouped(
                false,
                true,
                false,
                crate::album_grouping::AlbumGroupMode::Metadata,
            )
            .unwrap();
        let unknown = albums
            .iter()
            .find(|a| a.title == "Unknown Album")
            .expect("Unknown Album bucket");
        assert_eq!(unknown.track_count, 2);
    }

    #[test]
    fn metadata_group_va_detection() {
        let (_tmp, db) = fresh_db();
        // Same album, different track artists, album_artist set to VA.
        insert_track_for_test(
            &db,
            "/m/comp/01.flac",
            Some("Comp"),
            Some("Various Artists"),
            "A",
            "/m/comp",
        );
        insert_track_for_test(
            &db,
            "/m/comp/02.flac",
            Some("Comp"),
            Some("Various Artists"),
            "B",
            "/m/comp",
        );

        let albums = db
            .get_albums_metadata_grouped(
                false,
                true,
                false,
                crate::album_grouping::AlbumGroupMode::Metadata,
            )
            .unwrap();
        let comp = albums
            .iter()
            .find(|a| a.title == "Comp")
            .expect("Comp album");
        assert_eq!(comp.track_count, 2);
        assert_eq!(comp.artist, "Various Artists");
    }

    #[test]
    fn folder_group_mode_compilation_is_one_album() {
        let (_tmp, db) = fresh_db();
        // Saint Seiya case (spec 2026-07-19-local-album-grouping-mode §D):
        // one folder, same album tag, 10 distinct track artists, NO
        // album_artist. Metadata mode splits per track artist; Folder mode
        // keeps ONE card.
        for (i, artist) in [
            "MAKE-UP",
            "MAKE-UP PROJECT",
            "Horie",
            "Kageyama",
            "Furuya",
            "Trooper",
            "Matsuzawa",
            "Marina",
            "Broadway",
            "Oren",
        ]
        .iter()
        .enumerate()
        {
            insert_track_for_test(
                &db,
                &format!("/m/seiya/{:02}.flac", i + 1),
                Some("Saint Seiya Best"),
                None,
                artist,
                "/m/seiya",
            );
        }

        // Metadata mode: one group per album|artist pair (the #411 split).
        let albums = db
            .get_albums_metadata_grouped(
                false,
                true,
                false,
                crate::album_grouping::AlbumGroupMode::Metadata,
            )
            .unwrap();
        assert_eq!(albums.len(), 10, "metadata mode splits per track artist");

        // Folder mode: ONE album, Various Artists, everyone in all_artists.
        let albums = db
            .get_albums_metadata_grouped(
                false,
                true,
                false,
                crate::album_grouping::AlbumGroupMode::Folder,
            )
            .unwrap();
        assert_eq!(albums.len(), 1, "folder mode keeps the compilation whole");
        let comp = &albums[0];
        assert_eq!(comp.title, "Saint Seiya Best");
        assert_eq!(comp.artist, "Various Artists");
        assert_eq!(comp.track_count, 10);
        let all = comp.all_artists.as_str();
        for artist in ["MAKE-UP", "Horie", "Kageyama", "Marina"] {
            assert!(all.contains(artist), "all_artists carries {artist}");
        }

        // Same folder with album_artist set -> that artist, not VA.
        let (_tmp2, db2) = fresh_db();
        insert_track_for_test(
            &db2,
            "/m/eels/01.flac",
            Some("Beautiful Freak"),
            Some("EELS"),
            "EELS",
            "/m/eels",
        );
        insert_track_for_test(
            &db2,
            "/m/eels/02.flac",
            Some("Beautiful Freak"),
            Some("EELS"),
            "EELS",
            "/m/eels",
        );
        let albums = db2
            .get_albums_metadata_grouped(
                false,
                true,
                false,
                crate::album_grouping::AlbumGroupMode::Folder,
            )
            .unwrap();
        assert_eq!(albums.len(), 1);
        assert_eq!(albums[0].artist, "EELS");

        // Orphan bucket still works in folder mode (no folder key at all).
        let (_tmp3, db3) = fresh_db();
        insert_track_for_test(&db3, "/m/ghost/01.flac", None, None, "X", "");
        insert_track_for_test(&db3, "/m/ghost/02.flac", None, None, "Y", "");
        let albums = db3
            .get_albums_metadata_grouped(
                false,
                true,
                false,
                crate::album_grouping::AlbumGroupMode::Folder,
            )
            .unwrap();
        let unknown = albums
            .iter()
            .find(|a| a.title == "Unknown Album")
            .expect("orphan bucket in folder mode");
        assert_eq!(unknown.track_count, 2);
    }

    #[test]
    fn metadata_group_tracks_fetch_returns_all_in_group() {
        let (_tmp, db) = fresh_db();
        insert_track_for_test(
            &db,
            "/m/folderA/01.flac",
            Some("Album X"),
            Some("Artist Y"),
            "Artist Y",
            "/m/folderA",
        );
        insert_track_for_test(
            &db,
            "/m/folderB/02.flac",
            Some("Album X"),
            Some("Artist Y"),
            "Artist Y",
            "/m/folderB",
        );
        insert_track_for_test(
            &db,
            "/m/folderA/03.flac",
            Some("Album X"),
            Some("Artist Y"),
            "Artist Y",
            "/m/folderA",
        );
        // Different album in same folder set
        insert_track_for_test(
            &db,
            "/m/folderA/04.flac",
            Some("Album Z"),
            Some("Artist Y"),
            "Artist Y",
            "/m/folderA",
        );

        let key = "Album X|Artist Y";
        let tracks = db.get_album_tracks_metadata(key).unwrap();
        assert_eq!(tracks.len(), 3);
    }

    /// Like `insert_track_for_test`, but with control over
    /// `album_group_title` (the scan-time snapshot — folder name when the
    /// tag was missing) and `year`, for the #447/#507 regressions.
    #[allow(clippy::too_many_arguments)]
    fn insert_full_track_for_test(
        db: &LibraryDatabase,
        file_path: &str,
        album: &str,
        album_artist: Option<&str>,
        artist: &str,
        album_group_key: &str,
        album_group_title: &str,
        year: Option<u32>,
    ) {
        let mut t = LocalTrack::default();
        t.file_path = file_path.to_string();
        t.title = format!("Track at {}", file_path);
        t.album = album.to_string();
        t.album_artist = album_artist.map(String::from);
        t.artist = artist.to_string();
        t.album_group_key = album_group_key.to_string();
        t.album_group_title = album_group_title.to_string();
        t.year = year;
        db.insert_track(&t).unwrap();
    }

    #[test]
    fn metadata_group_respects_album_artist_over_mixed_track_artists() {
        // #507 core: every track carries the same Album Artist tag while the
        // per-track artists differ -> the album shows the album artist, NOT
        // "Various Artists".
        let (_tmp, db) = fresh_db();
        insert_full_track_for_test(
            &db,
            "/m/mix/t1.flac",
            "Mix Album",
            Some("Curated Artist"),
            "Artist A",
            "/m/mix",
            "mix",
            Some(2025),
        );
        insert_full_track_for_test(
            &db,
            "/m/mix/t2.flac",
            "Mix Album",
            Some("Curated Artist"),
            "Artist B",
            "/m/mix",
            "mix",
            Some(2025),
        );

        let albums = db
            .get_albums_metadata_grouped(
                false,
                true,
                false,
                crate::album_grouping::AlbumGroupMode::Metadata,
            )
            .unwrap();
        let mix = albums
            .iter()
            .find(|a| a.title == "Mix Album")
            .expect("Mix Album group");
        assert_eq!(mix.artist, "Curated Artist");
    }

    #[test]
    fn metadata_group_title_prefers_album_tag_over_folder_snapshot() {
        // #447 title: the live album tag differs from the scan-time
        // album_group_title snapshot (folder name) -> the tag wins.
        let (_tmp, db) = fresh_db();
        insert_full_track_for_test(
            &db,
            "/m/Alle Songs/t1.flac",
            "ALBUM.",
            Some("The Artist"),
            "The Artist",
            "/m/Alle Songs",
            "Alle Songs",
            Some(2025),
        );

        let albums = db
            .get_albums_metadata_grouped(
                false,
                true,
                false,
                crate::album_grouping::AlbumGroupMode::Metadata,
            )
            .unwrap();
        let a = albums
            .iter()
            .find(|a| a.title == "ALBUM.")
            .expect("album tag title");
        assert_eq!(a.title, "ALBUM.");
    }

    #[test]
    fn metadata_group_year_is_per_album_not_per_folder() {
        // #447 year: two tagged albums sharing one folder must split into
        // two metadata groups, each with its OWN year — a folder-level
        // group would MIN() them together and show the oldest year for both.
        let (_tmp, db) = fresh_db();
        insert_full_track_for_test(
            &db,
            "/m/Alle Songs/old.flac",
            "Old Album",
            Some("X"),
            "X",
            "/m/Alle Songs",
            "Alle Songs",
            Some(2004),
        );
        insert_full_track_for_test(
            &db,
            "/m/Alle Songs/new1.flac",
            "New Album",
            Some("X"),
            "X",
            "/m/Alle Songs",
            "Alle Songs",
            Some(2025),
        );
        insert_full_track_for_test(
            &db,
            "/m/Alle Songs/new2.flac",
            "New Album",
            Some("X"),
            "X",
            "/m/Alle Songs",
            "Alle Songs",
            Some(2025),
        );

        let albums = db
            .get_albums_metadata_grouped(
                false,
                true,
                false,
                crate::album_grouping::AlbumGroupMode::Metadata,
            )
            .unwrap();
        let old = albums
            .iter()
            .find(|a| a.title == "Old Album")
            .expect("Old Album group");
        let new = albums
            .iter()
            .find(|a| a.title == "New Album")
            .expect("New Album group");
        assert_eq!(old.year, Some(2004));
        assert_eq!(new.year, Some(2025));
        assert_eq!(new.track_count, 2);
    }
}

#[cfg(test)]
mod folder_tree_tests {
    //! Tests for `list_folder_children` / `list_folder_tracks`.
    //!
    //! Fixture layout (paths only — metadata is enough to round-trip
    //! through `LocalTrack`):
    //!
    //! ```text
    //! /m/A/album1/t1.flac           (user)
    //! /m/A/album1/t2.flac           (user)
    //! /m/A/album1/Disc 1/t3.flac    (user, in subfolder)
    //! /m/A/album2/t4.flac           (user)
    //! /m/B/album3/t5.flac           (user)
    //! /m/A/album1/qcache.flac       (qobuz_download — must be filtered)
    //! /m/percent_test/100%.flac     (user, special chars)
    //! ```
    use super::*;
    use tempfile::TempDir;
    fn fresh_db() -> (TempDir, LibraryDatabase) {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("library.db");
        let db = LibraryDatabase::open(&path).unwrap();
        (tmp, db)
    }

    fn insert_at(
        db: &LibraryDatabase,
        file_path: &str,
        disc: Option<u32>,
        track_no: Option<u32>,
        title: &str,
    ) {
        // NB: `LibraryDatabase::insert_track` stamps `source` itself
        // (always 'user' unless the path matches a downloaded_purchases
        // row), so we never set track.source here. To insert with a
        // different source value (e.g. 'qobuz_download'), use
        // `insert_qobuz_download_at` below.
        let mut t = LocalTrack::default();
        t.file_path = file_path.to_string();
        t.title = title.to_string();
        t.album = "Test Album".to_string();
        t.album_artist = Some("Test Artist".to_string());
        t.artist = "Test Artist".to_string();
        t.album_group_key = file_path
            .rsplit_once('/')
            .map(|(parent, _)| parent.to_string())
            .unwrap_or_default();
        t.album_group_title = "Test Album".to_string();
        t.disc_number = disc;
        t.track_number = track_no;
        db.insert_track(&t).unwrap();
    }

    // ── §12-5: the gold purchase badge, end to end ───────────────────────
    //
    // The badge chain is: a purchase download writes `downloaded_purchases`
    // (file_path, format_id) → a later library scan inserts the same file →
    // `insert_track` stamps `source = 'qobuz_purchase'` → the UI branches on
    // that literal to draw the gold mark.
    //
    // The join is by EXACT `file_path` string equality, which is the whole
    // reason these tests exist. Nobody here can smoke-test Purchases, and a
    // path that differs by one character — a Unicode title sanitized
    // differently, a trailing space from an empty quality folder — breaks the
    // stamp silently: the file is there, the registry row is there, and the
    // badge simply never appears with nothing logged.

    #[test]
    fn a_scanned_file_matching_the_registry_is_stamped_as_a_purchase() {
        let tmp = tempfile::tempdir().unwrap();
        let db = LibraryDatabase::open(&tmp.path().join("lib.db")).unwrap();

        let path = "/music/Artist/Album [FLAC][24-bit,96kHz]/01 - Song.flac";
        db.mark_purchase_downloaded(1001, Some("alb-1"), path, 7)
            .unwrap();

        insert_at(&db, path, Some(1), Some(1), "Song");

        let source: String = db
            .with_connection(|conn| {
                conn.query_row(
                    "SELECT source FROM local_tracks WHERE file_path = ?1",
                    rusqlite::params![path],
                    |row| row.get(0),
                )
                .map_err(|e| LibraryError::Database(e.to_string()))
            })
            .unwrap();

        assert_eq!(
            source, "qobuz_purchase",
            "the scan must stamp a file that the purchase registry already knows"
        );
    }

    #[test]
    fn a_scanned_file_the_registry_does_not_know_stays_user() {
        let tmp = tempfile::tempdir().unwrap();
        let db = LibraryDatabase::open(&tmp.path().join("lib.db")).unwrap();

        insert_at(
            &db,
            "/music/Other/Album/01 - Song.flac",
            Some(1),
            Some(1),
            "Song",
        );

        let source: String = db
            .with_connection(|conn| {
                conn.query_row(
                    "SELECT source FROM local_tracks WHERE file_path = ?1",
                    rusqlite::params!["/music/Other/Album/01 - Song.flac"],
                    |row| row.get(0),
                )
                .map_err(|e| LibraryError::Database(e.to_string()))
            })
            .unwrap();

        assert_eq!(source, "user");
    }

    /// The failure mode the exact-equality join actually has. A registry row and
    /// a scanned file that differ by ONE character — here the trailing space an
    /// implementer gets by formatting the album folder as `"{album} {quality}"`
    /// with an empty quality — do not join, and the badge silently never appears.
    ///
    /// This is a characterisation test: it asserts the join is exact, so that if
    /// anyone ever makes it fuzzy they have to come here and say so deliberately.
    #[test]
    fn a_one_character_path_difference_breaks_the_stamp_silently() {
        let tmp = tempfile::tempdir().unwrap();
        let db = LibraryDatabase::open(&tmp.path().join("lib.db")).unwrap();

        db.mark_purchase_downloaded(1002, Some("alb-2"), "/music/A/Album /01 - S.flac", 6)
            .unwrap();
        insert_at(&db, "/music/A/Album/01 - S.flac", Some(1), Some(1), "S");

        let source: String = db
            .with_connection(|conn| {
                conn.query_row(
                    "SELECT source FROM local_tracks WHERE file_path = ?1",
                    rusqlite::params!["/music/A/Album/01 - S.flac"],
                    |row| row.get(0),
                )
                .map_err(|e| LibraryError::Database(e.to_string()))
            })
            .unwrap();

        assert_eq!(
            source, "user",
            "documented: the join is exact, so a one-character path drift loses the badge"
        );
    }

    /// A re-scan re-stamps. The contract records that if a scan runs BEFORE the
    /// registry write the row is stamped `'user'` until the next scan of that
    /// folder — this proves the "until" half, which is why no repair migration
    /// or re-stamp UI is owed.
    #[test]
    fn a_rescan_after_the_registry_write_upgrades_the_stamp() {
        let tmp = tempfile::tempdir().unwrap();
        let db = LibraryDatabase::open(&tmp.path().join("lib.db")).unwrap();
        let path = "/music/A/Album/01 - Late.flac";

        // Scan first: nothing in the registry yet.
        insert_at(&db, path, Some(1), Some(1), "Late");

        // The download registers afterwards, then the folder is scanned again.
        db.mark_purchase_downloaded(1003, Some("alb-3"), path, 6)
            .unwrap();
        insert_at(&db, path, Some(1), Some(1), "Late");

        let source: String = db
            .with_connection(|conn| {
                conn.query_row(
                    "SELECT source FROM local_tracks WHERE file_path = ?1",
                    rusqlite::params![path],
                    |row| row.get(0),
                )
                .map_err(|e| LibraryError::Database(e.to_string()))
            })
            .unwrap();

        assert_eq!(source, "qobuz_purchase", "the next scan re-stamps it");
    }

    /// Insert a row directly with `source = 'qobuz_download'`.
    /// `insert_track` overrides the source field so we go through raw
    /// SQL to model the offline-cache code path that DOES write that
    /// value.
    fn insert_qobuz_download_at(db: &LibraryDatabase, file_path: &str, title: &str) {
        db.with_connection(|conn| {
            conn.execute(
                "INSERT INTO local_tracks \
                 (file_path, title, artist, album, album_artist, \
                  track_number, disc_number, year, genre, catalog_number, \
                  duration_secs, format, bit_depth, sample_rate, channels, \
                  file_size_bytes, cue_file_path, cue_start_secs, cue_end_secs, \
                  artwork_path, last_modified, indexed_at, album_group_key, \
                  album_group_title, source, is_network_mount) \
                 VALUES (?1, ?2, 'X', 'X', 'X', \
                         1, 1, NULL, NULL, NULL, \
                         0, 'FLAC', NULL, 44100.0, 2, \
                         0, NULL, NULL, NULL, \
                         NULL, 0, 0, 'qcache', \
                         'qcache', 'qobuz_download', 0)",
                rusqlite::params![file_path, title],
            )
            .unwrap();
        });
    }

    fn seed_standard_fixture(db: &LibraryDatabase) {
        // Standard layout — 5 user tracks under /m/A and /m/B.
        insert_at(db, "/m/A/album1/t1.flac", Some(1), Some(1), "Alpha");
        insert_at(db, "/m/A/album1/t2.flac", Some(1), Some(2), "Beta");
        insert_at(db, "/m/A/album1/Disc 1/t3.flac", Some(1), Some(1), "Gamma");
        insert_at(db, "/m/A/album2/t4.flac", Some(1), Some(1), "Delta");
        insert_at(db, "/m/B/album3/t5.flac", Some(1), Some(1), "Epsilon");

        // One Qobuz download in the same album — must be filtered out
        // by both list_folder_children and list_folder_tracks.
        insert_qobuz_download_at(db, "/m/A/album1/qcache.flac", "QobuzCache");
    }

    fn seed_sacd_fixture(db: &LibraryDatabase) {
        let image = "/m/SACD/Opera.iso";
        db.with_connection(|conn| {
            conn.execute(
                "INSERT INTO local_sacd_images \
                     (fingerprint,image_path,image_size_bytes,image_modified_ns,observed_at) \
                 VALUES ('sacd-folder-test',?1,1,1,1)",
                rusqlite::params![image],
            )
            .unwrap();
        });
        for (number, title) in [(1, "Prelude / Dawn"), (2, "Finale")] {
            let path = format!("sacd:{image}#{number}");
            insert_at(db, &path, Some(1), Some(number), title);
            db.with_connection(|conn| {
                let id: i64 = conn
                    .query_row(
                        "SELECT id FROM local_tracks WHERE file_path=?1",
                        rusqlite::params![path],
                        |row| row.get(0),
                    )
                    .unwrap();
                conn.execute(
                    "INSERT INTO local_sacd_tracks \
                         (fingerprint,track_number,local_track_id) \
                     VALUES ('sacd-folder-test',?1,?2)",
                    rusqlite::params![number, id],
                )
                .unwrap();
            });
        }
    }

    #[test]
    fn sacd_image_is_an_expandable_folder_with_virtual_track_children() {
        let (_tmp, db) = fresh_db();
        seed_sacd_fixture(&db);

        let root = db.list_folder_children("/m/SACD", false).unwrap();
        assert!(matches!(
            root.as_slice(),
            [FolderTreeEntry::Folder {
                path,
                segment,
                track_count_under: 2,
                ..
            }] if path == "/m/SACD/Opera.iso" && segment == "Opera.iso"
        ));

        let image = db
            .list_folder_children("/m/SACD/Opera.iso", false)
            .unwrap();
        assert_eq!(image.len(), 2);
        assert!(matches!(
            &image[0],
            FolderTreeEntry::Track { path, segment }
                if path == "sacd:/m/SACD/Opera.iso#1"
                    && segment == "001 - Prelude ∕ Dawn"
        ));
        assert!(db.list_folder_tracks("/m/SACD", false).unwrap().is_empty());
        assert_eq!(
            db.list_folder_tracks("/m/SACD/Opera.iso", false)
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            db.list_folder_tracks_recursive("/m/SACD", false)
                .unwrap()
                .len(),
            2
        );
        assert_eq!(db.count_folder_tracks_recursive("/m/SACD", false).unwrap(), 2);
    }

    #[test]
    fn list_folder_children_returns_folders_before_tracks() {
        let (_tmp, db) = fresh_db();
        seed_standard_fixture(&db);

        // /m/A/album1 has: subfolder "Disc 1", tracks "t1.flac" + "t2.flac".
        // Expected order: folder first, then tracks alphabetical.
        let children = db.list_folder_children("/m/A/album1", false).unwrap();
        assert_eq!(children.len(), 3, "one folder + two tracks expected");

        match &children[0] {
            FolderTreeEntry::Folder {
                segment,
                track_count_under,
                path,
                ..
            } => {
                assert_eq!(segment, "Disc 1");
                assert_eq!(*track_count_under, 1);
                assert_eq!(path, "/m/A/album1/Disc 1");
            }
            other => panic!("expected folder first, got {:?}", other),
        }
        match &children[1] {
            FolderTreeEntry::Track { segment, path } => {
                assert_eq!(segment, "t1.flac");
                assert_eq!(path, "/m/A/album1/t1.flac");
            }
            other => panic!("expected track at index 1, got {:?}", other),
        }
        match &children[2] {
            FolderTreeEntry::Track { segment, .. } => {
                assert_eq!(segment, "t2.flac");
            }
            other => panic!("expected track at index 2, got {:?}", other),
        }
    }

    #[test]
    fn list_folder_children_filters_qobuz_downloads() {
        let (_tmp, db) = fresh_db();
        seed_standard_fixture(&db);

        let children = db.list_folder_children("/m/A/album1", false).unwrap();
        // qcache.flac (qobuz_download) must not appear, even though it
        // shares the same parent folder as t1.flac/t2.flac.
        for entry in &children {
            if let FolderTreeEntry::Track { segment, .. } = entry {
                assert_ne!(
                    segment, "qcache.flac",
                    "qobuz_download row leaked into tree"
                );
            }
        }

        // Track count under "Disc 1" should also exclude any qobuz rows
        // (none here, but the filter must hold even at folder level).
        let folder_count = children
            .iter()
            .filter_map(|e| match e {
                FolderTreeEntry::Folder {
                    track_count_under, ..
                } => Some(*track_count_under),
                _ => None,
            })
            .sum::<u32>();
        assert_eq!(folder_count, 1, "Disc 1 contains exactly 1 user track");
    }

    #[test]
    fn list_folder_children_handles_special_chars_in_path() {
        let (_tmp, db) = fresh_db();
        seed_standard_fixture(&db);

        // Folder containing a literal '%' in the filename. Without
        // escape_like_pattern, the '%' would behave as a wildcard and
        // either over-match or fail to match.
        insert_at(
            &db,
            "/m/percent_test/100%.flac",
            Some(1),
            Some(1),
            "Hundred",
        );
        // A second literal-percent path that should NOT show up under
        // /m/percent_test (different parent).
        insert_at(
            &db,
            "/m/percent_other/200%.flac",
            Some(1),
            Some(1),
            "Two Hundred",
        );

        let children = db.list_folder_children("/m/percent_test", false).unwrap();
        assert_eq!(children.len(), 1, "only the local 100% file matches");
        match &children[0] {
            FolderTreeEntry::Track { segment, path } => {
                assert_eq!(segment, "100%.flac");
                assert_eq!(path, "/m/percent_test/100%.flac");
            }
            other => panic!("expected single track, got {:?}", other),
        }

        // And vice-versa — also test underscore handling in the parent.
        // /m/percent_test contains an '_' char that LIKE would match
        // any single character. If escape_like_pattern is missing, a
        // sibling like /m/percentXtest/foo.flac would also match.
        insert_at(&db, "/m/percentXtest/decoy.flac", Some(1), Some(1), "Decoy");
        let children = db.list_folder_children("/m/percent_test", false).unwrap();
        assert_eq!(
            children.len(),
            1,
            "underscore in parent path must be escaped"
        );
    }

    #[test]
    fn list_folder_tracks_excludes_subfolder_contents() {
        let (_tmp, db) = fresh_db();
        seed_standard_fixture(&db);

        // /m/A/album1 has direct tracks t1.flac + t2.flac, plus one
        // track in a subfolder (Disc 1/t3.flac). The latter must NOT
        // appear in list_folder_tracks output.
        let tracks = db.list_folder_tracks("/m/A/album1", false).unwrap();
        assert_eq!(tracks.len(), 2, "subfolder tracks must be excluded");
        let titles: Vec<_> = tracks.iter().map(|t| t.title.as_str()).collect();
        assert!(titles.contains(&"Alpha"));
        assert!(titles.contains(&"Beta"));
        assert!(
            !titles.contains(&"Gamma"),
            "Disc 1/t3.flac leaked into direct-children list"
        );

        // Qobuz download must also be excluded from direct tracks.
        assert!(
            !titles.contains(&"QobuzCache"),
            "qobuz_download row leaked into direct-children list"
        );
    }

    #[test]
    fn list_folder_tracks_orders_by_disc_track_title() {
        let (_tmp, db) = fresh_db();
        // Build a small fixture deliberately out of natural sort order.
        // Expected sort: disc ASC, track ASC, title ASC (NOCASE).
        insert_at(&db, "/m/order/disc2-track1.flac", Some(2), Some(1), "D2T1");
        insert_at(&db, "/m/order/disc1-track2.flac", Some(1), Some(2), "D1T2");
        insert_at(
            &db,
            "/m/order/disc1-track1-bee.flac",
            Some(1),
            Some(1),
            "Bee",
        );
        insert_at(
            &db,
            "/m/order/disc1-track1-ant.flac",
            Some(1),
            Some(1),
            "ant", // lowercase — NOCASE collation should sort ant < Bee
        );

        let tracks = db.list_folder_tracks("/m/order", false).unwrap();
        let titles: Vec<_> = tracks.iter().map(|track| track.title.clone()).collect();
        assert_eq!(titles, vec!["ant", "Bee", "D1T2", "D2T1"]);
    }

    #[test]
    fn list_folder_tracks_recursive_includes_all_descendants() {
        let (_tmp, db) = fresh_db();
        seed_standard_fixture(&db);

        // /m/A/album1 has direct tracks t1.flac, t2.flac AND a deeper
        // file at /m/A/album1/Disc 1/t3.flac. The recursive listing
        // must return all three.
        let tracks = db
            .list_folder_tracks_recursive("/m/A/album1", false)
            .unwrap();
        let titles: Vec<_> = tracks.iter().map(|track| track.title.clone()).collect();
        assert_eq!(
            tracks.len(),
            3,
            "recursive listing must include subfolder tracks"
        );
        assert!(titles.contains(&"Alpha".to_string()));
        assert!(titles.contains(&"Beta".to_string()));
        assert!(titles.contains(&"Gamma".to_string()));

        // Qobuz download under the same parent must NOT appear.
        assert!(
            !titles.contains(&"QobuzCache".to_string()),
            "qobuz_download row leaked into recursive listing"
        );
    }

    #[test]
    fn list_folder_tracks_recursive_orders_by_file_path() {
        let (_tmp, db) = fresh_db();
        // Insert files deliberately out of file_path order — recursive
        // listing must return them sorted ASC by file_path.
        insert_at(&db, "/m/r/zeta.flac", Some(1), Some(1), "Z");
        insert_at(&db, "/m/r/alpha.flac", Some(1), Some(1), "A");
        insert_at(&db, "/m/r/sub/middle.flac", Some(1), Some(1), "M");

        let tracks = db.list_folder_tracks_recursive("/m/r", false).unwrap();
        let paths: Vec<_> = tracks.iter().map(|track| track.file_path.clone()).collect();
        assert_eq!(
            paths,
            vec![
                "/m/r/alpha.flac".to_string(),
                "/m/r/sub/middle.flac".to_string(),
                "/m/r/zeta.flac".to_string(),
            ]
        );
    }

    #[test]
    fn list_folder_tracks_recursive_handles_special_chars_in_path() {
        let (_tmp, db) = fresh_db();
        // Folder containing literal '_' that LIKE would otherwise treat
        // as a single-character wildcard. With escape_like_pattern, the
        // sibling /m/percentXtest must not contaminate the result set.
        insert_at(
            &db,
            "/m/percent_test/100%.flac",
            Some(1),
            Some(1),
            "Hundred",
        );
        insert_at(
            &db,
            "/m/percent_test/inner/200.flac",
            Some(1),
            Some(1),
            "TwoHundred",
        );
        insert_at(&db, "/m/percentXtest/decoy.flac", Some(1), Some(1), "Decoy");

        let tracks = db
            .list_folder_tracks_recursive("/m/percent_test", false)
            .unwrap();
        let titles: Vec<_> = tracks.iter().map(|track| track.title.clone()).collect();
        assert_eq!(tracks.len(), 2, "underscore in parent path must be escaped");
        assert!(titles.contains(&"Hundred".to_string()));
        assert!(titles.contains(&"TwoHundred".to_string()));
        assert!(!titles.contains(&"Decoy".to_string()));
    }

    #[test]
    fn list_folder_tracks_recursive_returns_empty_for_unknown_path() {
        // A folder path with no matching descendants must yield an empty
        // Vec rather than an error — frontend treats empty as "nothing
        // to play/queue" and skips the toast.
        let (_tmp, db) = fresh_db();
        let tracks = db
            .list_folder_tracks_recursive("/m/does/not/exist", false)
            .unwrap();
        assert!(tracks.is_empty());
    }

    /// Mirrors the offline / "exclude network folders" toggle: tracks
    /// living under a `library_folders` row marked `is_network = 1`
    /// must be filtered out of every tree-mode listing primitive when
    /// `exclude_network_folders = true`, and present when `false`.
    /// Matches the predicate used by `get_albums_with_full_filter`
    /// and `v2_library_search` so flat mode and tree mode see the same
    /// source of truth.
    #[test]
    fn list_folder_primitives_honor_network_exclude() {
        let (_tmp, db) = fresh_db();

        // Register two scan roots: one local, one flagged as network.
        db.add_folder_with_network_info("/m/local", false, None)
            .unwrap();
        db.add_folder_with_network_info("/m/net", true, Some("nfs"))
            .unwrap();

        // Seed user tracks under each root. The folder structure is
        // similar enough that the only thing distinguishing them is the
        // network-mount flag on the parent library_folders row.
        insert_at(&db, "/m/local/album/local1.flac", Some(1), Some(1), "L1");
        insert_at(&db, "/m/local/album/local2.flac", Some(1), Some(2), "L2");
        insert_at(&db, "/m/net/album/net1.flac", Some(1), Some(1), "N1");
        insert_at(&db, "/m/net/album/sub/net2.flac", Some(1), Some(1), "N2");

        // --- list_folder_children -----------------------------------
        // Without filter: both roots appear under '/m'.
        let all_children = db.list_folder_children("/m", false).unwrap();
        let segments: Vec<_> = all_children
            .iter()
            .filter_map(|e| match e {
                FolderTreeEntry::Folder { segment, .. } => Some(segment.as_str()),
                _ => None,
            })
            .collect();
        assert!(segments.contains(&"local"));
        assert!(segments.contains(&"net"));

        // With filter: the network root collapses out (no descendant
        // tracks survive the EXISTS predicate, so it stops aggregating).
        let filtered = db.list_folder_children("/m", true).unwrap();
        let segments: Vec<_> = filtered
            .iter()
            .filter_map(|e| match e {
                FolderTreeEntry::Folder { segment, .. } => Some(segment.as_str()),
                _ => None,
            })
            .collect();
        assert!(segments.contains(&"local"));
        assert!(
            !segments.contains(&"net"),
            "network folder leaked into tree rail when exclude=true"
        );

        // --- list_folder_tracks (direct children) ------------------
        let direct_all = db.list_folder_tracks("/m/net/album", false).unwrap();
        assert_eq!(
            direct_all.len(),
            1,
            "net1.flac must appear when exclude=false"
        );

        let direct_filtered = db.list_folder_tracks("/m/net/album", true).unwrap();
        assert!(
            direct_filtered.is_empty(),
            "network track leaked into direct-children listing when exclude=true"
        );

        // Local folder is unaffected by the toggle.
        let local_filtered = db.list_folder_tracks("/m/local/album", true).unwrap();
        assert_eq!(local_filtered.len(), 2);

        // --- list_folder_tracks_recursive --------------------------
        let recursive_all = db.list_folder_tracks_recursive("/m/net", false).unwrap();
        assert_eq!(
            recursive_all.len(),
            2,
            "both net tracks visible when exclude=false"
        );

        let recursive_filtered = db.list_folder_tracks_recursive("/m/net", true).unwrap();
        assert!(
            recursive_filtered.is_empty(),
            "network tracks leaked into recursive listing when exclude=true"
        );

        // Recursive listing on a non-network root still returns its
        // tracks even when exclude=true.
        let recursive_local = db.list_folder_tracks_recursive("/m/local", true).unwrap();
        assert_eq!(recursive_local.len(), 2);
    }

    /// `count_folder_tracks_recursive` mirrors
    /// `list_folder_tracks_recursive` row-for-row: same source filter
    /// (qobuz_download excluded), same network-folder NOT EXISTS
    /// predicate, same prefix-with-slash boundary. The count is what
    /// the tree-mode rail uses for top-level scan-root rows that don't
    /// come back through `list_folder_children`.
    #[test]
    fn count_folder_tracks_recursive_matches_listing_primitive() {
        let (_tmp, db) = fresh_db();
        seed_standard_fixture(&db);

        // /m/A/album1 has 3 user descendants (t1, t2, Disc 1/t3) and one
        // qobuz_download (qcache.flac) under the same parent. Recursive
        // count must include the deeper subfolder track AND exclude the
        // qobuz row.
        let count = db
            .count_folder_tracks_recursive("/m/A/album1", false)
            .unwrap();
        assert_eq!(count, 3);

        // /m as the root catches every user track in the fixture.
        let total = db.count_folder_tracks_recursive("/m", false).unwrap();
        assert_eq!(total, 5);

        // Unknown path returns 0 (no rows match the LIKE prefix).
        let empty = db
            .count_folder_tracks_recursive("/m/does/not/exist", false)
            .unwrap();
        assert_eq!(empty, 0);
    }

    /// Network-folder filter must apply to the count primitive too —
    /// otherwise the rail would advertise tracks that don't show up in
    /// the listing, recursive playback, or flat-mode search.
    #[test]
    fn count_folder_tracks_recursive_honors_network_exclude() {
        let (_tmp, db) = fresh_db();

        db.add_folder_with_network_info("/m/local", false, None)
            .unwrap();
        db.add_folder_with_network_info("/m/net", true, Some("nfs"))
            .unwrap();

        insert_at(&db, "/m/local/album/local1.flac", Some(1), Some(1), "L1");
        insert_at(&db, "/m/local/album/local2.flac", Some(1), Some(2), "L2");
        insert_at(&db, "/m/net/album/net1.flac", Some(1), Some(1), "N1");
        insert_at(&db, "/m/net/album/sub/net2.flac", Some(1), Some(1), "N2");

        // Without filter: every descendant counts.
        assert_eq!(
            db.count_folder_tracks_recursive("/m/net", false).unwrap(),
            2
        );
        assert_eq!(
            db.count_folder_tracks_recursive("/m/local", false).unwrap(),
            2
        );

        // With filter: network root collapses to 0, local stays.
        assert_eq!(db.count_folder_tracks_recursive("/m/net", true).unwrap(), 0);
        assert_eq!(
            db.count_folder_tracks_recursive("/m/local", true).unwrap(),
            2
        );
    }
}

#[cfg(test)]
mod sidecar_position_tests {
    use super::*;
    use tempfile::TempDir;

    fn fresh_db() -> (TempDir, LibraryDatabase) {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("library.db");
        let db = LibraryDatabase::open(&path).unwrap();
        (tmp, db)
    }

    /// Real `local_tracks` rows for the FK on `playlist_local_tracks`.
    /// Returns the library row ids in insertion order.
    fn seed_local_tracks(db: &LibraryDatabase, count: usize) -> Vec<i64> {
        (0..count)
            .map(|i| {
                let mut t = LocalTrack::default();
                t.file_path = format!("/t/track{i}.flac");
                t.title = format!("T{i}");
                t.artist = "A".into();
                t.album = "B".into();
                db.insert_track(&t).unwrap()
            })
            .collect()
    }

    fn local_positions(db: &LibraryDatabase, pid: u64) -> Vec<(i64, i32)> {
        let mut stmt = db
            .conn
            .prepare(
                "SELECT local_track_id, position FROM playlist_local_tracks
                 WHERE qobuz_playlist_id = ?1 ORDER BY local_track_id ASC",
            )
            .unwrap();
        stmt.query_map(params![pid as i64], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    }

    fn plex_positions(db: &LibraryDatabase, pid: u64) -> Vec<(String, i32)> {
        let mut stmt = db
            .conn
            .prepare(
                "SELECT plex_rating_key, position FROM playlist_plex_tracks
                 WHERE qobuz_playlist_id = ?1 ORDER BY plex_rating_key ASC",
            )
            .unwrap();
        stmt.query_map(params![pid as i64], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    }

    #[test]
    fn reindexing_a_track_keeps_its_rowid_so_playlists_survive_a_rescan() {
        let (_tmp, db) = fresh_db();
        let ids = seed_local_tracks(&db, 1);
        db.add_local_track_to_playlist(7, ids[0], 0).unwrap();
        assert_eq!(local_positions(&db, 7).len(), 1);

        // What a rescan does: the same file, re-extracted, inserted again.
        let mut again = LocalTrack::default();
        again.file_path = "/t/track0.flac".into();
        again.title = "T0 (retagged)".into();
        again.artist = "A".into();
        again.album = "B".into();
        let id_after = db.insert_track(&again).unwrap();

        assert_eq!(
            id_after, ids[0],
            "re-indexing must UPDATE in place; a new rowid orphans every \
             playlist_local_tracks row that points at the old one"
        );
        assert_eq!(
            db.get_track(ids[0]).unwrap().unwrap().title,
            "T0 (retagged)",
            "the row must still take the fresh metadata"
        );

        let rows = local_positions(&db, 7);
        assert_eq!(rows.len(), 1, "the playlist row must survive");
        let resolvable: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM playlist_local_tracks p
                 JOIN local_tracks t ON t.id = p.local_track_id
                 WHERE p.qobuz_playlist_id = 7",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(resolvable, 1, "and it must still resolve to a track");
    }

    #[test]
    fn next_position_empty_sidecar_appends_after_qobuz_block() {
        let (_tmp, db) = fresh_db();
        assert_eq!(db.next_playlist_sidecar_position(7, 50).unwrap(), 50);
        assert_eq!(db.next_playlist_sidecar_position(7, 0).unwrap(), 0);
    }

    #[test]
    fn next_position_dense_positions_match_count_formula() {
        let (_tmp, db) = fresh_db();
        let ids = seed_local_tracks(&db, 1);
        db.add_local_track_to_playlist(7, ids[0], 50).unwrap();
        db.add_plex_track_to_playlist(7, "k1", 51).unwrap();
        // count-based 50+1+1 == max+1 == 52.
        assert_eq!(db.next_playlist_sidecar_position(7, 50).unwrap(), 52);
    }

    #[test]
    fn next_position_gapped_positions_clear_the_stored_max() {
        let (_tmp, db) = fresh_db();
        // T3 regression: positions keep gaps after removals; the count
        // formula alone would re-issue 52 while 80 is still stored.
        let ids = seed_local_tracks(&db, 2);
        db.add_local_track_to_playlist(7, ids[0], 50).unwrap();
        db.add_local_track_to_playlist(7, ids[1], 80).unwrap();
        assert_eq!(db.next_playlist_sidecar_position(7, 50).unwrap(), 81);
    }

    #[test]
    fn next_position_legacy_low_positions_fall_back_to_counts() {
        let (_tmp, db) = fresh_db();
        // Legacy 0-based rows: max+1 == 2, but the merged list is 52 long.
        let ids = seed_local_tracks(&db, 1);
        db.add_local_track_to_playlist(7, ids[0], 0).unwrap();
        db.add_plex_track_to_playlist(7, "k1", 1).unwrap();
        assert_eq!(db.next_playlist_sidecar_position(7, 50).unwrap(), 52);
    }

    #[test]
    fn next_position_scoped_per_playlist() {
        let (_tmp, db) = fresh_db();
        let ids = seed_local_tracks(&db, 1);
        db.add_local_track_to_playlist(7, ids[0], 99).unwrap();
        assert_eq!(db.next_playlist_sidecar_position(8, 10).unwrap(), 10);
    }

    #[test]
    fn heal_without_collisions_is_a_noop() {
        let (_tmp, db) = fresh_db();
        let ids = seed_local_tracks(&db, 2);
        db.add_local_track_to_playlist(7, ids[0], 0).unwrap();
        db.add_local_track_to_playlist(7, ids[1], 5).unwrap();
        db.add_plex_track_to_playlist(7, "k1", 9).unwrap();
        let healed = db.heal_playlist_sidecar_positions(7, 50).unwrap();
        assert!(healed.is_empty(), "drift is normal (E7): {healed:?}");
        assert_eq!(local_positions(&db, 7), vec![(ids[0], 0), (ids[1], 5)]);
        assert_eq!(plex_positions(&db, 7), vec![("k1".into(), 9)]);
    }

    #[test]
    fn heal_within_table_collision_moves_the_later_claimant() {
        let (_tmp, db) = fresh_db();
        // Two legacy 0-based batches: 0,1 then 0 again (E1).
        let ids = seed_local_tracks(&db, 3);
        db.add_local_track_to_playlist(7, ids[0], 0).unwrap();
        db.add_local_track_to_playlist(7, ids[1], 1).unwrap();
        db.add_local_track_to_playlist(7, ids[2], 0).unwrap();
        let healed = db.heal_playlist_sidecar_positions(7, 10).unwrap();
        assert_eq!(healed.len(), 1, "{healed:?}");
        // First claimant (rowid order on the added_at tie) keeps slot 0;
        // the later one moves to the append region: max(10+3, 1+1) = 13.
        assert_eq!(
            local_positions(&db, 7),
            vec![(ids[0], 0), (ids[1], 1), (ids[2], 13)]
        );
    }

    #[test]
    fn heal_cross_table_collision_keeps_local_moves_plex() {
        let (_tmp, db) = fresh_db();
        // Tauri create-and-add writes local AND plex 0-based (E2).
        let ids = seed_local_tracks(&db, 2);
        db.add_local_track_to_playlist(7, ids[0], 0).unwrap();
        db.add_local_track_to_playlist(7, ids[1], 1).unwrap();
        db.add_plex_track_to_playlist(7, "k1", 0).unwrap();
        db.add_plex_track_to_playlist(7, "k2", 1).unwrap();
        let healed = db.heal_playlist_sidecar_positions(7, 0).unwrap();
        assert_eq!(healed.len(), 2, "{healed:?}");
        assert_eq!(local_positions(&db, 7), vec![(ids[0], 0), (ids[1], 1)]);
        // Plex rows append: max(0+4, 1+1) = 4 onward, stable order.
        assert_eq!(
            plex_positions(&db, 7),
            vec![("k1".into(), 4), ("k2".into(), 5)]
        );
    }

    #[test]
    fn heal_is_idempotent() {
        let (_tmp, db) = fresh_db();
        let ids = seed_local_tracks(&db, 2);
        db.add_local_track_to_playlist(7, ids[0], 0).unwrap();
        db.add_local_track_to_playlist(7, ids[1], 0).unwrap();
        assert!(!db.heal_playlist_sidecar_positions(7, 5).unwrap().is_empty());
        assert!(db.heal_playlist_sidecar_positions(7, 5).unwrap().is_empty());
    }
}

#[cfg(test)]
mod remote_union_tests {
    use super::*;
    use tempfile::TempDir;

    /// A local library with one album, plus a shared remote mirror holding one
    /// Jellyfin album and one Subsonic album.
    fn bench() -> (TempDir, LibraryDatabase, std::path::PathBuf) {
        let tmp = TempDir::new().unwrap();
        let db = LibraryDatabase::open(&tmp.path().join("library.db")).unwrap();

        let mut t = LocalTrack::default();
        t.file_path = "/m/local/01.flac".into();
        t.title = "Local One".into();
        t.album = "Local Album".into();
        t.album_artist = Some("Local Artist".into());
        t.artist = "Local Artist".into();
        t.album_group_key = "/m/local".into();
        t.album_group_title = "Local Album".into();
        t.duration_secs = 100;
        t.format = crate::AudioFormat::Flac;
        t.bit_depth = Some(16);
        t.sample_rate = 44100.0;
        db.insert_track(&t).unwrap();

        // The shared mirror, written with plain SQL so this test does not
        // depend on `qbz-media-cache` (which depends on nothing here, and must
        // keep not depending on it).
        let remote = tmp.path().join("remote_cache.db");
        let conn = rusqlite::Connection::open(&remote).unwrap();
        conn.execute_batch(
            "CREATE TABLE remote_cache_tracks (
                id INTEGER PRIMARY KEY AUTOINCREMENT, source TEXT NOT NULL,
                item_id TEXT NOT NULL, server_id TEXT NOT NULL DEFAULT '',
                library_id TEXT NOT NULL DEFAULT '', title TEXT NOT NULL DEFAULT '',
                artist TEXT NOT NULL DEFAULT '', album_artist TEXT NOT NULL DEFAULT '',
                album TEXT NOT NULL DEFAULT '', album_id TEXT NOT NULL DEFAULT '',
                track_number INTEGER, disc_number INTEGER,
                duration_ms INTEGER NOT NULL DEFAULT 0, year INTEGER, genre TEXT,
                container TEXT NOT NULL DEFAULT '', codec TEXT, bit_depth INTEGER,
                sample_rate_hz INTEGER, channels INTEGER, bitrate_kbps INTEGER,
                artwork_token TEXT, collection_artwork_token TEXT,
                size_bytes INTEGER, updated_at INTEGER NOT NULL,
                UNIQUE (source, item_id));",
        )
        .unwrap();
        let add = |source: &str, item: &str, album: &str, album_id: &str, depth: i64| {
            conn.execute(
                "INSERT INTO remote_cache_tracks
                   (source,item_id,title,artist,album_artist,album,album_id,
                    duration_ms,container,bit_depth,sample_rate_hz,updated_at)
                 VALUES (?1,?2,?3,?4,?4,?5,?6,120000,'flac',?7,96000,1)",
                rusqlite::params![
                    source,
                    item,
                    format!("{item} title"),
                    "Remote Artist",
                    album,
                    album_id,
                    depth
                ],
            )
            .unwrap();
        };
        add("jellyfin", "jf-1", "Jellyfin Album", "jf-alb", 24);
        add("jellyfin", "jf-2", "Jellyfin Album", "jf-alb", 24);
        add("subsonic", "sub-1", "Subsonic Album", "sub-alb", 16);
        (tmp, db, remote)
    }

    fn titles(
        db: &LibraryDatabase,
        remote: Option<&std::path::Path>,
        sources: &[&str],
    ) -> Vec<String> {
        let page = db
            .get_albums_metadata_page(
                0,
                50,
                None,
                "title",
                "asc",
                true,
                false,
                None,
                remote,
                sources,
                crate::album_grouping::AlbumGroupMode::Folder,
            )
            .unwrap();
        let mut v: Vec<String> = page.albums.iter().map(|a| a.title.clone()).collect();
        v.sort();
        v
    }

    /// Both mirrors' albums appear beside the local ones, in ONE result set —
    /// which is the entire point of unioning rather than concatenating pages.
    #[test]
    fn enabled_remote_albums_join_the_local_ones() {
        let (_t, db, remote) = bench();
        assert_eq!(
            titles(&db, Some(&remote), &["jellyfin", "subsonic"]),
            vec!["Jellyfin Album", "Local Album", "Subsonic Album"]
        );
    }

    /// Turning ONE source off hides ITS rows and leaves the other's alone. The
    /// mirror is shared, so this is a filter rather than a detach — and getting
    /// it wrong takes the wrong server's music off the screen.
    #[test]
    fn disabling_one_source_leaves_the_other_visible() {
        let (_t, db, remote) = bench();
        assert_eq!(
            titles(&db, Some(&remote), &["subsonic"]),
            vec!["Local Album", "Subsonic Album"]
        );
        assert_eq!(
            titles(&db, Some(&remote), &["jellyfin"]),
            vec!["Jellyfin Album", "Local Album"]
        );
    }

    /// No source enabled, or no mirror on disk: local-only, and NOT an error.
    /// Refusing to list any albums because a server is off would turn "Jellyfin
    /// is unplugged" into "your library is empty".
    #[test]
    fn no_enabled_sources_is_local_only_not_a_failure() {
        let (_t, db, remote) = bench();
        assert_eq!(titles(&db, Some(&remote), &[]), vec!["Local Album"]);
        assert_eq!(titles(&db, None, &["jellyfin"]), vec!["Local Album"]);
        let missing = std::path::PathBuf::from("/nonexistent/remote_cache.db");
        assert_eq!(
            titles(&db, Some(&missing), &["jellyfin"]),
            vec!["Local Album"]
        );
    }

    /// The two tracks of the Jellyfin album collapse into ONE card, and the
    /// quality the server reported survives the aggregation. A grouping bug
    /// here shows as duplicate cards, which is what the album_id key prevents.
    #[test]
    fn a_remote_album_aggregates_its_tracks_and_keeps_its_quality() {
        let (_t, db, remote) = bench();
        let page = db
            .get_albums_metadata_page(
                0,
                50,
                None,
                "title",
                "asc",
                true,
                false,
                None,
                Some(&remote),
                &["jellyfin"],
                crate::album_grouping::AlbumGroupMode::Folder,
            )
            .unwrap();
        let jf = page
            .albums
            .iter()
            .find(|a| a.title == "Jellyfin Album")
            .expect("the jellyfin album");
        assert_eq!(
            jf.track_count, 2,
            "the two tracks did not group into one card"
        );
        assert_eq!(jf.bit_depth, Some(24));
        assert_eq!(jf.sample_rate, 96000.0);
        assert_eq!(jf.source, "jellyfin", "the row lost its source word");
        assert_eq!(jf.sources, vec!["jellyfin"]);
        assert_eq!(
            jf.identity_tracks.len(),
            2,
            "cross-source association evidence was not published"
        );
        // The group key is PREFIXED, which is what lets the source claim the
        // card when it comes back with no source word attached.
        assert_eq!(jf.id, "jellyfin:jf-alb");
        assert_eq!(page.total, 2, "the count disagrees with the page it counts");
    }

    /// The interpolated filter is validated, not escaped. Anything that is not
    /// plain lowercase drops out, and an all-invalid list shows NOTHING rather
    /// than everything — the safe direction for a predicate that reaches SQL.
    #[test]
    fn the_source_filter_rejects_anything_that_is_not_a_source_word() {
        assert_eq!(
            remote_source_filter(&["jellyfin"]),
            "source IN ('jellyfin')"
        );
        assert_eq!(
            remote_source_filter(&["jellyfin", "subsonic"]),
            "source IN ('jellyfin', 'subsonic')"
        );
        assert_eq!(remote_source_filter(&[]), "0");
        assert_eq!(
            remote_source_filter(&["x'; DROP TABLE local_tracks; --"]),
            "0"
        );
        assert_eq!(remote_source_filter(&["Jellyfin"]), "0");
        assert_eq!(remote_source_filter(&[""]), "0");
        // One bad entry does not smuggle itself in beside a good one.
        assert_eq!(
            remote_source_filter(&["jellyfin", "'; --"]),
            "source IN ('jellyfin')"
        );
    }
}

#[cfg(test)]
mod audio_format_roundtrip_tests {
    use super::*;

    /// `parse_format` is the inverse of `AudioFormat`'s `Display`, and the
    /// scanner writes exactly what `Display` produces. This asserts the pair
    /// for EVERY variant rather than for one format, because the defect it
    /// guards is structural: `"DSD"` had no arm, so every DSD row read back
    /// as `Unknown` — the format printed "UNKNOWN", the quality badge said CD
    /// and the detail said "1-bit / 2822.4 kHz". A fold to `Unknown` is a
    /// VALID answer, which is why nothing else caught it.
    ///
    /// Adding a variant to `AudioFormat` without an arm here fails this test
    /// instead of shipping the same silent loss again.
    #[test]
    fn every_variant_survives_the_display_parse_round_trip() {
        let all = [
            AudioFormat::Flac,
            AudioFormat::Alac,
            AudioFormat::Wav,
            AudioFormat::Aiff,
            AudioFormat::Ape,
            AudioFormat::Mp3,
            AudioFormat::Dsd,
        ];
        for f in all {
            let written = f.to_string();
            let read_back = LibraryDatabase::parse_format(&written);
            assert_eq!(
                read_back, f,
                "AudioFormat::{f:?} is stored as {written:?} and read back as \
                 {read_back:?} — parse_format is missing its arm"
            );
        }
    }

    /// The `Unknown` fold stays reachable for genuinely unknown words; it just
    /// must not be where a KNOWN format lands.
    #[test]
    fn an_unknown_word_still_folds_to_unknown() {
        assert_eq!(LibraryDatabase::parse_format("opus"), AudioFormat::Unknown);
        assert_eq!(LibraryDatabase::parse_format(""), AudioFormat::Unknown);
    }

    /// The scanner's own casing must not matter (rows written by older builds).
    #[test]
    fn parsing_is_case_insensitive() {
        assert_eq!(LibraryDatabase::parse_format("dsd"), AudioFormat::Dsd);
        assert_eq!(LibraryDatabase::parse_format("Dsd"), AudioFormat::Dsd);
    }
}

#[cfg(test)]
mod metadata_editor_transaction_tests {
    use super::*;

    fn update(id: i64, title: &str) -> TrackMetadataUpdateFull {
        TrackMetadataUpdateFull {
            id,
            title: title.to_string(),
            artist: "Artist".to_string(),
            album: "Album".to_string(),
            album_artist: Some("Artist".to_string()),
            album_group_title: "Album".to_string(),
            track_number: Some(1),
            disc_number: Some(1),
            year: Some(2026),
            genre: Some("Rock".to_string()),
            catalog_number: Some("CAT-1".to_string()),
        }
    }

    #[test]
    fn metadata_and_artwork_update_only_exact_rows_and_rollback_on_stale_ids() {
        let tmp = tempfile::tempdir().unwrap();
        let mut db = LibraryDatabase::open(&tmp.path().join("library.db")).unwrap();
        let first = db
            .insert_track(&LocalTrack {
                file_path: "/music/album/01.flac".to_string(),
                title: "One".to_string(),
                artist: "Artist".to_string(),
                album: "Album".to_string(),
                album_group_key: "/music/album".to_string(),
                album_group_title: "Album".to_string(),
                artwork_path: Some("/art/disc-one.jpg".to_string()),
                ..LocalTrack::default()
            })
            .unwrap();
        let second = db
            .insert_track(&LocalTrack {
                file_path: "/music/album/02.flac".to_string(),
                title: "Two".to_string(),
                artist: "Artist".to_string(),
                album: "Album".to_string(),
                album_group_key: "/music/album".to_string(),
                album_group_title: "Album".to_string(),
                artwork_path: Some("/art/disc-two.jpg".to_string()),
                ..LocalTrack::default()
            })
            .unwrap();

        db.update_tracks_metadata_and_artwork_by_id(
            &[update(first, "Edited")],
            Some("/art/new.jpg"),
        )
        .unwrap();
        let first_row: (String, String) = db
            .conn
            .query_row(
                "SELECT title, artwork_path FROM local_tracks WHERE id = ?1",
                params![first],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        let second_art: String = db
            .conn
            .query_row(
                "SELECT artwork_path FROM local_tracks WHERE id = ?1",
                params![second],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            first_row,
            ("Edited".to_string(), "/art/new.jpg".to_string())
        );
        assert_eq!(second_art, "/art/disc-two.jpg");

        let error = db.update_tracks_metadata_and_artwork_by_id(
            &[update(first, "Must Roll Back"), update(i64::MAX, "Missing")],
            Some("/art/should-not-land.jpg"),
        );
        assert!(error.is_err());
        let after: (String, String) = db
            .conn
            .query_row(
                "SELECT title, artwork_path FROM local_tracks WHERE id = ?1",
                params![first],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(after, first_row);
    }
}

#[cfg(test)]
mod artist_image_cache_tests {
    use super::*;

    #[test]
    fn remote_refresh_preserves_custom_art_and_negative_results_are_fresh() {
        let tmp = tempfile::tempdir().unwrap();
        let db = LibraryDatabase::open(&tmp.path().join("library.db")).unwrap();

        db.cache_artist_image_with_canonical(
            "Beyonce",
            None,
            "custom",
            Some("/pictures/beyonce.jpg"),
            Some("Beyoncé"),
        )
        .unwrap();
        db.cache_artist_image_with_canonical(
            "Beyonce",
            Some("https://cdn.example/beyonce.jpg"),
            "qobuz",
            None,
            None,
        )
        .unwrap();
        let image = db.get_artist_image("Beyonce").unwrap().unwrap();
        assert_eq!(
            image.custom_image_path.as_deref(),
            Some("/pictures/beyonce.jpg")
        );
        assert_eq!(image.canonical_name.as_deref(), Some("Beyoncé"));

        db.cache_artist_image_with_canonical("No Portrait", None, "miss", None, None)
            .unwrap();
        assert!(db
            .artist_image_resolution_is_fresh("No Portrait", 60)
            .unwrap());
        assert!(db
            .get_artist_image("No Portrait")
            .unwrap()
            .unwrap()
            .image_url
            .is_none());
    }
}

#[cfg(all(test, unix))]
mod purchase_registry_reachability_tests {
    use super::*;
    use std::os::unix::fs::symlink;

    #[test]
    fn an_io_error_never_prunes_a_purchase_row() {
        let tmp = tempfile::tempdir().unwrap();
        let db = LibraryDatabase::open(&tmp.path().join("library.db")).unwrap();
        let loop_path = tmp.path().join("loop");
        symlink(&loop_path, &loop_path).unwrap();
        db.mark_purchase_downloaded(42, Some("album"), loop_path.to_str().unwrap(), 6)
            .unwrap();

        assert!(db.get_downloaded_purchase_track_ids().unwrap().is_empty());
        let remaining: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM downloaded_purchases", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(remaining, 1, "I/O errors are unreachable, never missing");
        crate::reachability::reset_cooldowns();
    }
}

#[cfg(test)]
mod schema_init_concurrency_tests {
    use super::*;
    use std::sync::{Arc, Barrier};

    #[test]
    fn concurrent_opens_serialize_a_legacy_schema_migration() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("library.db");
        let legacy = Connection::open(&path).unwrap();
        legacy
            .execute_batch(
                "CREATE TABLE local_sacd_images (
                    fingerprint TEXT PRIMARY KEY,
                    image_path TEXT NOT NULL UNIQUE,
                    image_size_bytes INTEGER NOT NULL,
                    image_modified_ns INTEGER NOT NULL,
                    observed_at INTEGER NOT NULL
                );",
            )
            .unwrap();
        drop(legacy);

        const THREADS: usize = 8;
        let barrier = Arc::new(Barrier::new(THREADS));
        let handles = (0..THREADS)
            .map(|_| {
                let path = path.clone();
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    LibraryDatabase::open(&path).map(drop)
                })
            })
            .collect::<Vec<_>>();
        for handle in handles {
            handle.join().unwrap().unwrap();
        }

        let db = Connection::open(&path).unwrap();
        let columns: i64 = db
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('local_sacd_images')
                  WHERE name='parser_revision'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(columns, 1);
    }
}
