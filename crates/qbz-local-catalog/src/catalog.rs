use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::{Duration, Instant};

use rusqlite::types::Value;
use rusqlite::{
    params, params_from_iter, Connection, OpenFlags, OptionalExtension, Row,
    Statement,
};

use crate::bootstrap::{BootstrapBatch, SourceCheckpoint, BOOTSTRAP_BATCH_ROWS};
use crate::model::{
    AlbumCursor, AlbumPage, AlbumRecord, ArtistCursor, ArtistPage, ArtistRecord, ProjectedTrack,
    QueryDescriptor, QuerySurface, SourceKey, SourceKind, TrackCursor, TrackGroup, TrackPage,
    TrackRecord, TrackRef, TrackSort,
};
use crate::projection::ReconciliationBatch;
use crate::schema;
use crate::{CatalogError, Result, SCHEMA_VERSION};

const MAX_PAGE_SIZE: usize = 500;
const ALBUM_ARTIST_FAMILY_SEPARATOR: &str = " • ";

#[derive(Default)]
struct AlbumArtistFamilyEvidence {
    display_counts: HashMap<String, u64>,
    suffixes: HashSet<String>,
}

pub struct Catalog {
    conn: Connection,
    generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogStats {
    pub schema_version: u32,
    pub generation: u64,
    pub track_count: u64,
    pub source_counts: Vec<(SourceKey, u64)>,
    pub page_size_bytes: u64,
    pub page_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntegrityReport {
    pub sqlite_ok: bool,
    pub foreign_key_violations: u64,
    pub fts_ok: bool,
}

/// Checks that are safe against the read-only active generation. Full FTS5
/// integrity is verified before activation; SQLite exposes that check through
/// a virtual-table write command, so diagnostics must not run it against the
/// live catalog merely to display health.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeIntegrityReport {
    pub sqlite_ok: bool,
    pub sqlite_result: String,
    pub foreign_key_violations: u64,
    pub materialized_views_ok: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QueryMetrics {
    pub sql_time: Duration,
    pub rows: usize,
}

struct RowWithCursor {
    record: TrackRecord,
    cursor: TrackCursor,
}

impl Catalog {
    pub fn open(path: &Path, generation: u64) -> Result<Self> {
        let mut conn = Connection::open(path)?;
        schema::init(&mut conn, generation)?;
        Ok(Self { conn, generation })
    }

    pub fn open_in_memory(generation: u64) -> Result<Self> {
        let mut conn = Connection::open_in_memory()?;
        schema::init(&mut conn, generation)?;
        Ok(Self { conn, generation })
    }

    pub fn open_read_only(path: &Path, generation: u64) -> Result<Self> {
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        schema::configure_read_only(&conn)?;
        schema::verify(&conn, generation)?;
        Ok(Self { conn, generation })
    }

    pub(crate) fn backup_to(&self, path: &Path) -> Result<()> {
        self.conn.backup(rusqlite::MAIN_DB, path, None)?;
        Ok(())
    }

    pub(crate) fn adopt_generation(
        path: &Path,
        previous_generation: u64,
        generation: u64,
    ) -> Result<Self> {
        let conn = Connection::open(path)?;
        schema::configure(&conn)?;
        schema::verify(&conn, previous_generation)?;
        conn.execute(
            "UPDATE catalog_meta SET value=?1 WHERE key='generation'",
            [generation.to_string()],
        )?;
        let mut catalog = Self { conn, generation };
        catalog.set_build_phase("projection", previous_generation)?;
        Ok(catalog)
    }

    pub(crate) fn set_build_phase(&mut self, phase: &str, base_generation: u64) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO catalog_meta(key,value) VALUES ('build_phase',?1)
             ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            [phase],
        )?;
        tx.execute(
            "INSERT INTO catalog_meta(key,value) VALUES ('base_generation',?1)
             ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            [base_generation.to_string()],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub(crate) fn build_phase(&self) -> Result<(String, u64)> {
        let phase = self
            .conn
            .query_row(
                "SELECT value FROM catalog_meta WHERE key='build_phase'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .unwrap_or_default();
        let base = self
            .conn
            .query_row(
                "SELECT value FROM catalog_meta WHERE key='base_generation'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .and_then(|value| value.parse().ok())
            .unwrap_or(0);
        Ok((phase, base))
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Idempotent batch projection. Source-native identity owns the upsert;
    /// the internal catalog rowid is retained and never becomes product state.
    pub fn upsert_tracks(&mut self, tracks: &[ProjectedTrack]) -> Result<usize> {
        let tx = self.conn.transaction()?;
        {
            let mut upsert = tx.prepare_cached(UPSERT_TRACK_SQL)?;
            let mut clear_credits =
                tx.prepare_cached("DELETE FROM artist_credits WHERE catalog_id = ?1")?;
            let mut insert_credit = tx.prepare_cached(
                "INSERT OR IGNORE INTO artist_credits(
                     catalog_id, artist_key, display_name, role, ordinal
                 ) VALUES (?1,?2,?3,?4,?5)",
            )?;
            for track in tracks {
                validate_track(track)?;
                upsert_track(&mut upsert, &mut clear_credits, &mut insert_credit, track)?;
            }
        }
        tx.commit()?;
        Ok(tracks.len())
    }

    pub fn remove_track(&mut self, track_ref: &TrackRef) -> Result<bool> {
        validate_ref(track_ref)?;
        Ok(self.conn.execute(
            "DELETE FROM tracks
              WHERE source_kind = ?1 AND source_instance = ?2 AND native_track_id = ?3",
            params![
                track_ref.source.as_str(),
                track_ref.source_instance,
                track_ref.native_id
            ],
        )? > 0)
    }

    pub(crate) fn source_checkpoint(&self, source: &SourceKey) -> Result<Option<SourceCheckpoint>> {
        self.conn
            .query_row(
                "SELECT available, checkpoint_cursor, checkpoint_rows,
                        checkpoint_version, complete_generation
                   FROM source_state
                  WHERE source_kind = ?1 AND source_instance = ?2",
                params![source.source.as_str(), source.source_instance],
                |row| {
                    Ok(SourceCheckpoint {
                        source: source.clone(),
                        available: row.get::<_, i64>(0)? != 0,
                        checkpoint_cursor: row.get(1)?,
                        checkpoint_rows: row.get::<_, i64>(2)?.max(0) as u64,
                        checkpoint_version: row.get(3)?,
                        complete_generation: row.get::<_, i64>(4)?.max(0) as u64,
                        complete: row.get::<_, i64>(4)? == self.generation as i64,
                    })
                },
            )
            .optional()
            .map_err(Into::into)
    }

    pub(crate) fn restart_projection_source(
        &mut self,
        source: &SourceKey,
        snapshot_version: &str,
    ) -> Result<()> {
        if source.source_instance.trim().is_empty() || snapshot_version.trim().is_empty() {
            return Err(CatalogError::InvalidInput(
                "source instance and snapshot version must not be empty".to_string(),
            ));
        }
        let tx = self.conn.transaction()?;
        tx.execute(
            "DELETE FROM tracks WHERE source_kind = ?1 AND source_instance = ?2",
            params![source.source.as_str(), source.source_instance],
        )?;
        tx.execute(
            "DELETE FROM source_copies WHERE source_kind = ?1 AND source_instance = ?2",
            params![source.source.as_str(), source.source_instance],
        )?;
        tx.execute(
            "DELETE FROM editions
              WHERE NOT EXISTS (
                    SELECT 1 FROM source_copies sc WHERE sc.edition_id=editions.edition_id
              )",
            [],
        )?;
        tx.execute(
            "DELETE FROM logical_albums
              WHERE NOT EXISTS (
                    SELECT 1 FROM editions e
                     WHERE e.logical_album_id=logical_albums.logical_album_id
              )",
            [],
        )?;
        tx.execute(
            "INSERT INTO source_state(
                 source_kind, source_instance, available, last_observed_at,
                 watermark, complete_generation, checkpoint_cursor,
                 checkpoint_rows, checkpoint_version
             ) VALUES (?1,?2,1,0,'',0,'',0,?3)
             ON CONFLICT(source_kind, source_instance) DO UPDATE SET
                 available=1,
                 last_observed_at=0,
                 watermark='',
                 complete_generation=0,
                 checkpoint_cursor='',
                 checkpoint_rows=0,
                 checkpoint_version=excluded.checkpoint_version",
            params![
                source.source.as_str(),
                source.source_instance,
                snapshot_version
            ],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub(crate) fn apply_bootstrap_batch(
        &mut self,
        batch: &BootstrapBatch,
    ) -> Result<SourceCheckpoint> {
        if batch.tracks.len() > BOOTSTRAP_BATCH_ROWS {
            return Err(CatalogError::BatchTooLarge {
                found: batch.tracks.len(),
                maximum: BOOTSTRAP_BATCH_ROWS,
            });
        }
        if batch.source.source_instance.trim().is_empty()
            || batch.snapshot_version.trim().is_empty()
        {
            return Err(CatalogError::InvalidInput(
                "source instance and snapshot version must not be empty".to_string(),
            ));
        }
        for track in &batch.tracks {
            if track.track_ref.source != batch.source.source
                || track.track_ref.source_instance != batch.source.source_instance
            {
                return Err(CatalogError::InvalidInput(format!(
                    "batch {:?} contains a row from {:?}/{}",
                    batch.source, track.track_ref.source, track.track_ref.source_instance
                )));
            }
        }

        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO source_state(
                 source_kind, source_instance, checkpoint_version
             ) VALUES (?1,?2,?3)
             ON CONFLICT(source_kind, source_instance) DO NOTHING",
            params![
                batch.source.source.as_str(),
                batch.source.source_instance,
                batch.snapshot_version
            ],
        )?;
        let (committed_cursor, committed_rows, committed_version): (String, i64, String) = tx
            .query_row(
                "SELECT checkpoint_cursor, checkpoint_rows, checkpoint_version
                   FROM source_state
                  WHERE source_kind = ?1 AND source_instance = ?2",
                params![batch.source.source.as_str(), batch.source.source_instance],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )?;
        if committed_version != batch.snapshot_version {
            return Err(CatalogError::SourceSnapshotChanged(batch.source.clone()));
        }
        if committed_cursor != batch.expected_cursor {
            return Err(CatalogError::CheckpointMismatch(batch.source.clone()));
        }

        {
            let mut upsert = tx.prepare_cached(UPSERT_TRACK_SQL)?;
            let mut clear_credits =
                tx.prepare_cached("DELETE FROM artist_credits WHERE catalog_id = ?1")?;
            let mut insert_credit = tx.prepare_cached(
                "INSERT OR IGNORE INTO artist_credits(
                     catalog_id, artist_key, display_name, role, ordinal
                 ) VALUES (?1,?2,?3,?4,?5)",
            )?;
            for track in &batch.tracks {
                let mut track = track.clone();
                track.observed_generation = self.generation as i64;
                track.source_copy_id = Some(ensure_source_copy(&tx, &track, self.generation)?);
                validate_track(&track)?;
                upsert_track(&mut upsert, &mut clear_credits, &mut insert_credit, &track)?;
            }
        }
        let next_rows = committed_rows.saturating_add(batch.tracks.len() as i64);
        tx.execute(
            "UPDATE source_state
                SET available=1,
                    last_observed_at=?3,
                    complete_generation=?4,
                    checkpoint_cursor=?5,
                    checkpoint_rows=?6
              WHERE source_kind=?1 AND source_instance=?2",
            params![
                batch.source.source.as_str(),
                batch.source.source_instance,
                now_unix_seconds(),
                if batch.complete {
                    self.generation as i64
                } else {
                    0
                },
                batch.next_cursor,
                next_rows,
            ],
        )?;
        tx.commit()?;
        Ok(SourceCheckpoint {
            source: batch.source.clone(),
            available: true,
            checkpoint_cursor: batch.next_cursor.clone(),
            checkpoint_rows: next_rows.max(0) as u64,
            checkpoint_version: batch.snapshot_version.clone(),
            complete_generation: if batch.complete { self.generation } else { 0 },
            complete: batch.complete,
        })
    }

    pub(crate) fn begin_reconciliation(
        &mut self,
        source: &SourceKey,
        snapshot_version: &str,
    ) -> Result<()> {
        if source.source_instance.trim().is_empty() || snapshot_version.trim().is_empty() {
            return Err(CatalogError::InvalidInput(
                "source instance and snapshot version must not be empty".to_string(),
            ));
        }
        let tx = self.conn.transaction()?;
        // A restarted attempt in the same derived generation must not inherit
        // observation marks from a superseded snapshot.
        tx.execute(
            "UPDATE tracks SET last_observed_generation=0
              WHERE source_kind=?1 AND source_instance=?2",
            params![source.source.as_str(), source.source_instance],
        )?;
        tx.execute(
            "INSERT INTO source_state(
                 source_kind,source_instance,available,last_observed_at,watermark,
                 complete_generation,checkpoint_cursor,checkpoint_rows,checkpoint_version
             ) VALUES (?1,?2,1,0,'',0,'',0,?3)
             ON CONFLICT(source_kind,source_instance) DO UPDATE SET
                 available=1,
                 complete_generation=0,
                 checkpoint_cursor='',
                 checkpoint_rows=0,
                 checkpoint_version=excluded.checkpoint_version",
            params![
                source.source.as_str(),
                source.source_instance,
                snapshot_version
            ],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub(crate) fn apply_reconciliation_batch(
        &mut self,
        batch: &ReconciliationBatch,
    ) -> Result<SourceCheckpoint> {
        if batch.tracks.len() > BOOTSTRAP_BATCH_ROWS {
            return Err(CatalogError::BatchTooLarge {
                found: batch.tracks.len(),
                maximum: BOOTSTRAP_BATCH_ROWS,
            });
        }
        for track in &batch.tracks {
            if track.track_ref.source != batch.source.source
                || track.track_ref.source_instance != batch.source.source_instance
            {
                return Err(CatalogError::InvalidInput(format!(
                    "reconciliation {:?} contains a row from {:?}/{}",
                    batch.source, track.track_ref.source, track.track_ref.source_instance
                )));
            }
        }
        let tx = self.conn.transaction()?;
        let (committed_cursor, committed_rows, committed_version): (String, i64, String) = tx
            .query_row(
                "SELECT checkpoint_cursor,checkpoint_rows,checkpoint_version
                   FROM source_state
                  WHERE source_kind=?1 AND source_instance=?2",
                params![batch.source.source.as_str(), batch.source.source_instance],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )?;
        if committed_version != batch.snapshot_version {
            return Err(CatalogError::SourceSnapshotChanged(batch.source.clone()));
        }
        if committed_cursor != batch.expected_cursor {
            return Err(CatalogError::CheckpointMismatch(batch.source.clone()));
        }
        {
            let mut upsert = tx.prepare_cached(UPSERT_TRACK_SQL)?;
            let mut clear_credits =
                tx.prepare_cached("DELETE FROM artist_credits WHERE catalog_id=?1")?;
            let mut insert_credit = tx.prepare_cached(
                "INSERT OR IGNORE INTO artist_credits(
                     catalog_id,artist_key,display_name,role,ordinal
                 ) VALUES (?1,?2,?3,?4,?5)",
            )?;
            for track in &batch.tracks {
                let mut track = track.clone();
                track.observed_generation = self.generation as i64;
                track.source_copy_id = Some(ensure_source_copy(&tx, &track, self.generation)?);
                validate_track(&track)?;
                upsert_track(&mut upsert, &mut clear_credits, &mut insert_credit, &track)?;
            }
        }
        let next_rows = committed_rows.saturating_add(batch.tracks.len() as i64);
        if batch.complete {
            tx.execute(
                "DELETE FROM tracks
                  WHERE source_kind=?1 AND source_instance=?2
                    AND last_observed_generation != ?3",
                params![
                    batch.source.source.as_str(),
                    batch.source.source_instance,
                    self.generation as i64
                ],
            )?;
            tx.execute(
                "DELETE FROM source_copies
                  WHERE source_kind=?1 AND source_instance=?2
                    AND NOT EXISTS (
                        SELECT 1 FROM tracks t
                         WHERE t.source_copy_id=source_copies.source_copy_id
                    )",
                params![batch.source.source.as_str(), batch.source.source_instance],
            )?;
            // Two statements, two calls: rusqlite < 0.32 silently ran only the
            // FIRST statement of a multi-statement `execute` (the logical_albums
            // prune below never happened); 0.40 refuses it outright.
            tx.execute_batch(
                "DELETE FROM editions
                  WHERE NOT EXISTS (
                        SELECT 1 FROM source_copies sc
                         WHERE sc.edition_id=editions.edition_id
                  );
                 DELETE FROM logical_albums
                  WHERE NOT EXISTS (
                        SELECT 1 FROM editions e
                         WHERE e.logical_album_id=logical_albums.logical_album_id
                  );",
            )?;
        }
        tx.execute(
            "UPDATE source_state
                SET available=1,last_observed_at=?3,
                    watermark=CASE WHEN ?4 THEN ?5 ELSE watermark END,
                    complete_generation=CASE WHEN ?4 THEN ?6 ELSE 0 END,
                    checkpoint_cursor=?7,checkpoint_rows=?8
              WHERE source_kind=?1 AND source_instance=?2",
            params![
                batch.source.source.as_str(),
                batch.source.source_instance,
                now_unix_seconds(),
                batch.complete,
                batch.snapshot_version,
                self.generation as i64,
                batch.next_cursor,
                next_rows,
            ],
        )?;
        tx.commit()?;
        Ok(SourceCheckpoint {
            source: batch.source.clone(),
            available: true,
            checkpoint_cursor: batch.next_cursor.clone(),
            checkpoint_rows: next_rows.max(0) as u64,
            checkpoint_version: batch.snapshot_version.clone(),
            complete_generation: if batch.complete { self.generation } else { 0 },
            complete: batch.complete,
        })
    }

    pub(crate) fn source_watermark(&self, source: &SourceKey) -> Result<Option<String>> {
        self.conn
            .query_row(
                "SELECT watermark FROM source_state
                  WHERE source_kind=?1 AND source_instance=?2",
                params![source.source.as_str(), source.source_instance],
                |row| row.get(0),
            )
            .optional()
            .map_err(Into::into)
    }

    pub(crate) fn checkpoint_for_activation(&mut self) -> Result<()> {
        self.conn
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
        Ok(())
    }

    pub(crate) fn rebuild_materialized_views(&mut self) -> Result<()> {
        let tx = self.conn.transaction()?;
        rebuild_artist_identities(&tx)?;
        tx.execute_batch(
            "DELETE FROM artists_fts;
             DELETE FROM artist_source_stats;
             DELETE FROM edition_artists;
             DELETE FROM albums_materialized;
             DELETE FROM artists_materialized;

             INSERT INTO edition_artists(edition_id,artist_key,role)
             SELECT DISTINCT sc.edition_id,ac.artist_key,ac.role
               FROM artist_identity_credits ac
               JOIN tracks t ON t.catalog_id=ac.catalog_id
               JOIN source_copies sc ON sc.source_copy_id=t.source_copy_id;

             INSERT INTO albums_materialized(
                 edition_id,logical_album_id,title,sort_title,artist,sort_artist,
                 year,year_missing,year_value,track_count,total_duration_ms,
                 source_count,available,
                 source_kind,native_album_id,source_raw,all_artists,format,
                 bit_depth,sample_rate_hz,quality_tier,directory_path,folder_count,
                 added_at,
                 artwork_source,artwork_token
             )
             SELECT e.edition_id,e.logical_album_id,e.display_title,la.sort_title,
                    e.display_artist,la.sort_artist,e.release_year,
                    CASE WHEN e.release_year IS NULL THEN 1 ELSE 0 END,
                    COALESCE(e.release_year,0),COUNT(t.catalog_id),
                    COALESCE(SUM(t.duration_ms),0),
                    COUNT(DISTINCT sc.source_kind || char(31) || sc.source_instance),
                    COALESCE(MAX(t.available),0),
                    COALESCE((
                        SELECT scp.source_kind FROM source_copies scp
                         WHERE scp.edition_id=e.edition_id
                         ORDER BY scp.available DESC,
                                  CASE scp.source_kind
                                      WHEN 'local' THEN 0 WHEN 'offline' THEN 1
                                      WHEN 'plex' THEN 2 WHEN 'jellyfin' THEN 3 ELSE 4 END,
                                  scp.source_copy_id
                         LIMIT 1
                    ),'local'),
                    COALESCE((
                        SELECT scp.native_album_id FROM source_copies scp
                         WHERE scp.edition_id=e.edition_id
                         ORDER BY scp.available DESC,
                                  CASE scp.source_kind
                                      WHEN 'local' THEN 0 WHEN 'offline' THEN 1
                                      WHEN 'plex' THEN 2 WHEN 'jellyfin' THEN 3 ELSE 4 END,
                                  scp.source_copy_id
                         LIMIT 1
                    ),''),
                    COALESCE((
                        SELECT t2.source_raw FROM tracks t2
                         JOIN source_copies sc2 ON sc2.source_copy_id=t2.source_copy_id
                         WHERE sc2.edition_id=e.edition_id
                         ORDER BY t2.available DESC,t2.catalog_id LIMIT 1
                    ),''),
                    COALESCE((
                        SELECT group_concat(DISTINCT ac.display_name)
                          FROM artist_identity_credits ac
                          JOIN tracks ta ON ta.catalog_id=ac.catalog_id
                          JOIN source_copies sca ON sca.source_copy_id=ta.source_copy_id
                         WHERE sca.edition_id=e.edition_id
                    ),''),
                    COALESCE((
                        SELECT LOWER(t2.format) FROM tracks t2
                         JOIN source_copies sc2 ON sc2.source_copy_id=t2.source_copy_id
                         WHERE sc2.edition_id=e.edition_id
                         ORDER BY COALESCE(t2.bit_depth,0) DESC,
                                  COALESCE(t2.sample_rate_hz,0) DESC,t2.catalog_id
                         LIMIT 1
                    ),''),
                    MAX(t.bit_depth),MAX(t.sample_rate_hz),
                    CASE
                        WHEN MAX(CASE WHEN LOWER(t.format)='mp3' THEN 0 ELSE 1 END)=0
                            THEN 'mp3'
                        WHEN MAX(CASE WHEN LOWER(t.format) IN ('dsd','dsf','dff') THEN 1 ELSE 0 END)=1
                            THEN 'dsd'
                        WHEN MAX(t.bit_depth)>=24 AND MAX(t.sample_rate_hz)>96000 THEN 'max'
                        WHEN MAX(t.bit_depth)>=24 THEN 'hires'
                        WHEN MAX(t.bit_depth) IS NOT NULL THEN 'cd'
                        WHEN MAX(t.sample_rate_hz)>=44100 THEN 'cd'
                        ELSE ''
                    END,
                    COALESCE(MIN(NULLIF(sc.local_directory,'')),''),
                    COUNT(DISTINCT NULLIF(sc.local_directory,'')),MAX(t.added_at),
                    COALESCE((
                        SELECT t2.source_kind FROM tracks t2
                         WHERE t2.source_copy_id IN (
                             SELECT sc2.source_copy_id FROM source_copies sc2
                              WHERE sc2.edition_id=e.edition_id
                         ) AND COALESCE(t2.artwork_token,'') != ''
                         ORDER BY t2.available DESC,t2.catalog_id LIMIT 1
                    ),''),
                    COALESCE((
                        SELECT t2.artwork_token FROM tracks t2
                         WHERE t2.source_copy_id IN (
                             SELECT sc2.source_copy_id FROM source_copies sc2
                              WHERE sc2.edition_id=e.edition_id
                         ) AND COALESCE(t2.artwork_token,'') != ''
                         ORDER BY t2.available DESC,t2.catalog_id LIMIT 1
                    ),'')
               FROM editions e
               JOIN logical_albums la ON la.logical_album_id=e.logical_album_id
               JOIN source_copies sc ON sc.edition_id=e.edition_id
               JOIN tracks t ON t.source_copy_id=sc.source_copy_id
              GROUP BY e.edition_id;

             INSERT INTO artists_materialized(
                 artist_key,display_name,sort_name,album_count,track_count,available,
                 source_kind,artwork_source,artwork_token
             )
             SELECT ac.artist_key,MIN(ac.display_name),ac.artist_key,
                    COUNT(DISTINCT CASE WHEN t.available=1 THEN sc.edition_id END),
                    COUNT(DISTINCT CASE WHEN t.available=1 THEN ac.catalog_id END),
                    COALESCE(MAX(t.available),0),
                    CASE WHEN COUNT(DISTINCT t.source_kind)>1 THEN 'mixed'
                         ELSE MIN(t.source_kind) END,
                    COALESCE((
                        SELECT t2.source_kind
                          FROM artist_identity_credits ac2
                          JOIN tracks t2 ON t2.catalog_id=ac2.catalog_id
                         WHERE ac2.artist_key=ac.artist_key
                           AND t2.available=1
                           AND COALESCE(t2.artwork_token,'') != ''
                         ORDER BY CASE ac2.role
                                      WHEN 'track_artist' THEN 0
                                      WHEN 'album_artist' THEN 1
                                      WHEN 'featured' THEN 2
                                      WHEN 'performer' THEN 3 ELSE 4 END,
                                  t2.catalog_id LIMIT 1
                    ),''),
                    COALESCE((
                        SELECT t2.artwork_token
                          FROM artist_identity_credits ac2
                          JOIN tracks t2 ON t2.catalog_id=ac2.catalog_id
                         WHERE ac2.artist_key=ac.artist_key
                           AND t2.available=1
                           AND COALESCE(t2.artwork_token,'') != ''
                         ORDER BY CASE ac2.role
                                      WHEN 'track_artist' THEN 0
                                      WHEN 'album_artist' THEN 1
                                      WHEN 'featured' THEN 2
                                      WHEN 'performer' THEN 3 ELSE 4 END,
                                  t2.catalog_id LIMIT 1
                    ),'')
               FROM artist_identity_credits ac
               JOIN tracks t ON t.catalog_id=ac.catalog_id
               LEFT JOIN source_copies sc ON sc.source_copy_id=t.source_copy_id
              GROUP BY ac.artist_key;

             INSERT INTO artist_source_stats(
                 artist_key,source_kind,source_instance,album_count,track_count,available
             )
             SELECT ac.artist_key,t.source_kind,t.source_instance,
                    COUNT(DISTINCT CASE WHEN t.available=1 THEN sc.edition_id END),
                    COUNT(DISTINCT CASE WHEN t.available=1 THEN ac.catalog_id END),
                    COALESCE(MAX(t.available),0)
               FROM artist_identity_credits ac
               JOIN tracks t ON t.catalog_id=ac.catalog_id
               LEFT JOIN source_copies sc ON sc.source_copy_id=t.source_copy_id
              GROUP BY ac.artist_key,t.source_kind,t.source_instance;

             INSERT INTO artists_fts(artist_key,display_name)
             SELECT artist_key,display_name FROM artists_materialized;",
        )?;
        tx.commit()?;
        Ok(())
    }

    pub(crate) fn materialized_views_valid(&self) -> Result<bool> {
        let (tracks, album_tracks, unresolved, credit_artists, materialized_artists, missing_stats):
            (i64, i64, i64, i64, i64, i64) = self.conn.query_row(
            "SELECT
                 (SELECT COUNT(*) FROM tracks),
                 (SELECT COALESCE(SUM(track_count),0) FROM albums_materialized),
                 (SELECT COUNT(*) FROM tracks WHERE source_copy_id IS NULL),
                 (SELECT COUNT(DISTINCT artist_key) FROM artist_identity_credits),
                 (SELECT COUNT(*) FROM artists_materialized),
                 (SELECT COUNT(*) FROM artists_materialized am
                   WHERE NOT EXISTS (SELECT 1 FROM artist_source_stats ast
                                      WHERE ast.artist_key=am.artist_key))",
            [],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            },
        )?;
        Ok(tracks == album_tracks
            && unresolved == 0
            && credit_artists == materialized_artists
            && missing_stats == 0)
    }

    pub fn resolve(&self, track_ref: &TrackRef) -> Result<Option<TrackRecord>> {
        validate_ref(track_ref)?;
        let sql = format!(
            "SELECT {TRACK_COLUMNS}
               FROM tracks t
              WHERE t.source_kind = ?1
                AND t.source_instance = ?2
                AND t.native_track_id = ?3"
        );
        self.conn
            .query_row(
                &sql,
                params![
                    track_ref.source.as_str(),
                    track_ref.source_instance,
                    track_ref.native_id
                ],
                map_row,
            )
            .optional()
            .map(|row| row.map(|value| value.record))
            .map_err(Into::into)
    }

    pub fn count_tracks(&self, descriptor: &QueryDescriptor) -> Result<u64> {
        validate_tracks_descriptor(descriptor)?;
        let (sql, values) = count_query(descriptor)?;
        let count: i64 = self
            .conn
            .query_row(&sql, params_from_iter(values), |row| row.get(0))?;
        Ok(count as u64)
    }

    pub fn query_tracks(
        &self,
        descriptor: &QueryDescriptor,
        cursor: Option<&TrackCursor>,
        page_size: usize,
    ) -> Result<TrackPage> {
        self.query_tracks_timed(descriptor, cursor, page_size)
            .map(|(page, _)| page)
    }

    pub fn query_tracks_timed(
        &self,
        descriptor: &QueryDescriptor,
        cursor: Option<&TrackCursor>,
        page_size: usize,
    ) -> Result<(TrackPage, QueryMetrics)> {
        validate_tracks_descriptor(descriptor)?;
        let limit = page_size.clamp(1, MAX_PAGE_SIZE);
        let (sql, values, sort) = page_query(descriptor, cursor, limit)?;
        let descriptor_key = descriptor_key(descriptor);
        let started = Instant::now();
        let mut stmt = self.conn.prepare_cached(&sql)?;
        let mapped = stmt.query_map(params_from_iter(values), map_row)?;
        let mut values = mapped.collect::<rusqlite::Result<Vec<_>>>()?;
        for value in &mut values {
            value.cursor.sort = sort;
            value.cursor.descriptor_key = descriptor_key.clone();
        }
        let sql_time = started.elapsed();
        let has_more = values.len() > limit;
        values.truncate(limit);
        let next_cursor = has_more.then(|| values.last().expect("non-empty page").cursor.clone());
        let rows = values
            .into_iter()
            .map(|value| value.record)
            .collect::<Vec<_>>();
        let metrics = QueryMetrics {
            sql_time,
            rows: rows.len(),
        };
        Ok((
            TrackPage {
                rows,
                next_cursor,
                has_more,
            },
            metrics,
        ))
    }

    pub fn count_albums(&self, descriptor: &QueryDescriptor) -> Result<u64> {
        validate_albums_descriptor(descriptor)?;
        let parts = album_filter_parts(descriptor, None)?;
        let sql = format!(
            "SELECT COUNT(*) {} WHERE {}",
            parts.from_sql, parts.where_sql
        );
        let count: i64 = self
            .conn
            .query_row(&sql, params_from_iter(parts.params), |row| row.get(0))?;
        Ok(count.max(0) as u64)
    }

    /// Exact number of visual rows for the current grid/list layout. Group
    /// headers count as one row and album chunks never cross a global group.
    pub fn count_album_entries(&self, descriptor: &QueryDescriptor, columns: usize) -> Result<u64> {
        validate_albums_descriptor(descriptor)?;
        let columns = columns.clamp(1, 32) as i64;
        let parts = album_filter_parts(descriptor, None)?;
        let sql = if descriptor.group() == TrackGroup::Off {
            format!(
                "SELECT (COUNT(*) + ? - 1) / ? {} WHERE {}",
                parts.from_sql, parts.where_sql
            )
        } else {
            let group = album_group_expression(descriptor.group());
            format!(
                "SELECT COALESCE(SUM(1 + (n + ? - 1) / ?),0)
                   FROM (SELECT {group} AS group_key,COUNT(*) AS n
                           {} WHERE {} GROUP BY group_key)",
                parts.from_sql, parts.where_sql
            )
        };
        let mut params = vec![Value::Integer(columns), Value::Integer(columns)];
        params.extend(parts.params);
        let count: i64 = self
            .conn
            .query_row(&sql, params_from_iter(params), |row| row.get(0))?;
        Ok(count.max(0) as u64)
    }

    pub fn query_albums(
        &self,
        descriptor: &QueryDescriptor,
        cursor: Option<&AlbumCursor>,
        page_size: usize,
    ) -> Result<AlbumPage> {
        self.query_albums_timed(descriptor, cursor, page_size)
            .map(|(page, _)| page)
    }

    pub fn query_albums_timed(
        &self,
        descriptor: &QueryDescriptor,
        cursor: Option<&AlbumCursor>,
        page_size: usize,
    ) -> Result<(AlbumPage, QueryMetrics)> {
        validate_albums_descriptor(descriptor)?;
        let limit = page_size.clamp(1, MAX_PAGE_SIZE);
        let parts = album_filter_parts(descriptor, cursor)?;
        let order = album_order_fields(descriptor, "am")
            .into_iter()
            .map(|field| {
                format!(
                    "{} {}",
                    field.expression,
                    if field.descending { "DESC" } else { "ASC" }
                )
            })
            .chain(std::iter::once("am.edition_id ASC".to_string()))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT {ALBUM_COLUMNS} {} WHERE {} ORDER BY {order} LIMIT {}",
            parts.from_sql,
            parts.where_sql,
            limit + 1
        );
        let key = descriptor_key(descriptor);
        let started = Instant::now();
        let mut stmt = self.conn.prepare_cached(&sql)?;
        let mapped = stmt.query_map(params_from_iter(parts.params), map_album_row)?;
        let mut values = mapped.collect::<rusqlite::Result<Vec<_>>>()?;
        for value in &mut values {
            value.cursor.descriptor_key = key.clone();
        }
        let sql_time = started.elapsed();
        let has_more = values.len() > limit;
        values.truncate(limit);
        let next_cursor = has_more.then(|| values.last().expect("non-empty page").cursor.clone());
        let rows = values
            .into_iter()
            .map(|value| value.record)
            .collect::<Vec<_>>();
        let metrics = QueryMetrics {
            sql_time,
            rows: rows.len(),
        };
        Ok((
            AlbumPage {
                rows,
                next_cursor,
                has_more,
            },
            metrics,
        ))
    }

    pub fn count_artists(&self, descriptor: &QueryDescriptor) -> Result<u64> {
        validate_artists_descriptor(descriptor)?;
        let parts = artist_filter_parts(descriptor, None)?;
        let sql = format!(
            "SELECT COUNT(*) {} WHERE {}",
            parts.from_sql, parts.where_sql
        );
        let count: i64 = self
            .conn
            .query_row(&sql, params_from_iter(parts.params), |row| row.get(0))?;
        Ok(count.max(0) as u64)
    }

    /// Exact visual length of the Artists rail: one row per artist plus one
    /// header for every non-empty A-Z/# bucket under the immutable query.
    pub fn count_artist_entries(&self, descriptor: &QueryDescriptor) -> Result<u64> {
        validate_artists_descriptor(descriptor)?;
        let parts = artist_filter_parts(descriptor, None)?;
        let sql = format!(
            "SELECT COUNT(*) + COUNT(DISTINCT {}) {} WHERE {}",
            artist_initial_expression("ar"),
            parts.from_sql,
            parts.where_sql
        );
        let count: i64 = self
            .conn
            .query_row(&sql, params_from_iter(parts.params), |row| row.get(0))?;
        Ok(count.max(0) as u64)
    }

    pub fn query_artists(
        &self,
        descriptor: &QueryDescriptor,
        cursor: Option<&ArtistCursor>,
        page_size: usize,
    ) -> Result<ArtistPage> {
        self.query_artists_timed(descriptor, cursor, page_size)
            .map(|(page, _)| page)
    }

    pub fn query_artists_timed(
        &self,
        descriptor: &QueryDescriptor,
        cursor: Option<&ArtistCursor>,
        page_size: usize,
    ) -> Result<(ArtistPage, QueryMetrics)> {
        validate_artists_descriptor(descriptor)?;
        let limit = page_size.clamp(1, MAX_PAGE_SIZE);
        let parts = artist_filter_parts(descriptor, cursor)?;
        let descending = artist_descending(descriptor);
        let order = if descending { "DESC" } else { "ASC" };
        let sql = format!(
            "SELECT ar.artist_key,ar.display_name,{},{},ar.artwork_source,
                    ar.artwork_token,{},ar.sort_name
               {} WHERE {}
              ORDER BY ar.sort_name {order},ar.artist_key {order} LIMIT {}",
            parts.album_count_sql,
            parts.track_count_sql,
            parts.source_sql,
            parts.from_sql,
            parts.where_sql,
            limit + 1
        );
        let key = descriptor_key(descriptor);
        let started = Instant::now();
        let mut stmt = self.conn.prepare_cached(&sql)?;
        let mapped = stmt.query_map(params_from_iter(parts.params), |row| {
            Ok((
                ArtistRecord {
                    artist_key: row.get(0)?,
                    display_name: row.get(1)?,
                    album_count: row.get::<_, i64>(2)?.max(0) as u32,
                    track_count: row.get::<_, i64>(3)?.max(0) as u32,
                    artwork_source: row.get(4)?,
                    artwork_token: row.get(5)?,
                    source: row.get(6)?,
                },
                row.get::<_, String>(7)?,
            ))
        })?;
        let mut values = mapped.collect::<rusqlite::Result<Vec<_>>>()?;
        let sql_time = started.elapsed();
        let has_more = values.len() > limit;
        values.truncate(limit);
        let next_cursor = has_more.then(|| {
            let (record, sort_name) = values.last().expect("non-empty page");
            ArtistCursor {
                descriptor_key: key,
                sort_name: sort_name.clone(),
                artist_key: record.artist_key.clone(),
            }
        });
        let rows = values
            .into_iter()
            .map(|(record, _)| record)
            .collect::<Vec<_>>();
        let metrics = QueryMetrics {
            sql_time,
            rows: rows.len(),
        };
        Ok((
            ArtistPage {
                rows,
                next_cursor,
                has_more,
            },
            metrics,
        ))
    }

    /// Count the selected artist's albums through the normalized,
    /// source-aware `edition_artists` relationship. No album collection is
    /// materialized in the frontend to answer this question.
    pub fn count_artist_albums(&self, artist_key: &str, sources: &[SourceKey]) -> Result<u64> {
        validate_artist_key(artist_key)?;
        let (where_sql, values) = artist_album_filter(artist_key, sources, None)?;
        let sql = format!("SELECT COUNT(*) FROM albums_materialized am WHERE {where_sql}");
        let count: i64 = self
            .conn
            .query_row(&sql, params_from_iter(values), |row| row.get(0))?;
        Ok(count.max(0) as u64)
    }

    pub fn query_artist_albums(
        &self,
        artist_key: &str,
        sources: &[SourceKey],
        cursor: Option<&AlbumCursor>,
        page_size: usize,
    ) -> Result<AlbumPage> {
        self.query_artist_albums_timed(artist_key, sources, cursor, page_size)
            .map(|(page, _)| page)
    }

    pub fn query_artist_albums_timed(
        &self,
        artist_key: &str,
        sources: &[SourceKey],
        cursor: Option<&AlbumCursor>,
        page_size: usize,
    ) -> Result<(AlbumPage, QueryMetrics)> {
        validate_artist_key(artist_key)?;
        let limit = page_size.clamp(1, MAX_PAGE_SIZE);
        let (where_sql, values) = artist_album_filter(artist_key, sources, cursor)?;
        let sql = format!(
            "SELECT {ALBUM_COLUMNS} FROM albums_materialized am
              WHERE {where_sql}
              ORDER BY am.sort_title,am.edition_id LIMIT {}",
            limit + 1
        );
        let key = artist_relation_key(artist_key, sources);
        let started = Instant::now();
        let mut stmt = self.conn.prepare_cached(&sql)?;
        let mapped = stmt.query_map(params_from_iter(values), map_album_row)?;
        let mut values = mapped.collect::<rusqlite::Result<Vec<_>>>()?;
        for value in &mut values {
            value.cursor.descriptor_key = key.clone();
        }
        let sql_time = started.elapsed();
        let has_more = values.len() > limit;
        values.truncate(limit);
        let next_cursor = has_more.then(|| values.last().expect("non-empty page").cursor.clone());
        let rows = values
            .into_iter()
            .map(|value| value.record)
            .collect::<Vec<_>>();
        let metrics = QueryMetrics {
            sql_time,
            rows: rows.len(),
        };
        Ok((
            AlbumPage {
                rows,
                next_cursor,
                has_more,
            },
            metrics,
        ))
    }

    pub fn stats(&self) -> Result<CatalogStats> {
        let track_count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM tracks", [], |row| row.get(0))?;
        let mut stmt = self.conn.prepare(
            "SELECT source_kind, source_instance, COUNT(*)
               FROM tracks
              GROUP BY source_kind, source_instance
              ORDER BY source_kind, source_instance",
        )?;
        let rows = stmt.query_map([], |row| {
            let word: String = row.get(0)?;
            Ok((
                SourceKey {
                    source: SourceKind::from_str(&word).expect("schema source CHECK"),
                    source_instance: row.get(1)?,
                },
                row.get::<_, i64>(2)? as u64,
            ))
        })?;
        let source_counts = rows.collect::<rusqlite::Result<Vec<_>>>()?;
        let page_size_bytes: i64 = self
            .conn
            .query_row("PRAGMA page_size", [], |row| row.get(0))?;
        let page_count: i64 = self
            .conn
            .query_row("PRAGMA page_count", [], |row| row.get(0))?;
        Ok(CatalogStats {
            schema_version: SCHEMA_VERSION,
            generation: self.generation,
            track_count: track_count as u64,
            source_counts,
            page_size_bytes: page_size_bytes as u64,
            page_count: page_count as u64,
        })
    }

    pub fn integrity_check(&self) -> Result<IntegrityReport> {
        let result: String = self
            .conn
            .query_row("PRAGMA quick_check", [], |row| row.get(0))?;
        let foreign_key_violations: i64 =
            self.conn
                .query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
                    row.get(0)
                })?;
        let tracks_fts_ok = self
            .conn
            .execute(
                "INSERT INTO tracks_fts(tracks_fts) VALUES ('integrity-check')",
                [],
            )
            .is_ok();
        let albums_fts_ok = self
            .conn
            .execute(
                "INSERT INTO albums_fts(albums_fts) VALUES ('integrity-check')",
                [],
            )
            .is_ok();
        let artists_fts_ok = self
            .conn
            .execute(
                "INSERT INTO artists_fts(artists_fts) VALUES ('integrity-check')",
                [],
            )
            .is_ok();
        Ok(IntegrityReport {
            sqlite_ok: result == "ok",
            foreign_key_violations: foreign_key_violations as u64,
            fts_ok: tracks_fts_ok && albums_fts_ok && artists_fts_ok,
        })
    }

    /// Re-check the active generation without acquiring a write connection.
    /// FTS5 consistency remains an activation invariant; this runtime probe
    /// covers SQLite pages, foreign keys and every derived aggregate.
    pub fn runtime_integrity_check(&self) -> Result<RuntimeIntegrityReport> {
        // A database-wide `quick_check` asks FTS5 to validate its inverted
        // index, and SQLite implements that virtual-table callback as a write
        // operation. Check every ordinary catalog table explicitly so this
        // remains valid on the read-only active connection; the full FTS
        // integrity command is an activation gate before the file is promoted.
        let mut sqlite_ok = true;
        let mut failures = Vec::new();
        for table in [
            "catalog_meta",
            "source_state",
            "logical_albums",
            "editions",
            "source_copies",
            "tracks",
            "artist_credits",
            "artist_identity_credits",
            "albums_materialized",
            "artists_materialized",
            "artist_source_stats",
            "edition_artists",
        ] {
            let result: String =
                self.conn
                    .query_row(&format!("PRAGMA quick_check('{table}')"), [], |row| {
                        row.get(0)
                    })?;
            if result != "ok" {
                sqlite_ok = false;
                failures.push(format!("{table}: {result}"));
            }
        }
        let foreign_key_violations: i64 =
            self.conn
                .query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
                    row.get(0)
                })?;
        Ok(RuntimeIntegrityReport {
            sqlite_ok,
            sqlite_result: if failures.is_empty() {
                "ok".to_string()
            } else {
                failures.join("; ")
            },
            foreign_key_violations: foreign_key_violations as u64,
            materialized_views_ok: self.materialized_views_valid()?,
        })
    }

    #[cfg(test)]
    pub(crate) fn connection(&self) -> &Connection {
        &self.conn
    }
}

/// Collapse a repeated `collection • contributor` album-artist family into
/// one collection identity while retaining every track/contributor credit.
///
/// A single bullet-bearing name is left intact because it can be a deliberate
/// collaboration or stage name. Two distinct suffixes sharing a normalized
/// prefix provide corpus-level evidence that the source encoded a family of
/// album credits rather than separate root artists. Only the derived credit
/// relation is rebuilt; raw credits plus track and album presentation metadata
/// remain byte-for-byte source-authored for the next incremental generation.
fn rebuild_artist_identities(tx: &rusqlite::Transaction<'_>) -> Result<usize> {
    let mut families = HashMap::<String, AlbumArtistFamilyEvidence>::new();
    {
        let mut statement = tx.prepare(
            "SELECT display_name,COUNT(*)
               FROM artist_credits
              WHERE role='album_artist' AND instr(display_name,?1)>0
              GROUP BY display_name
              ORDER BY display_name",
        )?;
        let rows = statement.query_map(params![ALBUM_ARTIST_FAMILY_SEPARATOR], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?.max(0) as u64,
            ))
        })?;
        for row in rows {
            let (display_name, count) = row?;
            let Some((prefix, suffix)) = split_album_artist_family(&display_name) else {
                continue;
            };
            let family_key = normalize_artist_key(prefix);
            let suffix_key = normalize_artist_key(suffix);
            if family_key.is_empty() || suffix_key.is_empty() {
                continue;
            }
            let evidence = families.entry(family_key).or_default();
            evidence.suffixes.insert(suffix_key);
            *evidence
                .display_counts
                .entry(prefix.to_string())
                .or_default() += count;
        }
    }

    let mut aliases = Vec::new();
    for (family_key, evidence) in &families {
        if evidence.suffixes.len() < 2 {
            continue;
        }
        let canonical_display = {
            let mut displays = evidence.display_counts.iter().collect::<Vec<_>>();
            displays.sort_by(|(left_name, left_count), (right_name, right_count)| {
                right_count
                    .cmp(left_count)
                    .then_with(|| left_name.cmp(right_name))
            });
            displays
                .into_iter()
                .next()
                .map(|(display_name, _)| (*display_name).clone())
                .unwrap_or_default()
        };
        for prefix in evidence.display_counts.keys() {
            aliases.push((
                prefix.clone(),
                family_key.clone(),
                canonical_display.clone(),
            ));
        }
    }
    aliases.sort();

    tx.execute("DELETE FROM artist_identity_credits", [])?;
    tx.execute(
        "INSERT OR IGNORE INTO artist_identity_credits(
             catalog_id,artist_key,display_name,role,ordinal
         ) SELECT catalog_id,artist_key,display_name,role,ordinal
             FROM artist_credits",
        [],
    )?;
    let mut rewritten = 0;
    for (prefix, family_key, canonical_display) in aliases {
        rewritten += tx.execute(
            "DELETE FROM artist_identity_credits
              WHERE role='album_artist' AND instr(display_name,?1)>0
                AND trim(substr(display_name,1,instr(display_name,?1)-1))=?2",
            params![ALBUM_ARTIST_FAMILY_SEPARATOR, prefix],
        )?;
        tx.execute(
            "INSERT OR IGNORE INTO artist_identity_credits(
                 catalog_id,artist_key,display_name,role,ordinal
             ) SELECT catalog_id,?1,?2,role,ordinal
                 FROM artist_credits
                WHERE role='album_artist' AND instr(display_name,?3)>0
                  AND trim(substr(display_name,1,instr(display_name,?3)-1))=?4",
            params![
                family_key,
                canonical_display,
                ALBUM_ARTIST_FAMILY_SEPARATOR,
                prefix
            ],
        )?;
    }
    Ok(rewritten)
}

fn split_album_artist_family(display_name: &str) -> Option<(&str, &str)> {
    let (prefix, suffix) = display_name.split_once(ALBUM_ARTIST_FAMILY_SEPARATOR)?;
    let prefix = prefix.trim();
    let suffix = suffix.trim();
    (!prefix.is_empty() && !suffix.is_empty()).then_some((prefix, suffix))
}

fn validate_ref(track_ref: &TrackRef) -> Result<()> {
    if track_ref.source_instance.trim().is_empty() {
        return Err(CatalogError::InvalidInput(
            "source instance must not be empty".to_string(),
        ));
    }
    if track_ref.native_id.trim().is_empty() {
        return Err(CatalogError::InvalidInput(
            "native track id must not be empty".to_string(),
        ));
    }
    Ok(())
}

fn validate_track(track: &ProjectedTrack) -> Result<()> {
    validate_ref(&track.track_ref)?;
    if track.duration_ms > i64::MAX as u64 {
        return Err(CatalogError::InvalidInput(
            "duration does not fit SQLite INTEGER".to_string(),
        ));
    }
    Ok(())
}

fn ensure_source_copy(conn: &Connection, track: &ProjectedTrack, generation: u64) -> Result<i64> {
    let display_artist = if track.album_artist.trim().is_empty() {
        track.artist.trim()
    } else {
        track.album_artist.trim()
    };
    let native_album_id = track
        .native_album_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let fallback_id;
    let (association, evidence, album_id) = if let Some(native) = native_album_id {
        ("source_native", native, native)
    } else {
        fallback_id = stable_parts(&[
            &normalize_sort_key(&track.album),
            &normalize_sort_key(display_artist),
            &track.year.map(|year| year.to_string()).unwrap_or_default(),
        ]);
        ("text_fallback", fallback_id.as_str(), fallback_id.as_str())
    };
    let stable_key = stable_parts(&[
        track.track_ref.source.as_str(),
        &track.track_ref.source_instance,
        album_id,
    ]);
    let sort_title = normalize_sort_key(&track.album);
    let sort_artist = normalize_sort_key(display_artist);
    let logical_album_id: i64 = conn.query_row(
        "INSERT INTO logical_albums(
             stable_key,display_title,sort_title,display_artist,sort_artist,
             association_strength,association_evidence
         ) VALUES (?1,?2,?3,?4,?5,?6,?7)
         ON CONFLICT(stable_key) DO UPDATE SET
             display_title=excluded.display_title,
             sort_title=excluded.sort_title,
             display_artist=excluded.display_artist,
             sort_artist=excluded.sort_artist,
             association_strength=excluded.association_strength,
             association_evidence=excluded.association_evidence
         RETURNING logical_album_id",
        params![
            stable_key,
            track.album,
            sort_title,
            display_artist,
            sort_artist,
            association,
            evidence,
        ],
        |row| row.get(0),
    )?;
    let edition_key = stable_parts(&[&stable_key, "edition"]);
    let edition_id: i64 = conn.query_row(
        "INSERT INTO editions(
             logical_album_id,edition_key,display_title,display_artist,release_year,
             provider_release_id,evidence_kind,evidence_value
         ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)
         ON CONFLICT(edition_key) DO UPDATE SET
             display_title=excluded.display_title,
             display_artist=excluded.display_artist,
             release_year=excluded.release_year,
             provider_release_id=excluded.provider_release_id,
             evidence_kind=excluded.evidence_kind,
             evidence_value=excluded.evidence_value
         RETURNING edition_id",
        params![
            logical_album_id,
            edition_key,
            track.album,
            display_artist,
            track.year.map(i64::from),
            native_album_id,
            association,
            evidence,
        ],
        |row| row.get(0),
    )?;
    let local_directory = track.local_path.as_deref().and_then(|path| {
        Path::new(path)
            .parent()
            .map(|parent| parent.to_string_lossy().into_owned())
    });
    let source_copy_id: i64 = conn.query_row(
        "INSERT INTO source_copies(
             edition_id,source_kind,source_instance,native_album_id,local_directory,
             available,last_observed_generation
         ) VALUES (?1,?2,?3,?4,?5,?6,?7)
         ON CONFLICT(source_kind,source_instance,native_album_id) DO UPDATE SET
             edition_id=excluded.edition_id,
             local_directory=excluded.local_directory,
             available=excluded.available,
             last_observed_generation=excluded.last_observed_generation
         RETURNING source_copy_id",
        params![
            edition_id,
            track.track_ref.source.as_str(),
            track.track_ref.source_instance,
            album_id,
            local_directory,
            i64::from(track.available),
            generation as i64,
        ],
        |row| row.get(0),
    )?;
    Ok(source_copy_id)
}

fn stable_parts(parts: &[&str]) -> String {
    let mut key = String::new();
    for part in parts {
        key.push_str(&part.len().to_string());
        key.push(':');
        key.push_str(part);
        key.push('|');
    }
    key
}

fn validate_tracks_descriptor(descriptor: &QueryDescriptor) -> Result<()> {
    if descriptor.surface() != crate::QuerySurface::Tracks {
        return Err(CatalogError::InvalidInput(
            "a non-Tracks descriptor was passed to a Tracks query".to_string(),
        ));
    }
    Ok(())
}

fn upsert_track(
    upsert: &mut Statement<'_>,
    clear_credits: &mut Statement<'_>,
    insert_credit: &mut Statement<'_>,
    track: &ProjectedTrack,
) -> Result<i64> {
    let sort_title = normalize_sort_key(&track.title);
    let sort_artist = normalize_sort_key(if track.album_artist.trim().is_empty() {
        &track.artist
    } else {
        &track.album_artist
    });
    let sort_track_artist = normalize_sort_key(&track.artist);
    let sort_album = normalize_sort_key(&track.album);
    let year_missing = i64::from(track.year.is_none());
    let year_value = track.year.unwrap_or(0) as i64;
    let disc_sort = track.disc_number.unwrap_or(0) as i64;
    let track_sort = track.track_number.unwrap_or(0) as i64;
    let mut seen_credits = HashSet::new();
    let credits = track
        .credits
        .iter()
        .filter_map(|credit| {
            let name = credit.display_name.trim();
            (!name.is_empty() && seen_credits.insert(name.to_lowercase())).then(|| name.to_string())
        })
        .collect::<Vec<_>>()
        .join(" \u{1f} ");

    let catalog_id: i64 = upsert.query_row(
        params![
            track.track_ref.source.as_str(),
            track.track_ref.source_instance,
            track.track_ref.native_id,
            track.source_raw,
            track.local_track_id,
            track.local_path,
            track.source_copy_id,
            track.title,
            sort_title,
            track.artist,
            sort_artist,
            sort_track_artist,
            track.album_artist,
            track.album,
            sort_album,
            credits,
            track.duration_ms as i64,
            track.year.map(i64::from),
            year_missing,
            year_value,
            track.disc_number.map(i64::from),
            disc_sort,
            track.track_number.map(i64::from),
            track_sort,
            track.format.trim().to_ascii_lowercase(),
            track.bit_depth.map(i64::from),
            track.sample_rate_hz.map(i64::from),
            track.artwork_token,
            track.isrc,
            track.musicbrainz_recording_id,
            track.added_at,
            i64::from(track.available),
            track.observed_generation,
        ],
        |row| row.get(0),
    )?;

    clear_credits.execute(params![catalog_id])?;
    for credit in &track.credits {
        let artist_key = normalize_artist_key(&credit.display_name);
        if artist_key.is_empty() {
            continue;
        }
        insert_credit.execute(params![
            catalog_id,
            artist_key,
            credit.display_name.trim(),
            credit.role.as_str(),
            i64::from(credit.ordinal),
        ])?;
    }
    Ok(catalog_id)
}

const UPSERT_TRACK_SQL: &str = "INSERT INTO tracks (
    source_kind, source_instance, native_track_id, source_raw, local_track_id,
    local_path, source_copy_id, title, sort_title, artist, sort_artist,
    sort_track_artist, album_artist, album, sort_album, credits, duration_ms,
    year, year_missing, year_value,
    disc_number, disc_sort, track_number, track_sort, format, bit_depth,
    sample_rate_hz, artwork_token, isrc, musicbrainz_recording_id, added_at,
    available, last_observed_generation
) VALUES (
    ?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,
    ?19,?20,?21,?22,?23,?24,?25,?26,?27,?28,?29,?30,?31,?32,?33
)
ON CONFLICT(source_kind, source_instance, native_track_id) DO UPDATE SET
    source_raw=excluded.source_raw,
    local_track_id=excluded.local_track_id,
    local_path=excluded.local_path,
    source_copy_id=excluded.source_copy_id,
    title=excluded.title,
    sort_title=excluded.sort_title,
    artist=excluded.artist,
    sort_artist=excluded.sort_artist,
    sort_track_artist=excluded.sort_track_artist,
    album_artist=excluded.album_artist,
    album=excluded.album,
    sort_album=excluded.sort_album,
    credits=excluded.credits,
    duration_ms=excluded.duration_ms,
    year=excluded.year,
    year_missing=excluded.year_missing,
    year_value=excluded.year_value,
    disc_number=excluded.disc_number,
    disc_sort=excluded.disc_sort,
    track_number=excluded.track_number,
    track_sort=excluded.track_sort,
    format=excluded.format,
    bit_depth=excluded.bit_depth,
    sample_rate_hz=excluded.sample_rate_hz,
    artwork_token=excluded.artwork_token,
    isrc=excluded.isrc,
    musicbrainz_recording_id=excluded.musicbrainz_recording_id,
    added_at=excluded.added_at,
    available=excluded.available,
    last_observed_generation=excluded.last_observed_generation
RETURNING catalog_id";

struct QueryParts {
    where_sql: String,
    params: Vec<Value>,
    sort: TrackSort,
}

fn filter_parts(
    descriptor: &QueryDescriptor,
    cursor: Option<&TrackCursor>,
    alias: &str,
) -> Result<QueryParts> {
    let sort = effective_sort(descriptor);
    if let Some(cursor) = cursor {
        if cursor.sort != sort || cursor.descriptor_key != descriptor_key(descriptor) {
            return Err(CatalogError::CursorDescriptorMismatch);
        }
    }
    let mut predicates = Vec::new();
    let mut values = Vec::new();
    if descriptor.available_only() {
        predicates.push(format!("{alias}.available = ?"));
        values.push(Value::Integer(1));
    }
    if !descriptor.sources().is_empty() {
        let mut source_predicates = Vec::new();
        for source in descriptor.sources() {
            if source.source_instance.trim().is_empty() {
                return Err(CatalogError::InvalidInput(
                    "source filter instance must not be empty".to_string(),
                ));
            }
            source_predicates.push(format!(
                "({alias}.source_kind = ? AND {alias}.source_instance = ?)"
            ));
            values.push(Value::Text(source.source.as_str().to_string()));
            values.push(Value::Text(source.source_instance.clone()));
        }
        predicates.push(format!("({})", source_predicates.join(" OR ")));
    }
    if !descriptor.source_buckets().is_empty() {
        let mut source_arms = Vec::new();
        for bucket in descriptor.source_buckets() {
            source_arms.push(match bucket.as_str() {
                // A scanned file is owned by the Local catalog source and is
                // normally spelled `user` in source_raw. Qobuz downloads can
                // share that ownership, so raw provenance must remove them
                // from Local and expose them through the Offline chip instead.
                "local" => format!(
                    "({alias}.source_kind='local' AND {alias}.source_raw NOT IN ('qobuz_download','qobuz_purchase'))"
                ),
                "offline" => format!(
                    "({alias}.source_kind='offline' OR {alias}.source_raw IN ('qobuz_download','qobuz_purchase'))"
                ),
                "plex" => format!("{alias}.source_kind='plex'"),
                "jellyfin" => format!("{alias}.source_kind='jellyfin'"),
                "subsonic" => format!("{alias}.source_kind='subsonic'"),
                _ => {
                    return Err(CatalogError::InvalidInput(format!(
                        "unknown track source bucket {bucket}"
                    )))
                }
            });
        }
        predicates.push(format!("({})", source_arms.join(" OR ")));
    }
    if !descriptor.formats().is_empty() || descriptor.other_formats() {
        let mut format_arms = Vec::new();
        if !descriptor.formats().is_empty() {
            format_arms.push(format!(
                "{alias}.format IN ({})",
                std::iter::repeat("?")
                    .take(descriptor.formats().len())
                    .collect::<Vec<_>>()
                    .join(",")
            ));
            values.extend(descriptor.formats().iter().cloned().map(Value::Text));
        }
        if descriptor.other_formats() {
            format_arms.push(format!(
                "{alias}.format NOT IN ('flac','alac','ape','wav','wave','mp3','aac')"
            ));
        }
        predicates.push(format!("({})", format_arms.join(" OR ")));
    }
    if !descriptor.quality_tiers().is_empty() {
        // Keep this expression in lockstep with qbz-qt's `tier_of` and the
        // albums_materialized quality_tier projection. Tracks deliberately do
        // not materialize a second quality column just for this filter.
        let tier = format!(
            "CASE \
                WHEN LOWER({alias}.format)='mp3' THEN 'mp3' \
                WHEN LOWER({alias}.format) IN ('dsd','dsf','dff') THEN 'dsd' \
                WHEN {alias}.bit_depth>=24 AND {alias}.sample_rate_hz>96000 THEN 'max' \
                WHEN {alias}.bit_depth>=24 THEN 'hires' \
                WHEN {alias}.bit_depth IS NOT NULL THEN 'cd' \
                WHEN {alias}.sample_rate_hz>=44100 THEN 'cd' \
                ELSE '' END"
        );
        let mut quality_arms = Vec::new();
        for quality in descriptor.quality_tiers() {
            quality_arms.push(match quality.as_str() {
                "dsd" => format!("({tier})='dsd'"),
                "hires" => format!("({tier}) IN ('hires','max')"),
                "cd" => format!("({tier})='cd'"),
                "lossy" => format!("({tier}) IN ('mp3','lossy')"),
                _ => {
                    return Err(CatalogError::InvalidInput(format!(
                        "unknown track quality tier {quality}"
                    )))
                }
            });
        }
        predicates.push(format!("({})", quality_arms.join(" OR ")));
    }

    if let Some(cursor) = cursor {
        let (predicate, cursor_values) = cursor_predicate(cursor, descriptor.group(), alias);
        predicates.push(predicate);
        values.extend(cursor_values);
    }
    if predicates.is_empty() {
        predicates.push("1".to_string());
    }
    Ok(QueryParts {
        where_sql: predicates.join(" AND "),
        params: values,
        sort,
    })
}

fn count_query(descriptor: &QueryDescriptor) -> Result<(String, Vec<Value>)> {
    if descriptor.search().is_empty() {
        let parts = filter_parts(descriptor, None, "t")?;
        return Ok((
            format!("SELECT COUNT(*) FROM tracks t WHERE {}", parts.where_sql),
            parts.params,
        ));
    }
    let match_value = fts_match_value(descriptor.search())?;
    let parts = filter_parts(descriptor, None, "t")?;
    let sql = format!(
        "SELECT COUNT(*)
           FROM tracks_fts
           CROSS JOIN tracks t NOT INDEXED
          WHERE tracks_fts MATCH ?
            AND t.catalog_id = tracks_fts.rowid
            AND {}",
        parts.where_sql
    );
    let mut values = vec![match_value];
    values.extend(parts.params);
    Ok((sql, values))
}

fn page_query(
    descriptor: &QueryDescriptor,
    cursor: Option<&TrackCursor>,
    limit: usize,
) -> Result<(String, Vec<Value>, TrackSort)> {
    if descriptor.search().is_empty() {
        let parts = filter_parts(descriptor, cursor, "t")?;
        let sql = format!(
            "SELECT {TRACK_COLUMNS}
               FROM tracks t
              WHERE {}
              ORDER BY {}
              LIMIT {}",
            parts.where_sql,
            order_clause(parts.sort, descriptor.group(), "t"),
            limit + 1
        );
        return Ok((sql, parts.params, parts.sort));
    }

    let match_value = fts_match_value(descriptor.search())?;
    let parts = filter_parts(descriptor, cursor, "t")?;
    let sql = format!(
        "SELECT {TRACK_COLUMNS}
           FROM tracks_fts
           CROSS JOIN tracks t NOT INDEXED
          WHERE tracks_fts MATCH ?
            AND t.catalog_id = tracks_fts.rowid
            AND {}
          ORDER BY {}
          LIMIT {}",
        parts.where_sql,
        order_clause(parts.sort, descriptor.group(), "t"),
        limit + 1,
    );
    let mut values = vec![match_value];
    values.extend(parts.params);
    Ok((sql, values, parts.sort))
}

fn fts_match_value(search: &str) -> Result<Value> {
    if search.chars().count() < 3 {
        return Err(CatalogError::SearchTooShort);
    }
    let escaped = search.replace('"', "\"\"");
    Ok(Value::Text(format!("\"{escaped}\"")))
}

fn effective_sort(descriptor: &QueryDescriptor) -> TrackSort {
    match descriptor.group() {
        TrackGroup::Album => TrackSort::Default,
        TrackGroup::Artist => TrackSort::ArtistAsc,
        TrackGroup::Name => TrackSort::TitleAsc,
        TrackGroup::Off => descriptor.sort(),
    }
}

fn order_clause(sort: TrackSort, group: TrackGroup, alias: &str) -> String {
    let columns = if group == TrackGroup::Artist {
        "sort_track_artist, sort_album, sort_title, catalog_id"
    } else {
        match sort {
            TrackSort::Default => {
                "sort_album, sort_artist, disc_sort, track_sort, sort_title, catalog_id"
            }
            TrackSort::TitleAsc => "sort_title, sort_artist, catalog_id",
            TrackSort::TitleDesc => "sort_title DESC, sort_artist, catalog_id",
            TrackSort::ArtistAsc => "sort_artist, sort_album, disc_sort, track_sort, catalog_id",
            TrackSort::ArtistDesc => {
                "sort_artist DESC, sort_album, disc_sort, track_sort, catalog_id"
            }
            TrackSort::YearAsc => {
                "year_missing, year_value, sort_album, disc_sort, track_sort, catalog_id"
            }
            TrackSort::YearDesc => {
                "year_missing, year_value DESC, sort_album, disc_sort, track_sort, catalog_id"
            }
            TrackSort::AddedDesc => "added_at DESC, sort_album, disc_sort, track_sort, catalog_id",
        }
    };
    columns
        .split(", ")
        .map(|column| {
            let (name, suffix) = column.split_once(' ').unwrap_or((column, ""));
            if suffix.is_empty() {
                format!("{alias}.{name}")
            } else {
                format!("{alias}.{name} {suffix}")
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn cursor_predicate(cursor: &TrackCursor, group: TrackGroup, alias: &str) -> (String, Vec<Value>) {
    let text = |value: &str| Value::Text(value.to_string());
    let integer = Value::Integer;
    let (predicate, values) = if group == TrackGroup::Artist {
        (
            "(t.sort_track_artist,t.sort_album,t.sort_title,t.catalog_id) > (?,?,?,?)",
            vec![
                text(&cursor.sort_track_artist),
                text(&cursor.sort_album),
                text(&cursor.sort_title),
                integer(cursor.row_id),
            ],
        )
    } else {
        match cursor.sort {
        TrackSort::Default => (
            "(t.sort_album,t.sort_artist,t.disc_sort,t.track_sort,t.sort_title,t.catalog_id) > (?,?,?,?,?,?)",
            vec![
                text(&cursor.sort_album),
                text(&cursor.sort_artist),
                integer(cursor.disc_sort),
                integer(cursor.track_sort),
                text(&cursor.sort_title),
                integer(cursor.row_id),
            ],
        ),
        TrackSort::TitleAsc => (
            "(t.sort_title,t.sort_artist,t.catalog_id) > (?,?,?)",
            vec![
                text(&cursor.sort_title),
                text(&cursor.sort_artist),
                integer(cursor.row_id),
            ],
        ),
        TrackSort::TitleDesc => (
            "(t.sort_title < ? OR (t.sort_title = ? AND (t.sort_artist,t.catalog_id) > (?,?)))",
            vec![
                text(&cursor.sort_title),
                text(&cursor.sort_title),
                text(&cursor.sort_artist),
                integer(cursor.row_id),
            ],
        ),
        TrackSort::ArtistAsc => (
            "(t.sort_artist,t.sort_album,t.disc_sort,t.track_sort,t.catalog_id) > (?,?,?,?,?)",
            vec![
                text(&cursor.sort_artist),
                text(&cursor.sort_album),
                integer(cursor.disc_sort),
                integer(cursor.track_sort),
                integer(cursor.row_id),
            ],
        ),
        TrackSort::ArtistDesc => (
            "(t.sort_artist < ? OR (t.sort_artist = ? AND (t.sort_album,t.disc_sort,t.track_sort,t.catalog_id) > (?,?,?,?)))",
            vec![
                text(&cursor.sort_artist),
                text(&cursor.sort_artist),
                text(&cursor.sort_album),
                integer(cursor.disc_sort),
                integer(cursor.track_sort),
                integer(cursor.row_id),
            ],
        ),
        TrackSort::YearAsc => (
            "(t.year_missing,t.year_value,t.sort_album,t.disc_sort,t.track_sort,t.catalog_id) > (?,?,?,?,?,?)",
            vec![
                integer(cursor.year_missing),
                integer(cursor.year_value),
                text(&cursor.sort_album),
                integer(cursor.disc_sort),
                integer(cursor.track_sort),
                integer(cursor.row_id),
            ],
        ),
        TrackSort::YearDesc => (
            "(t.year_missing > ? OR (t.year_missing = ? AND (t.year_value < ? OR (t.year_value = ? AND (t.sort_album,t.disc_sort,t.track_sort,t.catalog_id) > (?,?,?,?)))))",
            vec![
                integer(cursor.year_missing),
                integer(cursor.year_missing),
                integer(cursor.year_value),
                integer(cursor.year_value),
                text(&cursor.sort_album),
                integer(cursor.disc_sort),
                integer(cursor.track_sort),
                integer(cursor.row_id),
            ],
        ),
        TrackSort::AddedDesc => (
            "(t.added_at < ? OR (t.added_at = ? AND (t.sort_album,t.disc_sort,t.track_sort,t.catalog_id) > (?,?,?,?)))",
            vec![
                integer(cursor.added_at),
                integer(cursor.added_at),
                text(&cursor.sort_album),
                integer(cursor.disc_sort),
                integer(cursor.track_sort),
                integer(cursor.row_id),
            ],
        ),
        }
    };
    (predicate.replace("t.", &format!("{alias}.")), values)
}

fn map_row(row: &Row<'_>) -> rusqlite::Result<RowWithCursor> {
    let source_word: String = row.get(0)?;
    let source = SourceKind::from_str(&source_word).expect("schema source CHECK");
    let sort_title = row.get(20)?;
    let sort_artist = row.get(21)?;
    let sort_track_artist = row.get(22)?;
    let sort_album = row.get(23)?;
    let year_missing = row.get(24)?;
    let year_value = row.get(25)?;
    let disc_sort = row.get(26)?;
    let track_sort = row.get(27)?;
    let added_at = row.get(28)?;
    let row_id = row.get(29)?;
    Ok(RowWithCursor {
        record: TrackRecord {
            track_ref: TrackRef {
                source,
                source_instance: row.get(1)?,
                native_id: row.get(2)?,
            },
            source_raw: row.get(3)?,
            local_track_id: row.get(4)?,
            local_path: row.get(5)?,
            native_album_id: row.get::<_, Option<String>>(6)?,
            title: row.get(7)?,
            artist: row.get(8)?,
            album_artist: row.get(9)?,
            album: row.get(10)?,
            duration_ms: row.get::<_, i64>(11)?.max(0) as u64,
            year: row.get::<_, Option<i64>>(12)?.map(|value| value as u32),
            disc_number: row.get::<_, Option<i64>>(13)?.map(|value| value as u32),
            track_number: row.get::<_, Option<i64>>(14)?.map(|value| value as u32),
            format: row.get(15)?,
            bit_depth: row.get::<_, Option<i64>>(16)?.map(|value| value as u32),
            sample_rate_hz: row.get::<_, Option<i64>>(17)?.map(|value| value as u32),
            artwork_token: row.get(18)?,
            available: row.get::<_, i64>(19)? != 0,
        },
        cursor: TrackCursor {
            // Filled by query_tracks_timed after the effective sort is known.
            sort: TrackSort::Default,
            descriptor_key: String::new(),
            sort_title,
            sort_artist,
            sort_track_artist,
            sort_album,
            year_missing,
            year_value,
            disc_sort,
            track_sort,
            added_at,
            row_id,
        },
    })
}

const TRACK_COLUMNS: &str = "
    t.source_kind, t.source_instance, t.native_track_id, t.source_raw,
    t.local_track_id, t.local_path,
    (SELECT NULLIF(sc.native_album_id,'') FROM source_copies sc
      WHERE sc.source_copy_id=t.source_copy_id),
    t.title, t.artist, t.album_artist, t.album, t.duration_ms, t.year, t.disc_number,
    t.track_number, t.format, t.bit_depth, t.sample_rate_hz, t.artwork_token, t.available,
    t.sort_title, t.sort_artist, t.sort_track_artist, t.sort_album, t.year_missing,
    t.year_value, t.disc_sort, t.track_sort, t.added_at, t.catalog_id";

struct AlbumRowWithCursor {
    record: AlbumRecord,
    cursor: AlbumCursor,
}

struct AlbumQueryParts {
    from_sql: String,
    where_sql: String,
    params: Vec<Value>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum AlbumOrderKind {
    TitleInitial,
    SortTitle,
    SortArtist,
    YearMissing,
    YearValue,
    AddedAt,
}

struct AlbumOrderField {
    kind: AlbumOrderKind,
    expression: String,
    descending: bool,
}

impl AlbumOrderField {
    fn cursor_value(&self, cursor: &AlbumCursor) -> Value {
        match self.kind {
            AlbumOrderKind::TitleInitial => {
                Value::Text(cursor.sort_title.chars().next().unwrap_or('#').to_string())
            }
            AlbumOrderKind::SortTitle => Value::Text(cursor.sort_title.clone()),
            AlbumOrderKind::SortArtist => Value::Text(cursor.sort_artist.clone()),
            AlbumOrderKind::YearMissing => Value::Integer(cursor.year_missing),
            AlbumOrderKind::YearValue => Value::Integer(cursor.year_value),
            AlbumOrderKind::AddedAt => Value::Integer(cursor.added_at),
        }
    }
}

fn validate_albums_descriptor(descriptor: &QueryDescriptor) -> Result<()> {
    if descriptor.surface() != QuerySurface::Albums {
        return Err(CatalogError::InvalidInput(
            "a non-Albums descriptor was passed to an Albums query".to_string(),
        ));
    }
    Ok(())
}

fn album_filter_parts(
    descriptor: &QueryDescriptor,
    cursor: Option<&AlbumCursor>,
) -> Result<AlbumQueryParts> {
    if let Some(cursor) = cursor {
        if cursor.descriptor_key != descriptor_key(descriptor) {
            return Err(CatalogError::CursorDescriptorMismatch);
        }
    }
    let mut predicates = Vec::new();
    let mut params = Vec::new();
    let from_sql = if descriptor.search().is_empty() {
        "FROM albums_materialized am".to_string()
    } else {
        params.push(fts_match_value(descriptor.search())?);
        predicates.push("albums_fts MATCH ?".to_string());
        predicates.push("am.edition_id=albums_fts.rowid".to_string());
        "FROM albums_fts CROSS JOIN albums_materialized am NOT INDEXED".to_string()
    };
    if descriptor.available_only() {
        predicates.push("am.available=1".to_string());
    }
    if !descriptor.sources().is_empty() {
        let mut source = Vec::new();
        for key in descriptor.sources() {
            if key.source_instance.trim().is_empty() {
                return Err(CatalogError::InvalidInput(
                    "source filter instance must not be empty".to_string(),
                ));
            }
            source.push("(scf.source_kind=? AND scf.source_instance=?)");
            params.push(Value::Text(key.source.as_str().to_string()));
            params.push(Value::Text(key.source_instance.clone()));
        }
        predicates.push(format!(
            "EXISTS (SELECT 1 FROM source_copies scf WHERE scf.edition_id=am.edition_id AND ({}))",
            source.join(" OR ")
        ));
    }
    if !descriptor.formats().is_empty() || descriptor.other_formats() {
        let mut format_arms = Vec::new();
        if !descriptor.formats().is_empty() {
            format_arms.push(format!(
                "am.format IN ({})",
                std::iter::repeat("?")
                    .take(descriptor.formats().len())
                    .collect::<Vec<_>>()
                    .join(",")
            ));
        }
        if descriptor.other_formats() {
            format_arms.push(
                "am.format NOT IN ('flac','alac','ape','wav','wave','mp3','aac')".to_string(),
            );
        }
        predicates.push(format!("({})", format_arms.join(" OR ")));
        params.extend(descriptor.formats().iter().cloned().map(Value::Text));
    }
    if !descriptor.source_buckets().is_empty() {
        let mut source_arms = Vec::new();
        for bucket in descriptor.source_buckets() {
            source_arms.push(match bucket.as_str() {
                "local" => "(am.source_kind='local' AND am.source_raw NOT IN ('qobuz_download','qobuz_purchase'))",
                "offline" => "(am.source_kind='offline' OR am.source_raw IN ('qobuz_download','qobuz_purchase'))",
                "plex" => "am.source_kind='plex'",
                "jellyfin" => "am.source_kind='jellyfin'",
                "subsonic" => "am.source_kind='subsonic'",
                _ => {
                    return Err(CatalogError::InvalidInput(format!(
                        "unknown album source bucket {bucket}"
                    )))
                }
            });
        }
        predicates.push(format!("({})", source_arms.join(" OR ")));
    }
    if !descriptor.quality_tiers().is_empty() {
        let mut quality = Vec::new();
        for tier in descriptor.quality_tiers() {
            match tier.as_str() {
                "dsd" => quality.push("am.quality_tier='dsd'"),
                "hires" => quality.push("am.quality_tier IN ('hires','max')"),
                "cd" => quality.push("am.quality_tier='cd'"),
                "lossy" => quality.push("am.quality_tier IN ('mp3','lossy')"),
                _ => {
                    return Err(CatalogError::InvalidInput(format!(
                        "unknown album quality tier {tier}"
                    )))
                }
            }
        }
        predicates.push(format!("({})", quality.join(" OR ")));
    }
    if let Some(cursor) = cursor {
        let (predicate, values) = album_cursor_predicate(descriptor, cursor);
        predicates.push(predicate);
        params.extend(values);
    }
    if predicates.is_empty() {
        predicates.push("1".to_string());
    }
    Ok(AlbumQueryParts {
        from_sql,
        where_sql: predicates.join(" AND "),
        params,
    })
}

fn album_group_expression(group: TrackGroup) -> &'static str {
    match group {
        TrackGroup::Artist => "am.sort_artist",
        TrackGroup::Name | TrackGroup::Album => "substr(am.sort_title,1,1)",
        TrackGroup::Off => "''",
    }
}

fn album_order_fields(descriptor: &QueryDescriptor, alias: &str) -> Vec<AlbumOrderField> {
    let expression = |name: &str| format!("{alias}.{name}");
    let mut fields = Vec::new();
    match descriptor.group() {
        TrackGroup::Artist => fields.push(AlbumOrderField {
            kind: AlbumOrderKind::SortArtist,
            expression: expression("sort_artist"),
            descending: false,
        }),
        TrackGroup::Name | TrackGroup::Album => fields.push(AlbumOrderField {
            kind: AlbumOrderKind::TitleInitial,
            expression: format!("substr({alias}.sort_title,1,1)"),
            descending: false,
        }),
        TrackGroup::Off => {}
    }
    let mut push = |kind: AlbumOrderKind, name: &str, descending: bool| {
        if fields.iter().any(|field| field.kind == kind) {
            return;
        }
        fields.push(AlbumOrderField {
            kind,
            expression: expression(name),
            descending,
        });
    };
    match descriptor.sort() {
        TrackSort::Default | TrackSort::ArtistAsc => {
            push(AlbumOrderKind::SortArtist, "sort_artist", false);
            push(AlbumOrderKind::SortTitle, "sort_title", false);
        }
        TrackSort::ArtistDesc => {
            push(AlbumOrderKind::SortArtist, "sort_artist", true);
            push(AlbumOrderKind::SortTitle, "sort_title", false);
        }
        TrackSort::TitleAsc => {
            push(AlbumOrderKind::SortTitle, "sort_title", false);
            push(AlbumOrderKind::SortArtist, "sort_artist", false);
        }
        TrackSort::TitleDesc => {
            push(AlbumOrderKind::SortTitle, "sort_title", true);
            push(AlbumOrderKind::SortArtist, "sort_artist", false);
        }
        TrackSort::YearAsc => {
            push(AlbumOrderKind::YearMissing, "year_missing", false);
            push(AlbumOrderKind::YearValue, "year_value", false);
            push(AlbumOrderKind::SortTitle, "sort_title", false);
        }
        TrackSort::YearDesc => {
            push(AlbumOrderKind::YearMissing, "year_missing", false);
            push(AlbumOrderKind::YearValue, "year_value", true);
            push(AlbumOrderKind::SortTitle, "sort_title", false);
        }
        TrackSort::AddedDesc => {
            push(AlbumOrderKind::AddedAt, "added_at", true);
            push(AlbumOrderKind::SortTitle, "sort_title", false);
        }
    }
    fields
}

fn album_cursor_predicate(
    descriptor: &QueryDescriptor,
    cursor: &AlbumCursor,
) -> (String, Vec<Value>) {
    fn build(
        fields: &[AlbumOrderField],
        cursor: &AlbumCursor,
        index: usize,
        params: &mut Vec<Value>,
    ) -> String {
        if index == fields.len() {
            params.push(Value::Integer(cursor.edition_id));
            return "am.edition_id>?".to_string();
        }
        let field = &fields[index];
        let comparison = if field.descending { "<" } else { ">" };
        params.push(field.cursor_value(cursor));
        params.push(field.cursor_value(cursor));
        let rest = build(fields, cursor, index + 1, params);
        format!(
            "({expr}{comparison}? OR ({expr}=? AND {rest}))",
            expr = field.expression
        )
    }
    let fields = album_order_fields(descriptor, "am");
    let mut params = Vec::new();
    let predicate = build(&fields, cursor, 0, &mut params);
    (predicate, params)
}

fn map_album_row(row: &Row<'_>) -> rusqlite::Result<AlbumRowWithCursor> {
    let source_word: String = row.get(1)?;
    let source = SourceKind::from_str(&source_word).expect("schema source CHECK");
    Ok(AlbumRowWithCursor {
        record: AlbumRecord {
            edition_id: row.get(0)?,
            source,
            native_album_id: row.get(2)?,
            source_raw: row.get(3)?,
            source_words: row
                .get::<_, String>(23)?
                .split('\u{1f}')
                .filter(|word| !word.is_empty())
                .map(str::to_string)
                .collect(),
            title: row.get(4)?,
            artist: row.get(5)?,
            all_artists: row.get(6)?,
            year: row.get::<_, Option<i64>>(7)?.map(|value| value as u32),
            track_count: row.get::<_, i64>(8)?.max(0) as u32,
            total_duration_ms: row.get::<_, i64>(9)?.max(0) as u64,
            quality_tier: row.get(10)?,
            format: row.get(11)?,
            bit_depth: row.get::<_, Option<i64>>(12)?.map(|value| value as u32),
            sample_rate_hz: row.get::<_, Option<i64>>(13)?.map(|value| value as u32),
            artwork_source: row.get(14)?,
            artwork_token: row.get(15)?,
            directory_path: row.get(16)?,
            folder_count: row.get::<_, i64>(17)?.max(0) as u32,
            added_at: row.get(18)?,
        },
        cursor: AlbumCursor {
            descriptor_key: String::new(),
            sort_title: row.get(19)?,
            sort_artist: row.get(20)?,
            year_missing: row.get(21)?,
            year_value: row.get(22)?,
            added_at: row.get(18)?,
            edition_id: row.get(0)?,
        },
    })
}

const ALBUM_COLUMNS: &str = "
    am.edition_id,am.source_kind,am.native_album_id,am.source_raw,
    am.title,am.artist,am.all_artists,am.year,am.track_count,am.total_duration_ms,
    am.quality_tier,am.format,am.bit_depth,am.sample_rate_hz,
    am.artwork_source,am.artwork_token,am.directory_path,am.folder_count,am.added_at,
    am.sort_title,am.sort_artist,
    CASE WHEN am.year IS NULL THEN 1 ELSE 0 END,COALESCE(am.year,0),
    COALESCE((
        SELECT group_concat(source_word,char(31))
          FROM (
              SELECT DISTINCT
                     CASE WHEN TRIM(t_sources.source_raw) != ''
                          THEN LOWER(TRIM(t_sources.source_raw))
                          ELSE sc_sources.source_kind END AS source_word,
                     CASE sc_sources.source_kind
                         WHEN 'local' THEN 0 WHEN 'offline' THEN 1
                         WHEN 'plex' THEN 2 WHEN 'jellyfin' THEN 3 ELSE 4 END
                         AS source_order
                FROM source_copies sc_sources INDEXED BY idx_source_copies_edition
                CROSS JOIN tracks t_sources INDEXED BY idx_tracks_source_copy
                  ON t_sources.source_copy_id=sc_sources.source_copy_id
               WHERE sc_sources.edition_id=am.edition_id
                 AND t_sources.available=1
               ORDER BY source_order,source_word
          )
    ),'')";

struct ArtistQueryParts {
    from_sql: String,
    where_sql: String,
    params: Vec<Value>,
    album_count_sql: String,
    track_count_sql: String,
    source_sql: String,
}

fn validate_artists_descriptor(descriptor: &QueryDescriptor) -> Result<()> {
    if descriptor.surface() != QuerySurface::Artists {
        return Err(CatalogError::InvalidInput(
            "a non-Artists descriptor was passed to an Artists query".to_string(),
        ));
    }
    if !descriptor.formats().is_empty()
        || descriptor.other_formats()
        || !descriptor.quality_tiers().is_empty()
        || !descriptor.source_buckets().is_empty()
        || descriptor.group() != TrackGroup::Off
    {
        return Err(CatalogError::InvalidInput(
            "Artists supports source instances and name search only".to_string(),
        ));
    }
    match descriptor.sort() {
        TrackSort::Default | TrackSort::ArtistAsc | TrackSort::ArtistDesc => Ok(()),
        _ => Err(CatalogError::InvalidInput(
            "Artists supports name ordering only".to_string(),
        )),
    }
}

fn artist_descending(descriptor: &QueryDescriptor) -> bool {
    descriptor.sort() == TrackSort::ArtistDesc
}

fn artist_initial_expression(alias: &str) -> String {
    format!(
        "CASE WHEN UPPER(SUBSTR({alias}.sort_name,1,1)) BETWEEN 'A' AND 'Z' \
              THEN UPPER(SUBSTR({alias}.sort_name,1,1)) ELSE '#' END"
    )
}

fn artist_filter_parts(
    descriptor: &QueryDescriptor,
    cursor: Option<&ArtistCursor>,
) -> Result<ArtistQueryParts> {
    if let Some(cursor) = cursor {
        if cursor.descriptor_key != descriptor_key(descriptor) {
            return Err(CatalogError::CursorDescriptorMismatch);
        }
    }
    let mut params = Vec::new();
    let (source_join, album_count_sql, track_count_sql, source_sql, source_filter) =
        if descriptor.sources().is_empty() {
            (
                "".to_string(),
                "ar.album_count".to_string(),
                "ar.track_count".to_string(),
                "ar.source_kind".to_string(),
                None,
            )
        } else {
            let mut source = Vec::new();
            for key in descriptor.sources() {
                if key.source_instance.trim().is_empty() {
                    return Err(CatalogError::InvalidInput(
                        "source filter instance must not be empty".to_string(),
                    ));
                }
                let first = params.len() + 1;
                source.push(format!(
                    "(ass.source_kind=?{first} AND ass.source_instance=?{})",
                    first + 1
                ));
                params.push(Value::Text(key.source.as_str().to_string()));
                params.push(Value::Text(key.source_instance.clone()));
            }
            let clause = source.join(" OR ");
            (
                "".to_string(),
                format!(
                    "(SELECT COALESCE(SUM(ass.album_count),0) FROM artist_source_stats ass \
                      WHERE ass.artist_key=ar.artist_key AND ass.available=1 AND ({clause}))"
                ),
                format!(
                    "(SELECT COALESCE(SUM(ass.track_count),0) FROM artist_source_stats ass \
                      WHERE ass.artist_key=ar.artist_key AND ass.available=1 AND ({clause}))"
                ),
                format!(
                    "(SELECT CASE WHEN COUNT(DISTINCT ass.source_kind)>1 THEN 'mixed' \
                                  ELSE MIN(ass.source_kind) END FROM artist_source_stats ass \
                      WHERE ass.artist_key=ar.artist_key AND ass.available=1 AND ({clause}))"
                ),
                Some(clause),
            )
        };

    let mut predicates = Vec::new();
    let mut search_join = String::new();
    if !descriptor.search().is_empty() {
        let search_parameter = params.len() + 1;
        if descriptor.search().chars().count() < 3 {
            predicates.push(format!("INSTR(ar.sort_name,?{search_parameter})>0"));
            params.push(Value::Text(normalize_artist_key(descriptor.search())));
        } else {
            search_join = "JOIN artists_fts ON artists_fts.artist_key=ar.artist_key".to_string();
            predicates.push(format!("artists_fts MATCH ?{search_parameter}"));
            params.push(fts_match_value(descriptor.search())?);
        }
    }
    if descriptor.available_only() {
        predicates.push("ar.available=1".to_string());
    }
    if let Some(source) = source_filter {
        predicates.push(format!(
            "EXISTS (SELECT 1 FROM artist_source_stats ass \
                     WHERE ass.artist_key=ar.artist_key AND ass.available=1 AND ({source}))"
        ));
    }
    if let Some(cursor) = cursor {
        let comparison = if artist_descending(descriptor) {
            "<"
        } else {
            ">"
        };
        predicates.push(format!(
            "(ar.sort_name{comparison}? OR (ar.sort_name=? AND ar.artist_key{comparison}?))"
        ));
        params.push(Value::Text(cursor.sort_name.clone()));
        params.push(Value::Text(cursor.sort_name.clone()));
        params.push(Value::Text(cursor.artist_key.clone()));
    }
    if predicates.is_empty() {
        predicates.push("1".to_string());
    }
    Ok(ArtistQueryParts {
        from_sql: format!("FROM artists_materialized ar {source_join} {search_join}"),
        where_sql: predicates.join(" AND "),
        params,
        album_count_sql,
        track_count_sql,
        source_sql,
    })
}

fn validate_artist_key(artist_key: &str) -> Result<()> {
    if artist_key.trim().is_empty() || normalize_artist_key(artist_key) != artist_key {
        return Err(CatalogError::InvalidInput(
            "artist relationship requires a normalized artist key".to_string(),
        ));
    }
    Ok(())
}

fn artist_relation_key(artist_key: &str, sources: &[SourceKey]) -> String {
    let mut sources = sources.to_vec();
    sources.sort();
    sources.dedup();
    let mut key = String::from("artist-albums|");
    push_key_part(&mut key, artist_key);
    for source in sources {
        push_key_part(&mut key, source.source.as_str());
        push_key_part(&mut key, &source.source_instance);
    }
    key
}

fn artist_album_filter(
    artist_key: &str,
    sources: &[SourceKey],
    cursor: Option<&AlbumCursor>,
) -> Result<(String, Vec<Value>)> {
    let relation_key = artist_relation_key(artist_key, sources);
    if let Some(cursor) = cursor {
        if cursor.descriptor_key != relation_key {
            return Err(CatalogError::CursorDescriptorMismatch);
        }
    }
    let mut predicates = vec![
        "am.available=1".to_string(),
        "EXISTS (SELECT 1 FROM edition_artists ea \
                 WHERE ea.edition_id=am.edition_id AND ea.artist_key=?)"
            .to_string(),
    ];
    let mut values = vec![Value::Text(artist_key.to_string())];
    if !sources.is_empty() {
        let mut source = Vec::new();
        for key in sources {
            if key.source_instance.trim().is_empty() {
                return Err(CatalogError::InvalidInput(
                    "source filter instance must not be empty".to_string(),
                ));
            }
            source.push("(scf.source_kind=? AND scf.source_instance=?)");
            values.push(Value::Text(key.source.as_str().to_string()));
            values.push(Value::Text(key.source_instance.clone()));
        }
        predicates.push(format!(
            "EXISTS (SELECT 1 FROM source_copies scf WHERE scf.edition_id=am.edition_id AND ({}))",
            source.join(" OR ")
        ));
    }
    if let Some(cursor) = cursor {
        predicates.push("(am.sort_title>? OR (am.sort_title=? AND am.edition_id>?))".to_string());
        values.push(Value::Text(cursor.sort_title.clone()));
        values.push(Value::Text(cursor.sort_title.clone()));
        values.push(Value::Integer(cursor.edition_id));
    }
    Ok((predicates.join(" AND "), values))
}

/// Unicode-aware display sort key shared by ingest and query indices.
pub fn normalize_sort_key(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Stable artist key: lowercased, common Latin diacritics folded, and runs of
/// punctuation collapsed while preserving non-Latin alphanumerics.
pub fn normalize_artist_key(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut pending_space = false;
    for character in value.to_lowercase().chars() {
        let folded = fold_diacritic(character);
        if folded.is_alphanumeric() {
            if pending_space && !out.is_empty() {
                out.push(' ');
            }
            out.push(folded);
            pending_space = false;
        } else {
            pending_space = true;
        }
    }
    out
}

fn fold_diacritic(character: char) -> char {
    match character {
        'á' | 'à' | 'â' | 'ä' | 'ã' | 'å' | 'ā' => 'a',
        'é' | 'è' | 'ê' | 'ë' | 'ē' | 'ė' => 'e',
        'í' | 'ì' | 'î' | 'ï' | 'ī' => 'i',
        'ó' | 'ò' | 'ô' | 'ö' | 'õ' | 'ō' | 'ø' => 'o',
        'ú' | 'ù' | 'û' | 'ü' | 'ū' => 'u',
        'ñ' => 'n',
        'ç' => 'c',
        'ý' | 'ÿ' => 'y',
        'ß' => 's',
        _ => character,
    }
}

pub(crate) fn descriptor_key(descriptor: &QueryDescriptor) -> String {
    let mut key = String::new();
    push_key_part(
        &mut key,
        match descriptor.surface() {
            crate::QuerySurface::Tracks => "tracks",
            crate::QuerySurface::Albums => "albums",
            crate::QuerySurface::Artists => "artists",
        },
    );
    push_key_part(&mut key, descriptor.search());
    push_key_part(
        &mut key,
        if descriptor.available_only() {
            "1"
        } else {
            "0"
        },
    );
    push_key_part(&mut key, sort_word(effective_sort(descriptor)));
    push_key_part(
        &mut key,
        match descriptor.group() {
            TrackGroup::Off => "off",
            TrackGroup::Album => "album",
            TrackGroup::Artist => "artist",
            TrackGroup::Name => "name",
        },
    );
    for source in descriptor.sources() {
        push_key_part(&mut key, source.source.as_str());
        push_key_part(&mut key, &source.source_instance);
    }
    for bucket in descriptor.source_buckets() {
        push_key_part(&mut key, bucket);
    }
    for format in descriptor.formats() {
        push_key_part(&mut key, format);
    }
    push_key_part(
        &mut key,
        if descriptor.other_formats() {
            "other"
        } else {
            ""
        },
    );
    for tier in descriptor.quality_tiers() {
        push_key_part(&mut key, tier);
    }
    key
}

fn push_key_part(key: &mut String, value: &str) {
    key.push_str(&value.len().to_string());
    key.push(':');
    key.push_str(value);
    key.push('|');
}

fn sort_word(sort: TrackSort) -> &'static str {
    match sort {
        TrackSort::Default => "default",
        TrackSort::TitleAsc => "title-asc",
        TrackSort::TitleDesc => "title-desc",
        TrackSort::ArtistAsc => "artist-asc",
        TrackSort::ArtistDesc => "artist-desc",
        TrackSort::YearAsc => "year-asc",
        TrackSort::YearDesc => "year-desc",
        TrackSort::AddedDesc => "added-desc",
    }
}

fn now_unix_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs().min(i64::MAX as u64) as i64)
        .unwrap_or(0)
}
