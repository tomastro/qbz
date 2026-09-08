//! Profile-scoped metadata sidecars for media-server albums.
//!
//! Physical Local Library albums keep their `.qbz.json` beside the audio
//! files. Plex/Jellyfin/Subsonic albums have no writable local directory, so
//! their equivalent document lives in one small SQLite sidecar database in
//! the active user's data directory. The payload is deliberately the SAME
//! [`AlbumTagSidecar`] used by physical albums: the editor, validation and
//! future schema migrations therefore have one document rather than a remote
//! approximation of it.

use std::path::Path;
use std::time::Duration;

use rusqlite::{params, Connection, OptionalExtension};

use crate::{AlbumTagSidecar, LibraryError};

/// Stable identity of one remote album. `source_instance` is the server's own
/// id (or `default` for old cache rows that predate it); `album_id` and every
/// track `file_path` inside the payload are source-native opaque ids.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RemoteTagTarget {
    pub source: String,
    pub source_instance: String,
    pub album_id: String,
}

impl RemoteTagTarget {
    pub fn new(
        source: impl Into<String>,
        source_instance: impl Into<String>,
        album_id: impl Into<String>,
    ) -> Self {
        let source_instance = source_instance.into();
        Self {
            source: source.into(),
            source_instance: if source_instance.trim().is_empty() {
                "default".to_string()
            } else {
                source_instance
            },
            album_id: album_id.into(),
        }
    }
}

/// One decoded row used by the derived catalog's bounded bootstrap overlay.
#[derive(Debug, Clone)]
pub struct StoredRemoteTagSidecar {
    pub target: RemoteTagTarget,
    pub sidecar: AlbumTagSidecar,
}

pub struct RemoteTagSidecarStore {
    conn: Connection,
}

impl RemoteTagSidecarStore {
    pub fn open(path: &Path) -> Result<Self, LibraryError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(LibraryError::Io)?;
        }
        let conn =
            Connection::open(path).map_err(|error| LibraryError::Database(error.to_string()))?;
        conn.busy_timeout(Duration::from_secs(5))
            .map_err(|error| LibraryError::Database(error.to_string()))?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;
             CREATE TABLE IF NOT EXISTS remote_album_tag_sidecars (
                 source          TEXT NOT NULL,
                 source_instance TEXT NOT NULL,
                 album_id        TEXT NOT NULL,
                 payload_json    TEXT NOT NULL,
                 updated_at      INTEGER NOT NULL,
                 PRIMARY KEY(source, source_instance, album_id)
             );
             CREATE INDEX IF NOT EXISTS idx_remote_tag_sidecars_updated
                 ON remote_album_tag_sidecars(source, source_instance, updated_at);",
        )
        .map_err(|error| LibraryError::Database(error.to_string()))?;
        Ok(Self { conn })
    }

    pub fn get(&self, target: &RemoteTagTarget) -> Result<Option<AlbumTagSidecar>, LibraryError> {
        let json = self
            .conn
            .query_row(
                "SELECT payload_json FROM remote_album_tag_sidecars
                  WHERE source=?1 AND source_instance=?2 AND album_id=?3",
                params![target.source, target.source_instance, target.album_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|error| LibraryError::Database(error.to_string()))?;
        json.map(|json| {
            serde_json::from_str(&json).map_err(|error| LibraryError::Metadata(error.to_string()))
        })
        .transpose()
    }

    /// Replace one complete album document atomically. A full payload is
    /// intentional: track navigation edits one row in a validated snapshot,
    /// then persists that snapshot just like the album editor does.
    pub fn put(
        &mut self,
        target: &RemoteTagTarget,
        sidecar: &AlbumTagSidecar,
    ) -> Result<(), LibraryError> {
        let json = serde_json::to_string(sidecar)
            .map_err(|error| LibraryError::Metadata(error.to_string()))?;
        let tx = self
            .conn
            .transaction()
            .map_err(|error| LibraryError::Database(error.to_string()))?;
        tx.execute(
            "INSERT INTO remote_album_tag_sidecars(
                 source,source_instance,album_id,payload_json,updated_at
             ) VALUES (?1,?2,?3,?4,?5)
             ON CONFLICT(source,source_instance,album_id) DO UPDATE SET
                 payload_json=excluded.payload_json,
                 updated_at=excluded.updated_at",
            params![
                target.source,
                target.source_instance,
                target.album_id,
                json,
                sidecar.updated_at
            ],
        )
        .map_err(|error| LibraryError::Database(error.to_string()))?;
        tx.commit()
            .map_err(|error| LibraryError::Database(error.to_string()))
    }

    pub fn all(&self) -> Result<Vec<StoredRemoteTagSidecar>, LibraryError> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT source,source_instance,album_id,payload_json
                   FROM remote_album_tag_sidecars",
            )
            .map_err(|error| LibraryError::Database(error.to_string()))?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })
            .map_err(|error| LibraryError::Database(error.to_string()))?;
        let mut out = Vec::new();
        for row in rows {
            let (source, source_instance, album_id, json) =
                row.map_err(|error| LibraryError::Database(error.to_string()))?;
            let sidecar = match serde_json::from_str(&json) {
                Ok(sidecar) => sidecar,
                Err(error) => {
                    // One damaged optional overlay must not disable metadata
                    // edits for every other server album in the profile. The
                    // authoritative media caches remain readable; skip only
                    // the bad row and leave enough identity in the log to
                    // repair/remove it later.
                    log::warn!(
                        "[remote-tag-sidecar] ignoring corrupt row source={} instance={} album={}: {}",
                        source,
                        source_instance,
                        album_id,
                        error
                    );
                    continue;
                }
            };
            out.push(StoredRemoteTagSidecar {
                target: RemoteTagTarget::new(source, source_instance, album_id),
                sidecar,
            });
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AlbumMetadataOverride, TrackMetadataOverride};

    #[test]
    fn profile_sidecar_roundtrip_keeps_native_track_identity_and_clears() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("metadata_sidecars.db");
        let target = RemoteTagTarget::new("subsonic", "navidrome-a", "album-9");
        let sidecar = AlbumTagSidecar::new(
            AlbumMetadataOverride {
                album_title: Some("Edited album".into()),
                album_artist: Some(String::new()),
                year: Some(0),
                genre: Some("Progressive rock".into()),
                catalog_number: Some("CAT-1".into()),
            },
            vec![TrackMetadataOverride {
                file_path: "native-track-4".into(),
                cue_start_secs: None,
                title: Some("Edited track".into()),
                disc_number: Some(0),
                track_number: Some(7),
            }],
        );

        let mut store = RemoteTagSidecarStore::open(&path).unwrap();
        store.put(&target, &sidecar).unwrap();
        let read = store.get(&target).unwrap().unwrap();
        assert_eq!(read.album.album_title.as_deref(), Some("Edited album"));
        assert_eq!(read.album.year, Some(0));
        assert_eq!(read.tracks[0].file_path, "native-track-4");
        assert_eq!(read.tracks[0].disc_number, Some(0));

        let all = store.all().unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].target, target);
    }

    #[test]
    fn old_cache_rows_use_a_stable_default_server_identity() {
        assert_eq!(
            RemoteTagTarget::new("plex", "", "album").source_instance,
            "default"
        );
    }

    #[test]
    fn one_corrupt_payload_does_not_hide_other_sidecars() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("metadata_sidecars.db");
        let mut store = RemoteTagSidecarStore::open(&path).unwrap();
        let target = RemoteTagTarget::new("plex", "server", "good");
        store
            .put(
                &target,
                &AlbumTagSidecar::new(AlbumMetadataOverride::default(), Vec::new()),
            )
            .unwrap();
        store
            .conn
            .execute(
                "INSERT INTO remote_album_tag_sidecars VALUES
                 ('plex','server','broken','{',1)",
                [],
            )
            .unwrap();

        let rows = store.all().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].target, target);
    }
}
