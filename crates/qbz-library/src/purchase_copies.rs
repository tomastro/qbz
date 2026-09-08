//! Persistent, additive inventory for DRM-free Qobuz purchase copies.
//!
//! `downloaded_purchases` remains a compatibility pointer for old UI and the
//! scanner. This module is the authority for N physical copies: coverage is
//! always evaluated inside one `copy_id`, never by combining folders.

use crate::{FileProbe, LibraryDatabase, LibraryError};
use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

const LEGACY_MIGRATION_KEY: &str = "purchase_copies_legacy_migration_v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PurchasePlaybackMode {
    Qobuz,
    Purchase,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PurchaseAlbumPlaybackPreference {
    pub album_id: String,
    pub mode: PurchasePlaybackMode,
    pub format_id: Option<i64>,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PurchaseDownloadCopy {
    pub copy_id: String,
    pub album_id: String,
    pub format_id: i64,
    pub resolved_album_folder: String,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PurchaseDownloadCopyTrack {
    pub copy_id: String,
    pub track_id: i64,
    pub file_path: String,
    pub mime_type: Option<String>,
    pub container: Option<String>,
    pub sample_rate_hz: Option<u32>,
    pub bit_depth: Option<u32>,
    pub channels: Option<u16>,
    pub file_size: u64,
    pub modified_ns: Option<u128>,
    pub last_verified_at: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PurchaseTrackHealth {
    Healthy,
    Missing,
    Changed,
    Unreachable,
    Unreadable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PurchaseCopyHealth {
    CompleteHealthy,
    Partial,
    Missing,
    Changed,
    Unreachable,
    Unreadable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PurchaseCopySnapshot {
    pub copy: PurchaseDownloadCopy,
    pub health: PurchaseCopyHealth,
    pub downloaded_tracks: u32,
    pub healthy_tracks: u32,
    pub total_tracks: u32,
    pub tracks: Vec<(PurchaseDownloadCopyTrack, PurchaseTrackHealth)>,
}

pub(crate) fn init_schema(conn: &Connection) -> Result<(), LibraryError> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS purchase_download_copies (
            copy_id TEXT PRIMARY KEY,
            album_id TEXT NOT NULL,
            format_id INTEGER NOT NULL,
            resolved_album_folder TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_purchase_copies_album
            ON purchase_download_copies(album_id, format_id, updated_at DESC);

        CREATE TABLE IF NOT EXISTS purchase_download_copy_tracks (
            copy_id TEXT NOT NULL,
            track_id INTEGER NOT NULL,
            file_path TEXT NOT NULL,
            mime_type TEXT,
            container TEXT,
            sample_rate_hz INTEGER,
            bit_depth INTEGER,
            channels INTEGER,
            file_size INTEGER NOT NULL DEFAULT 0,
            modified_ns TEXT,
            last_verified_at INTEGER,
            PRIMARY KEY(copy_id, track_id),
            FOREIGN KEY(copy_id) REFERENCES purchase_download_copies(copy_id) ON DELETE CASCADE
        );
        CREATE INDEX IF NOT EXISTS idx_purchase_copy_tracks_track
            ON purchase_download_copy_tracks(track_id, copy_id);

        CREATE TABLE IF NOT EXISTS purchase_download_copy_expected_tracks (
            copy_id TEXT NOT NULL,
            track_id INTEGER NOT NULL,
            PRIMARY KEY(copy_id, track_id),
            FOREIGN KEY(copy_id) REFERENCES purchase_download_copies(copy_id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS purchase_album_playback_preferences (
            album_id TEXT PRIMARY KEY,
            mode TEXT NOT NULL CHECK(mode IN ('qobuz','purchase')),
            format_id INTEGER NULL,
            updated_at INTEGER NOT NULL,
            CHECK((mode = 'qobuz' AND format_id IS NULL)
               OR (mode = 'purchase' AND format_id IS NOT NULL))
        );
        "#,
    )
    .map_err(|e| LibraryError::Database(format!("purchase copies schema: {e}")))?;

    // A short-lived race in the first additive-registry migration could import
    // the same physical album folder twice. Heal those rows before enforcing
    // the identity invariant so old profiles do not keep rendering duplicate
    // copies forever.
    let repaired = deduplicate_copy_identities(conn)?;
    conn.execute(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_purchase_copies_identity
         ON purchase_download_copies(album_id, format_id, resolved_album_folder)",
        [],
    )
    .map_err(|e| LibraryError::Database(format!("purchase copy identity index: {e}")))?;
    if repaired > 0 {
        log::info!("Repaired {repaired} duplicate purchase download copy record(s)");
    }
    Ok(())
}

/// Merge duplicate physical identities without losing coverage. The newest
/// row wins conflicts for one track; disjoint track and expected-track rows
/// from older duplicates are folded into it before those rows are removed.
fn deduplicate_copy_identities(conn: &Connection) -> Result<usize, LibraryError> {
    let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate).map_err(db_error)?;
    let identities = {
        let mut stmt = tx
            .prepare(
                "SELECT album_id, format_id, resolved_album_folder
                 FROM purchase_download_copies
                 GROUP BY album_id, format_id, resolved_album_folder
                 HAVING COUNT(*) > 1",
            )
            .map_err(db_error)?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .map_err(db_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_error)?;
        rows
    };

    let mut removed = 0;
    for (album_id, format_id, folder) in identities {
        let copy_ids = {
            let mut stmt = tx
                .prepare(
                    "SELECT copy_id FROM purchase_download_copies
                     WHERE album_id = ?1 AND format_id = ?2 AND resolved_album_folder = ?3
                     ORDER BY updated_at DESC, created_at DESC, copy_id",
                )
                .map_err(db_error)?;
            let rows = stmt
                .query_map(params![album_id, format_id, folder], |row| row.get(0))
                .map_err(db_error)?
                .collect::<Result<Vec<String>, _>>()
                .map_err(db_error)?;
            rows
        };
        let Some(keeper) = copy_ids.first() else {
            continue;
        };
        tx.execute(
            "UPDATE purchase_download_copies
             SET created_at = (SELECT MIN(created_at) FROM purchase_download_copies
                               WHERE album_id = ?2 AND format_id = ?3
                                 AND resolved_album_folder = ?4),
                 updated_at = (SELECT MAX(updated_at) FROM purchase_download_copies
                               WHERE album_id = ?2 AND format_id = ?3
                                 AND resolved_album_folder = ?4)
             WHERE copy_id = ?1",
            params![keeper, album_id, format_id, folder],
        )
        .map_err(db_error)?;

        for duplicate in copy_ids.iter().skip(1) {
            tx.execute(
                "INSERT OR IGNORE INTO purchase_download_copy_expected_tracks(copy_id, track_id)
                 SELECT ?1, track_id FROM purchase_download_copy_expected_tracks
                 WHERE copy_id = ?2",
                params![keeper, duplicate],
            )
            .map_err(db_error)?;
            tx.execute(
                "INSERT OR IGNORE INTO purchase_download_copy_tracks
                 (copy_id, track_id, file_path, mime_type, container, sample_rate_hz,
                  bit_depth, channels, file_size, modified_ns, last_verified_at)
                 SELECT ?1, track_id, file_path, mime_type, container, sample_rate_hz,
                        bit_depth, channels, file_size, modified_ns, last_verified_at
                 FROM purchase_download_copy_tracks WHERE copy_id = ?2",
                params![keeper, duplicate],
            )
            .map_err(db_error)?;
            // Foreign keys are intentionally not enabled on the app's SQLite
            // connections, so remove children explicitly before the parent.
            tx.execute(
                "DELETE FROM purchase_download_copy_expected_tracks WHERE copy_id = ?1",
                [duplicate],
            )
            .map_err(db_error)?;
            tx.execute(
                "DELETE FROM purchase_download_copy_tracks WHERE copy_id = ?1",
                [duplicate],
            )
            .map_err(db_error)?;
            removed += tx
                .execute(
                    "DELETE FROM purchase_download_copies WHERE copy_id = ?1",
                    [duplicate],
                )
                .map_err(db_error)?;
        }
    }
    tx.commit().map_err(db_error)?;
    Ok(removed)
}

/// One-time, filesystem-free import of the compatibility registry. Grouping by
/// album + format + parent folder is what prevents an old split registry from
/// becoming one fabricated complete copy.
pub(crate) fn migrate_legacy_registry(conn: &Connection) -> Result<(), LibraryError> {
    // Serialize the marker check with the import itself. A process-local schema
    // lock cannot protect two QBZ processes opening the same profile at once.
    let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate).map_err(db_error)?;
    let done = tx
        .query_row(
            "SELECT value FROM library_kv WHERE key = ?1",
            [LEGACY_MIGRATION_KEY],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|e| LibraryError::Database(format!("read purchase migration marker: {e}")))?
        .is_some();
    if done {
        return tx.commit().map_err(db_error);
    }

    let mut stmt = tx
        .prepare(
            "SELECT track_id, format_id, album_id, file_path
             FROM downloaded_purchases WHERE album_id IS NOT NULL
             ORDER BY album_id, format_id, file_path",
        )
        .map_err(|e| LibraryError::Database(format!("read legacy purchase registry: {e}")))?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })
        .map_err(|e| LibraryError::Database(format!("query legacy purchase registry: {e}")))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| LibraryError::Database(format!("collect legacy purchase registry: {e}")))?;
    drop(stmt);

    let mut groups: BTreeMap<(String, i64, String), Vec<(i64, String)>> = BTreeMap::new();
    for (track_id, format_id, album_id, file_path) in rows {
        let Some(folder) = Path::new(&file_path).parent() else {
            continue;
        };
        groups
            .entry((album_id, format_id, folder.to_string_lossy().into_owned()))
            .or_default()
            .push((track_id, file_path));
    }

    let now = unix_now();
    for ((album_id, format_id, folder), tracks) in groups {
        let copy_id = Uuid::new_v4().to_string();
        tx.execute(
            "INSERT OR IGNORE INTO purchase_download_copies
             (copy_id, album_id, format_id, resolved_album_folder, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
            params![copy_id, album_id, format_id, folder, now],
        )
        .map_err(|e| LibraryError::Database(format!("migrate purchase copy: {e}")))?;
        let effective_copy_id: String = tx
            .query_row(
                "SELECT copy_id FROM purchase_download_copies
                 WHERE album_id = ?1 AND format_id = ?2 AND resolved_album_folder = ?3",
                params![album_id, format_id, folder],
                |row| row.get(0),
            )
            .map_err(|e| LibraryError::Database(format!("resolve migrated purchase copy: {e}")))?;
        for (track_id, file_path) in tracks {
            let container = Path::new(&file_path)
                .extension()
                .and_then(|value| value.to_str())
                .map(|value| value.to_ascii_lowercase());
            tx.execute(
                "INSERT OR IGNORE INTO purchase_download_copy_tracks
                 (copy_id, track_id, file_path, container, file_size)
                 VALUES (?1, ?2, ?3, ?4, 0)",
                params![effective_copy_id, track_id, file_path, container],
            )
            .map_err(|e| LibraryError::Database(format!("migrate purchase track: {e}")))?;
        }
    }
    tx.execute(
        "INSERT OR REPLACE INTO library_kv(key, value) VALUES (?1, '1')",
        [LEGACY_MIGRATION_KEY],
    )
    .map_err(|e| LibraryError::Database(format!("write purchase migration marker: {e}")))?;
    tx.commit()
        .map_err(|e| LibraryError::Database(format!("commit purchase migration: {e}")))
}

impl LibraryDatabase {
    pub fn create_purchase_copy(
        &self,
        album_id: &str,
        format_id: i64,
        resolved_album_folder: &Path,
    ) -> Result<PurchaseDownloadCopy, LibraryError> {
        let copy = PurchaseDownloadCopy {
            copy_id: Uuid::new_v4().to_string(),
            album_id: album_id.to_string(),
            format_id,
            resolved_album_folder: resolved_album_folder.to_string_lossy().into_owned(),
            created_at: unix_now(),
            updated_at: unix_now(),
        };
        self.with_connection(|conn| insert_copy(conn, &copy))?;
        Ok(copy)
    }

    /// Create a copy and its immutable album coverage manifest in one
    /// transaction. The manifest is the denominator used by playback after a
    /// restart; without it, a directory containing one healthy track could be
    /// mistaken for a complete album.
    pub fn create_purchase_copy_with_expected_tracks(
        &self,
        album_id: &str,
        format_id: i64,
        resolved_album_folder: &Path,
        expected_track_ids: &[i64],
    ) -> Result<PurchaseDownloadCopy, LibraryError> {
        let expected = normalize_expected_track_ids(expected_track_ids)?;
        let copy = PurchaseDownloadCopy {
            copy_id: Uuid::new_v4().to_string(),
            album_id: album_id.to_string(),
            format_id,
            resolved_album_folder: resolved_album_folder.to_string_lossy().into_owned(),
            created_at: unix_now(),
            updated_at: unix_now(),
        };
        self.with_connection(|conn| {
            let tx = conn.unchecked_transaction().map_err(db_error)?;
            insert_copy(&tx, &copy)?;
            insert_expected_tracks(&tx, &copy.copy_id, &expected)?;
            tx.commit().map_err(db_error)
        })?;
        Ok(copy)
    }

    /// Attach the exact album coverage used to validate a migrated copy.
    /// Existing rows are replaced atomically so a crash cannot leave a
    /// half-written denominator.
    pub fn set_purchase_copy_expected_tracks(
        &self,
        copy_id: &str,
        expected_track_ids: &[i64],
    ) -> Result<(), LibraryError> {
        let expected = normalize_expected_track_ids(expected_track_ids)?;
        self.with_connection(|conn| {
            if select_copy(conn, copy_id)?.is_none() {
                return Err(LibraryError::Database(format!(
                    "purchase copy {copy_id} does not exist"
                )));
            }
            let tx = conn.unchecked_transaction().map_err(db_error)?;
            tx.execute(
                "DELETE FROM purchase_download_copy_expected_tracks WHERE copy_id = ?1",
                [copy_id],
            )
            .map_err(db_error)?;
            insert_expected_tracks(&tx, copy_id, &expected)?;
            tx.commit().map_err(db_error)
        })
    }

    pub fn get_purchase_copy_expected_track_ids(
        &self,
        copy_id: &str,
    ) -> Result<Vec<i64>, LibraryError> {
        self.with_connection(|conn| select_expected_tracks(conn, copy_id))
    }

    pub fn upsert_purchase_copy_track(
        &self,
        track: &PurchaseDownloadCopyTrack,
    ) -> Result<(), LibraryError> {
        self.with_connection(|conn| {
            conn.execute(
                "INSERT INTO purchase_download_copy_tracks
                 (copy_id, track_id, file_path, mime_type, container, sample_rate_hz,
                  bit_depth, channels, file_size, modified_ns, last_verified_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                 ON CONFLICT(copy_id, track_id) DO UPDATE SET
                   file_path=excluded.file_path, mime_type=excluded.mime_type,
                   container=excluded.container, sample_rate_hz=excluded.sample_rate_hz,
                   bit_depth=excluded.bit_depth, channels=excluded.channels,
                   file_size=excluded.file_size, modified_ns=excluded.modified_ns,
                   last_verified_at=excluded.last_verified_at",
                params![
                    track.copy_id,
                    track.track_id,
                    track.file_path,
                    track.mime_type,
                    track.container,
                    track.sample_rate_hz,
                    track.bit_depth,
                    track.channels,
                    u64_to_i64(track.file_size),
                    track.modified_ns.map(|value| value.to_string()),
                    track.last_verified_at,
                ],
            )
            .and_then(|_| {
                conn.execute(
                    "UPDATE purchase_download_copies SET updated_at = ?2 WHERE copy_id = ?1",
                    params![track.copy_id, unix_now()],
                )
            })
            .map(|_| ())
            .map_err(|e| LibraryError::Database(format!("upsert purchase copy track: {e}")))
        })
    }

    pub fn get_purchase_copy(
        &self,
        copy_id: &str,
    ) -> Result<Option<PurchaseDownloadCopy>, LibraryError> {
        self.with_connection(|conn| select_copy(conn, copy_id))
    }

    pub fn get_purchase_copies_for_album(
        &self,
        album_id: &str,
    ) -> Result<Vec<PurchaseDownloadCopy>, LibraryError> {
        self.with_connection(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT copy_id, album_id, format_id, resolved_album_folder,
                            created_at, updated_at
                     FROM purchase_download_copies WHERE album_id = ?1
                     ORDER BY updated_at DESC, copy_id",
                )
                .map_err(db_error)?;
            let rows = stmt
                .query_map([album_id], copy_from_row)
                .map_err(db_error)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(db_error)?;
            Ok(rows)
        })
    }

    pub fn get_purchase_copy_tracks(
        &self,
        copy_id: &str,
    ) -> Result<Vec<PurchaseDownloadCopyTrack>, LibraryError> {
        self.with_connection(|conn| select_copy_tracks(conn, copy_id))
    }

    /// Forget one inventory row while leaving the user's files untouched.
    /// Any compatibility pointers that still name paths from this exact copy
    /// are removed with it; pointers promoted from another copy survive.
    pub fn delete_purchase_copy_record(&self, copy_id: &str) -> Result<bool, LibraryError> {
        self.with_connection(|conn| {
            let Some(copy) = select_copy(conn, copy_id)? else {
                return Ok(false);
            };
            let tracks = select_copy_tracks(conn, copy_id)?;
            let tx = conn.unchecked_transaction().map_err(db_error)?;
            for track in &tracks {
                tx.execute(
                    "DELETE FROM downloaded_purchases
                     WHERE track_id = ?1 AND format_id = ?2 AND album_id = ?3 AND file_path = ?4",
                    params![
                        track.track_id,
                        copy.format_id,
                        copy.album_id,
                        track.file_path
                    ],
                )
                .map_err(db_error)?;
            }
            tx.execute(
                "DELETE FROM purchase_download_copy_expected_tracks WHERE copy_id = ?1",
                [copy_id],
            )
            .map_err(db_error)?;
            tx.execute(
                "DELETE FROM purchase_download_copy_tracks WHERE copy_id = ?1",
                [copy_id],
            )
            .map_err(db_error)?;
            let removed = tx
                .execute(
                    "DELETE FROM purchase_download_copies WHERE copy_id = ?1",
                    [copy_id],
                )
                .map_err(db_error)?
                > 0;
            tx.execute(
                "DELETE FROM purchase_album_playback_preferences
                 WHERE album_id = ?1 AND mode = 'purchase' AND format_id = ?2
                   AND NOT EXISTS (
                     SELECT 1 FROM purchase_download_copies
                     WHERE album_id = ?1 AND format_id = ?2
                   )",
                params![copy.album_id, copy.format_id],
            )
            .map_err(db_error)?;
            tx.commit().map_err(db_error)?;
            Ok(removed)
        })
    }

    /// Atomically point the compatibility registry at `copy_id`, but only if
    /// that one copy covers every expected album track.
    pub fn promote_complete_purchase_copy(
        &self,
        copy_id: &str,
        expected_track_ids: &[i64],
    ) -> Result<bool, LibraryError> {
        self.with_connection(|conn| {
            let Some(copy) = select_copy(conn, copy_id)? else {
                return Ok(false);
            };
            let tracks = select_copy_tracks(conn, copy_id)?;
            let present: HashSet<i64> = tracks.iter().map(|track| track.track_id).collect();
            if expected_track_ids.is_empty()
                || !expected_track_ids
                    .iter()
                    .all(|track_id| present.contains(track_id))
            {
                return Ok(false);
            }

            let tx = conn.unchecked_transaction().map_err(db_error)?;
            for track in tracks
                .iter()
                .filter(|track| expected_track_ids.contains(&track.track_id))
            {
                tx.execute(
                    "INSERT OR REPLACE INTO downloaded_purchases
                     (track_id, format_id, album_id, file_path, downloaded_at)
                     VALUES (?1, ?2, ?3, ?4, datetime('now'))",
                    params![
                        track.track_id,
                        copy.format_id,
                        copy.album_id,
                        track.file_path
                    ],
                )
                .map_err(db_error)?;
            }
            tx.commit().map_err(db_error)?;
            Ok(true)
        })
    }

    pub fn purchase_playback_preference(
        &self,
        album_id: &str,
    ) -> Result<PurchaseAlbumPlaybackPreference, LibraryError> {
        self.with_connection(|conn| {
            conn.query_row(
                "SELECT album_id, mode, format_id, updated_at
                 FROM purchase_album_playback_preferences WHERE album_id = ?1",
                [album_id],
                |row| {
                    let mode = match row.get::<_, String>(1)?.as_str() {
                        "purchase" => PurchasePlaybackMode::Purchase,
                        _ => PurchasePlaybackMode::Qobuz,
                    };
                    Ok(PurchaseAlbumPlaybackPreference {
                        album_id: row.get(0)?,
                        mode,
                        format_id: row.get(2)?,
                        updated_at: row.get(3)?,
                    })
                },
            )
            .optional()
            .map_err(db_error)
            .map(|value| {
                value.unwrap_or(PurchaseAlbumPlaybackPreference {
                    album_id: album_id.to_string(),
                    mode: PurchasePlaybackMode::Qobuz,
                    format_id: None,
                    updated_at: 0,
                })
            })
        })
    }

    pub fn set_purchase_playback_preference(
        &self,
        album_id: &str,
        mode: PurchasePlaybackMode,
        format_id: Option<i64>,
    ) -> Result<PurchaseAlbumPlaybackPreference, LibraryError> {
        let normalized_format = match mode {
            PurchasePlaybackMode::Qobuz => None,
            PurchasePlaybackMode::Purchase => format_id,
        };
        if mode == PurchasePlaybackMode::Purchase && normalized_format.is_none() {
            return Err(LibraryError::Database(
                "purchase playback preference requires a format_id".to_string(),
            ));
        }
        let value = PurchaseAlbumPlaybackPreference {
            album_id: album_id.to_string(),
            mode,
            format_id: normalized_format,
            updated_at: unix_now(),
        };
        self.with_connection(|conn| {
            conn.execute(
                "INSERT INTO purchase_album_playback_preferences
                 (album_id, mode, format_id, updated_at) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(album_id) DO UPDATE SET mode=excluded.mode,
                   format_id=excluded.format_id, updated_at=excluded.updated_at",
                params![
                    value.album_id,
                    mode_word(value.mode),
                    value.format_id,
                    value.updated_at
                ],
            )
            .map_err(db_error)
        })?;
        Ok(value)
    }

    /// Snapshot all copies of an album. This performs bounded probes and must
    /// therefore be called away from a UI thread.
    pub fn inspect_purchase_copies(
        &self,
        album_id: &str,
        expected_track_ids: &[i64],
    ) -> Result<Vec<PurchaseCopySnapshot>, LibraryError> {
        let expected: HashSet<i64> = expected_track_ids.iter().copied().collect();
        let mut result = Vec::new();
        for copy in self.get_purchase_copies_for_album(album_id)? {
            let rows = self.get_purchase_copy_tracks(&copy.copy_id)?;
            let mut tracks = Vec::with_capacity(rows.len());
            let mut healthy_ids = HashSet::new();
            let mut saw_missing = false;
            let mut saw_changed = false;
            let mut saw_unreachable = false;
            let mut saw_unreadable = false;
            for row in rows {
                let health = track_health(&row);
                match health {
                    PurchaseTrackHealth::Healthy => {
                        healthy_ids.insert(row.track_id);
                    }
                    PurchaseTrackHealth::Missing => saw_missing = true,
                    PurchaseTrackHealth::Changed => saw_changed = true,
                    PurchaseTrackHealth::Unreachable => saw_unreachable = true,
                    PurchaseTrackHealth::Unreadable => saw_unreadable = true,
                }
                tracks.push((row, health));
            }
            let complete = !expected.is_empty()
                && expected
                    .iter()
                    .all(|track_id| healthy_ids.contains(track_id));
            let health = if complete {
                PurchaseCopyHealth::CompleteHealthy
            } else if saw_unreachable {
                PurchaseCopyHealth::Unreachable
            } else if saw_changed {
                PurchaseCopyHealth::Changed
            } else if saw_unreadable {
                PurchaseCopyHealth::Unreadable
            } else if saw_missing {
                PurchaseCopyHealth::Missing
            } else {
                PurchaseCopyHealth::Partial
            };
            result.push(PurchaseCopySnapshot {
                copy,
                health,
                downloaded_tracks: tracks
                    .iter()
                    .filter(|(row, _)| expected.contains(&row.track_id))
                    .count() as u32,
                healthy_tracks: healthy_ids.intersection(&expected).count() as u32,
                total_tracks: expected.len() as u32,
                tracks,
            });
        }
        Ok(result)
    }

    /// Exact-format candidates that are complete and healthy against their
    /// own persisted album manifest. Results retain copy recency order and
    /// never combine coverage between copy IDs. This performs bounded file
    /// probes and must run away from the UI thread.
    pub fn complete_healthy_purchase_track_candidates(
        &self,
        album_id: &str,
        format_id: i64,
        track_id: i64,
    ) -> Result<Vec<(PurchaseDownloadCopy, PurchaseDownloadCopyTrack)>, LibraryError> {
        let mut candidates = Vec::new();
        for copy in self
            .get_purchase_copies_for_album(album_id)?
            .into_iter()
            .filter(|copy| copy.format_id == format_id)
        {
            let expected: HashSet<i64> = self
                .get_purchase_copy_expected_track_ids(&copy.copy_id)?
                .into_iter()
                .collect();
            if expected.is_empty() || !expected.contains(&track_id) {
                continue;
            }
            let rows = self.get_purchase_copy_tracks(&copy.copy_id)?;
            let mut target = None;
            let mut healthy = HashSet::new();
            for row in rows
                .into_iter()
                .filter(|row| expected.contains(&row.track_id))
            {
                if track_health(&row) == PurchaseTrackHealth::Healthy {
                    healthy.insert(row.track_id);
                    if row.track_id == track_id {
                        target = Some(row);
                    }
                }
            }
            if expected.iter().all(|id| healthy.contains(id)) {
                if let Some(target) = target {
                    candidates.push((copy, target));
                }
            }
        }
        Ok(candidates)
    }
}

fn insert_copy(conn: &Connection, copy: &PurchaseDownloadCopy) -> Result<(), LibraryError> {
    conn.execute(
        "INSERT INTO purchase_download_copies
         (copy_id, album_id, format_id, resolved_album_folder, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            copy.copy_id,
            copy.album_id,
            copy.format_id,
            copy.resolved_album_folder,
            copy.created_at,
            copy.updated_at
        ],
    )
    .map(|_| ())
    .map_err(db_error)
}

fn select_copy(
    conn: &Connection,
    copy_id: &str,
) -> Result<Option<PurchaseDownloadCopy>, LibraryError> {
    conn.query_row(
        "SELECT copy_id, album_id, format_id, resolved_album_folder, created_at, updated_at
         FROM purchase_download_copies WHERE copy_id = ?1",
        [copy_id],
        copy_from_row,
    )
    .optional()
    .map_err(db_error)
}

fn copy_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<PurchaseDownloadCopy> {
    Ok(PurchaseDownloadCopy {
        copy_id: row.get(0)?,
        album_id: row.get(1)?,
        format_id: row.get(2)?,
        resolved_album_folder: row.get(3)?,
        created_at: row.get(4)?,
        updated_at: row.get(5)?,
    })
}

fn select_copy_tracks(
    conn: &Connection,
    copy_id: &str,
) -> Result<Vec<PurchaseDownloadCopyTrack>, LibraryError> {
    let mut stmt = conn
        .prepare(
            "SELECT copy_id, track_id, file_path, mime_type, container, sample_rate_hz,
                    bit_depth, channels, file_size, modified_ns, last_verified_at
             FROM purchase_download_copy_tracks WHERE copy_id = ?1 ORDER BY track_id",
        )
        .map_err(db_error)?;
    let rows = stmt
        .query_map([copy_id], |row| {
            let modified_ns = row
                .get::<_, Option<String>>(9)?
                .and_then(|value| value.parse::<u128>().ok());
            Ok(PurchaseDownloadCopyTrack {
                copy_id: row.get(0)?,
                track_id: row.get(1)?,
                file_path: row.get(2)?,
                mime_type: row.get(3)?,
                container: row.get(4)?,
                sample_rate_hz: row.get(5)?,
                bit_depth: row.get(6)?,
                channels: row.get(7)?,
                file_size: row.get::<_, i64>(8)?.max(0) as u64,
                modified_ns,
                last_verified_at: row.get(10)?,
            })
        })
        .map_err(db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_error)?;
    Ok(rows)
}

fn normalize_expected_track_ids(expected_track_ids: &[i64]) -> Result<Vec<i64>, LibraryError> {
    let expected: HashSet<i64> = expected_track_ids
        .iter()
        .copied()
        .filter(|track_id| *track_id > 0)
        .collect();
    if expected.is_empty() {
        return Err(LibraryError::Database(
            "purchase copy requires at least one expected track".to_string(),
        ));
    }
    let mut expected: Vec<i64> = expected.into_iter().collect();
    expected.sort_unstable();
    Ok(expected)
}

fn insert_expected_tracks(
    conn: &Connection,
    copy_id: &str,
    expected_track_ids: &[i64],
) -> Result<(), LibraryError> {
    for track_id in expected_track_ids {
        conn.execute(
            "INSERT INTO purchase_download_copy_expected_tracks (copy_id, track_id)
             VALUES (?1, ?2)",
            params![copy_id, track_id],
        )
        .map_err(db_error)?;
    }
    Ok(())
}

fn select_expected_tracks(conn: &Connection, copy_id: &str) -> Result<Vec<i64>, LibraryError> {
    let mut stmt = conn
        .prepare(
            "SELECT track_id FROM purchase_download_copy_expected_tracks
             WHERE copy_id = ?1 ORDER BY track_id",
        )
        .map_err(db_error)?;
    let rows = stmt
        .query_map([copy_id], |row| row.get(0))
        .map_err(db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_error)?;
    Ok(rows)
}

fn track_health(track: &PurchaseDownloadCopyTrack) -> PurchaseTrackHealth {
    match crate::probe_file_default(Path::new(&track.file_path)) {
        FileProbe::Missing => PurchaseTrackHealth::Missing,
        FileProbe::Unreachable => PurchaseTrackHealth::Unreachable,
        FileProbe::Present(metadata) if !metadata.is_file() => PurchaseTrackHealth::Unreadable,
        FileProbe::Present(metadata) => {
            if track.file_size > 0 && metadata.len() != track.file_size {
                return PurchaseTrackHealth::Changed;
            }
            if let Some(expected) = track.modified_ns {
                let observed = metadata
                    .modified()
                    .ok()
                    .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
                    .map(|value| value.as_nanos());
                if observed != Some(expected) {
                    return PurchaseTrackHealth::Changed;
                }
            }
            PurchaseTrackHealth::Healthy
        }
    }
}

fn mode_word(mode: PurchasePlaybackMode) -> &'static str {
    match mode {
        PurchasePlaybackMode::Qobuz => "qobuz",
        PurchasePlaybackMode::Purchase => "purchase",
    }
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .min(i64::MAX as u64) as i64
}

fn u64_to_i64(value: u64) -> i64 {
    value.min(i64::MAX as u64) as i64
}

fn db_error(error: rusqlite::Error) -> LibraryError {
    LibraryError::Database(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(copy_id: &str, id: i64, path: &Path) -> PurchaseDownloadCopyTrack {
        let metadata = std::fs::metadata(path).unwrap();
        PurchaseDownloadCopyTrack {
            copy_id: copy_id.to_string(),
            track_id: id,
            file_path: path.to_string_lossy().into_owned(),
            mime_type: Some("audio/flac".to_string()),
            container: Some("flac".to_string()),
            sample_rate_hz: Some(96_000),
            bit_depth: Some(24),
            channels: Some(2),
            file_size: metadata.len(),
            modified_ns: None,
            last_verified_at: None,
        }
    }

    #[test]
    fn copies_never_combine_coverage_across_folders() {
        let root = tempfile::tempdir().unwrap();
        let db = LibraryDatabase::open(&root.path().join("library.db")).unwrap();
        let first_dir = root.path().join("first");
        let second_dir = root.path().join("second");
        std::fs::create_dir_all(&first_dir).unwrap();
        std::fs::create_dir_all(&second_dir).unwrap();
        let first = db.create_purchase_copy("album", 7, &first_dir).unwrap();
        let second = db.create_purchase_copy("album", 7, &second_dir).unwrap();
        let one = first_dir.join("01.flac");
        let two = second_dir.join("02.flac");
        std::fs::write(&one, b"one").unwrap();
        std::fs::write(&two, b"two").unwrap();
        db.upsert_purchase_copy_track(&track(&first.copy_id, 1, &one))
            .unwrap();
        db.upsert_purchase_copy_track(&track(&second.copy_id, 2, &two))
            .unwrap();

        let snapshots = db.inspect_purchase_copies("album", &[1, 2]).unwrap();
        assert_eq!(snapshots.len(), 2);
        assert!(snapshots
            .iter()
            .all(|copy| copy.health == PurchaseCopyHealth::Partial));
        assert!(!db
            .promote_complete_purchase_copy(&first.copy_id, &[1, 2])
            .unwrap());
    }

    #[test]
    fn a_complete_copy_promotes_legacy_atomically() {
        let root = tempfile::tempdir().unwrap();
        let db = LibraryDatabase::open(&root.path().join("library.db")).unwrap();
        let copy = db.create_purchase_copy("album", 55, root.path()).unwrap();
        for id in [1, 2] {
            let path = root.path().join(format!("{id}.dsf"));
            std::fs::write(&path, b"dsd").unwrap();
            db.upsert_purchase_copy_track(&track(&copy.copy_id, id, &path))
                .unwrap();
        }

        assert!(db
            .promote_complete_purchase_copy(&copy.copy_id, &[1, 2])
            .unwrap());
        assert_eq!(
            db.get_downloaded_purchase_formats().unwrap(),
            vec![(1, 55), (2, 55)]
        );
    }

    #[test]
    fn preference_defaults_to_qobuz_and_round_trips_exact_format() {
        let root = tempfile::tempdir().unwrap();
        let db = LibraryDatabase::open(&root.path().join("library.db")).unwrap();
        let default = db.purchase_playback_preference("album").unwrap();
        assert_eq!(default.mode, PurchasePlaybackMode::Qobuz);
        assert_eq!(default.format_id, None);

        db.set_purchase_playback_preference("album", PurchasePlaybackMode::Purchase, Some(55))
            .unwrap();
        let stored = db.purchase_playback_preference("album").unwrap();
        assert_eq!(stored.mode, PurchasePlaybackMode::Purchase);
        assert_eq!(stored.format_id, Some(55));
    }

    #[test]
    fn playback_candidates_require_the_persisted_complete_manifest() {
        let root = tempfile::tempdir().unwrap();
        let db = LibraryDatabase::open(&root.path().join("library.db")).unwrap();

        let complete_dir = root.path().join("complete");
        let partial_dir = root.path().join("partial");
        let legacy_dir = root.path().join("legacy");
        std::fs::create_dir_all(&complete_dir).unwrap();
        std::fs::create_dir_all(&partial_dir).unwrap();
        std::fs::create_dir_all(&legacy_dir).unwrap();

        let complete = db
            .create_purchase_copy_with_expected_tracks("album", 55, &complete_dir, &[1, 2])
            .unwrap();
        for id in [1, 2] {
            let path = complete_dir.join(format!("complete-{id}.dsf"));
            std::fs::write(&path, b"dsd").unwrap();
            db.upsert_purchase_copy_track(&track(&complete.copy_id, id, &path))
                .unwrap();
        }

        let partial = db
            .create_purchase_copy_with_expected_tracks("album", 55, &partial_dir, &[1, 2])
            .unwrap();
        let partial_path = partial_dir.join("partial-1.dsf");
        std::fs::write(&partial_path, b"dsd").unwrap();
        db.upsert_purchase_copy_track(&track(&partial.copy_id, 1, &partial_path))
            .unwrap();

        let legacy = db.create_purchase_copy("album", 55, &legacy_dir).unwrap();
        for id in [1, 2] {
            let path = legacy_dir.join(format!("legacy-{id}.dsf"));
            std::fs::write(&path, b"dsd").unwrap();
            db.upsert_purchase_copy_track(&track(&legacy.copy_id, id, &path))
                .unwrap();
        }

        let candidates = db
            .complete_healthy_purchase_track_candidates("album", 55, 1)
            .unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].0.copy_id, complete.copy_id);
        assert!(db
            .complete_healthy_purchase_track_candidates("album", 56, 1)
            .unwrap()
            .is_empty());

        std::fs::remove_file(complete_dir.join("complete-2.dsf")).unwrap();
        assert!(db
            .complete_healthy_purchase_track_candidates("album", 55, 1)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn legacy_migration_groups_by_folder() {
        let root = tempfile::tempdir().unwrap();
        let db_path = root.path().join("library.db");
        {
            let db = LibraryDatabase::open(&db_path).unwrap();
            db.with_connection(|conn| {
                conn.execute(
                    "DELETE FROM library_kv WHERE key = ?1",
                    [LEGACY_MIGRATION_KEY],
                )
                .unwrap();
            });
            db.mark_purchase_downloaded(1, Some("album"), "/a/01.flac", 7)
                .unwrap();
            db.mark_purchase_downloaded(2, Some("album"), "/b/02.flac", 7)
                .unwrap();
        }
        let db = LibraryDatabase::open(&db_path).unwrap();
        let copies = db.get_purchase_copies_for_album("album").unwrap();
        assert_eq!(copies.len(), 2);
        assert_ne!(
            copies[0].resolved_album_folder,
            copies[1].resolved_album_folder
        );
    }

    #[test]
    fn concurrent_legacy_migrations_import_one_physical_copy() {
        let root = tempfile::tempdir().unwrap();
        let db_path = root.path().join("library.db");
        {
            let db = LibraryDatabase::open(&db_path).unwrap();
            db.mark_purchase_downloaded(1, Some("album"), "/a/01.flac", 7)
                .unwrap();
            db.with_connection(|conn| {
                conn.execute("DELETE FROM purchase_download_copy_tracks", [])
                    .unwrap();
                conn.execute("DELETE FROM purchase_download_copies", [])
                    .unwrap();
                conn.execute(
                    "DELETE FROM library_kv WHERE key = ?1",
                    [LEGACY_MIGRATION_KEY],
                )
                .unwrap();
            });
        }

        let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
        let mut workers = Vec::new();
        for _ in 0..2 {
            let path = db_path.clone();
            let barrier = barrier.clone();
            workers.push(std::thread::spawn(move || {
                let conn = Connection::open(path).unwrap();
                conn.busy_timeout(std::time::Duration::from_secs(2))
                    .unwrap();
                barrier.wait();
                migrate_legacy_registry(&conn).unwrap();
            }));
        }
        barrier.wait();
        for worker in workers {
            worker.join().unwrap();
        }

        let db = LibraryDatabase::open(&db_path).unwrap();
        assert_eq!(db.get_purchase_copies_for_album("album").unwrap().len(), 1);
    }

    #[test]
    fn reopening_repairs_duplicate_physical_identities_without_losing_coverage() {
        let root = tempfile::tempdir().unwrap();
        let db_path = root.path().join("library.db");
        let folder = root.path().join("same-copy");
        std::fs::create_dir_all(&folder).unwrap();
        let first_path = folder.join("01.flac");
        let second_path = folder.join("02.flac");
        std::fs::write(&first_path, b"one").unwrap();
        std::fs::write(&second_path, b"two").unwrap();
        {
            let db = LibraryDatabase::open(&db_path).unwrap();
            db.with_connection(|conn| {
                conn.execute("DROP INDEX idx_purchase_copies_identity", [])
                    .unwrap();
                for (copy_id, updated_at) in [("old", 10_i64), ("new", 20_i64)] {
                    conn.execute(
                        "INSERT INTO purchase_download_copies
                         (copy_id, album_id, format_id, resolved_album_folder, created_at, updated_at)
                         VALUES (?1, 'album', 7, ?2, 1, ?3)",
                        params![copy_id, folder.to_string_lossy(), updated_at],
                    )
                    .unwrap();
                }
                for (copy_id, track_id, path) in [
                    ("old", 1_i64, &first_path),
                    ("new", 2_i64, &second_path),
                ] {
                    conn.execute(
                        "INSERT INTO purchase_download_copy_tracks
                         (copy_id, track_id, file_path, file_size)
                         VALUES (?1, ?2, ?3, 3)",
                        params![copy_id, track_id, path.to_string_lossy()],
                    )
                    .unwrap();
                    conn.execute(
                        "INSERT INTO purchase_download_copy_expected_tracks(copy_id, track_id)
                         VALUES (?1, ?2)",
                        params![copy_id, track_id],
                    )
                    .unwrap();
                }
            });
        }

        let db = LibraryDatabase::open(&db_path).unwrap();
        let copies = db.get_purchase_copies_for_album("album").unwrap();
        assert_eq!(copies.len(), 1);
        assert_eq!(copies[0].copy_id, "new", "the newest row is the keeper");
        assert_eq!(db.get_purchase_copy_tracks("new").unwrap().len(), 2);
        assert_eq!(
            db.get_purchase_copy_expected_track_ids("new").unwrap(),
            vec![1, 2]
        );
        assert!(db.create_purchase_copy("album", 7, &folder).is_err());
    }

    #[test]
    fn forgetting_a_copy_removes_only_its_registry_and_leaves_files() {
        let root = tempfile::tempdir().unwrap();
        let db = LibraryDatabase::open(&root.path().join("library.db")).unwrap();
        let folder = root.path().join("copy");
        std::fs::create_dir_all(&folder).unwrap();
        let path = folder.join("01.flac");
        std::fs::write(&path, b"one").unwrap();
        let copy = db
            .create_purchase_copy_with_expected_tracks("album", 7, &folder, &[1])
            .unwrap();
        db.upsert_purchase_copy_track(&track(&copy.copy_id, 1, &path))
            .unwrap();
        assert!(db
            .promote_complete_purchase_copy(&copy.copy_id, &[1])
            .unwrap());

        assert!(db.delete_purchase_copy_record(&copy.copy_id).unwrap());
        assert!(path.exists(), "forgetting never deletes the user's file");
        assert!(db.get_purchase_copy(&copy.copy_id).unwrap().is_none());
        assert!(db.get_downloaded_purchase_formats().unwrap().is_empty());
    }
}
