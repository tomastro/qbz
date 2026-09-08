//! Persistent catalogue identity for tracks inside a SACD image.
//!
//! A SACD is not a directory and an `.iso` extension is not proof that an
//! image is a SACD. Discovery therefore stays explicit: the frontend first
//! parses the Scarlet Book TOC, then hands the complete, validated generation
//! to this module. Failed or partial reads never call this API and can never
//! prune the last known-good rows.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use rusqlite::{params, OptionalExtension, Transaction, TransactionBehavior};

use crate::{AudioFormat, LibraryDatabase, LibraryError, LocalTrack};

/// One completely parsed SACD image generation.
#[derive(Debug, Clone)]
pub struct SacdImageImport {
    /// Stable geometry fingerprint supplied by the SACD parser.
    pub fingerprint: String,
    /// Native image path, without the `sacd:` virtual-track scheme.
    pub image_path: String,
    pub image_size_bytes: u64,
    pub image_modified_ns: i64,
    pub observed_at: i64,
    /// Scan-cache schema/parser generation. Not part of the fingerprint.
    pub parser_revision: i64,
    /// Complete stereo-area track set. Each path must be
    /// `sacd:<image_path>#<track_number>`.
    pub tracks: Vec<LocalTrack>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SacdImportResult {
    /// Authoritative `local_tracks.id`s in disc order.
    pub track_ids: Vec<i64>,
    pub inserted: usize,
    pub updated: usize,
    pub removed: usize,
    /// A newer complete generation was already committed for this disc.
    pub stale: bool,
}

impl LibraryDatabase {
    /// Mark a known image as seen by one registered library root, and report
    /// whether its file facts and parser revision still match. Unknown paths
    /// remain unowned until a complete parse is imported.
    pub fn observe_sacd_image(
        &self,
        root_id: i64,
        observed_generation: i64,
        image_path: &str,
        size_bytes: u64,
        modified_ns: i64,
        parser_revision: i64,
    ) -> Result<bool, LibraryError> {
        self.with_connection(|connection| {
            let transaction =
                Transaction::new_unchecked(connection, TransactionBehavior::Immediate)
                    .map_err(database_error)?;
            ensure_registered_root_owns_path(&transaction, root_id, image_path)?;
            let known = transaction
                .query_row(
                    "SELECT fingerprint,image_size_bytes,image_modified_ns,parser_revision
                       FROM local_sacd_images WHERE image_path=?1",
                    params![image_path],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, i64>(1)?,
                            row.get::<_, i64>(2)?,
                            row.get::<_, i64>(3)?,
                        ))
                    },
                )
                .optional()
                .map_err(database_error)?;
            let unchanged =
                if let Some((fingerprint, known_size, known_mtime, known_revision)) = known {
                    observe_root(&transaction, root_id, &fingerprint, observed_generation)?;
                    known_size == to_i64(size_bytes)
                        && known_mtime == modified_ns
                        && known_revision == parser_revision
                } else {
                    false
                };
            transaction.commit().map_err(database_error)?;
            Ok(unchanged)
        })
    }

    /// Atomically adopt a fully parsed SACD image found by a registered Local
    /// Library root. Requiring the root id and generation keeps this API out of
    /// the manual/ephemeral open path by construction.
    ///
    /// The geometry fingerprint, not the file path, owns the row ids. Moving
    /// an image and opening it again therefore preserves playlists/history
    /// references. A successful shorter generation prunes obsolete virtual
    /// tracks; validation or SQL failure rolls the whole operation back.
    pub fn import_sacd_image(
        &self,
        root_id: i64,
        observed_generation: i64,
        import: &SacdImageImport,
    ) -> Result<SacdImportResult, LibraryError> {
        let ordered = validate_import(import)?;
        self.with_connection(|connection| {
            // Reserve the writer before reading the row-id high-water mark;
            // two simultaneous explicit imports must serialize rather than
            // allocate the same ids from a shared snapshot.
            let transaction =
                Transaction::new_unchecked(connection, TransactionBehavior::Immediate)
                    .map_err(database_error)?;
            ensure_registered_root_owns_path(&transaction, root_id, &import.image_path)?;
            let latest_observation = transaction
                .query_row(
                    "SELECT MAX(observed_at) FROM local_sacd_images
                      WHERE fingerprint=?1 OR image_path=?2",
                    params![import.fingerprint, import.image_path],
                    |row| row.get::<_, Option<i64>>(0),
                )
                .map_err(database_error)?;
            if latest_observation.is_some_and(|latest| latest > import.observed_at) {
                let track_ids = {
                    let mut statement = transaction
                        .prepare(
                            "SELECT local_track_id FROM local_sacd_tracks
                              WHERE fingerprint=?1 ORDER BY track_number",
                        )
                        .map_err(database_error)?;
                    let rows = statement
                        .query_map(params![import.fingerprint], |row| row.get(0))
                        .map_err(database_error)?;
                    rows.collect::<Result<Vec<_>, _>>()
                        .map_err(database_error)?
                };
                let current_fingerprint = transaction
                    .query_row(
                        "SELECT fingerprint FROM local_sacd_images WHERE image_path=?1",
                        params![import.image_path],
                        |row| row.get::<_, String>(0),
                    )
                    .optional()
                    .map_err(database_error)?;
                if current_fingerprint.as_deref() == Some(import.fingerprint.as_str()) {
                    observe_root(
                        &transaction,
                        root_id,
                        &import.fingerprint,
                        observed_generation,
                    )?;
                    transaction.commit().map_err(database_error)?;
                } else {
                    transaction.rollback().map_err(database_error)?;
                }
                return Ok(SacdImportResult {
                    track_ids,
                    inserted: 0,
                    updated: 0,
                    removed: 0,
                    stale: true,
                });
            }
            // Allocate above the pre-transaction high-water mark. SQLite may
            // otherwise reuse the just-deleted maximum rowid when an image is
            // replaced, silently making a playlist entry for the old disc
            // point at a track on the new one.
            let mut next_id: i64 = transaction
                .query_row(
                    "SELECT COALESCE(MAX(id),0)+1 FROM local_tracks",
                    [],
                    |row| row.get(0),
                )
                .map_err(database_error)?;

            // A successfully parsed new disc at an already-known path means
            // the file was replaced. An absent/NAS-down image never reaches
            // this method, so it cannot enter this prune arm.
            let replaced: Vec<String> = {
                let mut statement = transaction
                    .prepare(
                        "SELECT fingerprint FROM local_sacd_images
                          WHERE image_path=?1 AND fingerprint<>?2",
                    )
                    .map_err(database_error)?;
                let rows = statement
                    .query_map(params![import.image_path, import.fingerprint], |row| {
                        row.get(0)
                    })
                    .map_err(database_error)?;
                rows.collect::<Result<Vec<_>, _>>()
                    .map_err(database_error)?
            };

            let mut removed = 0usize;
            for fingerprint in replaced {
                removed += remove_generation(&transaction, &fingerprint)?;
            }

            let mapped: HashMap<u32, i64> = {
                let mut statement = transaction
                    .prepare(
                        "SELECT track_number,local_track_id FROM local_sacd_tracks
                          WHERE fingerprint=?1",
                    )
                    .map_err(database_error)?;
                let rows = statement
                    .query_map(params![import.fingerprint], |row| {
                        Ok((row.get::<_, u32>(0)?, row.get::<_, i64>(1)?))
                    })
                    .map_err(database_error)?;
                rows.collect::<Result<HashMap<_, _>, _>>()
                    .map_err(database_error)?
            };

            let incoming_numbers = ordered
                .iter()
                .map(|(number, _)| *number)
                .collect::<HashSet<_>>();
            for (number, id) in &mapped {
                if !incoming_numbers.contains(number) {
                    transaction
                        .execute(
                            "DELETE FROM local_sacd_tracks
                              WHERE fingerprint=?1 AND track_number=?2",
                            params![import.fingerprint, number],
                        )
                        .map_err(database_error)?;
                    removed += transaction
                        .execute("DELETE FROM local_tracks WHERE id=?1", params![id])
                        .map_err(database_error)?;
                }
            }

            let mut result = SacdImportResult {
                track_ids: Vec::with_capacity(ordered.len()),
                inserted: 0,
                updated: 0,
                removed,
                stale: false,
            };

            for (number, track) in ordered {
                let existing = match mapped.get(&number).copied() {
                    Some(id) => Some(id),
                    None => transaction
                        .query_row(
                            "SELECT id FROM local_tracks
                              WHERE file_path=?1 AND cue_file_path IS NULL",
                            params![track.file_path],
                            |row| row.get(0),
                        )
                        .optional()
                        .map_err(database_error)?,
                };

                let id = match existing {
                    Some(id) => {
                        update_track(&transaction, id, track)?;
                        result.updated += 1;
                        id
                    }
                    None => {
                        let id = insert_track(&transaction, next_id, track)?;
                        next_id = next_id.checked_add(1).ok_or_else(|| {
                            LibraryError::Database("local track id space exhausted".to_string())
                        })?;
                        result.inserted += 1;
                        id
                    }
                };
                transaction
                    .execute(
                        "INSERT INTO local_sacd_tracks
                             (fingerprint,track_number,local_track_id)
                         VALUES (?1,?2,?3)
                         ON CONFLICT(fingerprint,track_number) DO UPDATE SET
                             local_track_id=excluded.local_track_id",
                        params![import.fingerprint, number, id],
                    )
                    .map_err(database_error)?;
                result.track_ids.push(id);
            }

            transaction
                .execute(
                    "INSERT INTO local_sacd_images
                         (fingerprint,image_path,image_size_bytes,image_modified_ns,observed_at,parser_revision)
                     VALUES (?1,?2,?3,?4,?5,?6)
                     ON CONFLICT(fingerprint) DO UPDATE SET
                         image_path=excluded.image_path,
                         image_size_bytes=excluded.image_size_bytes,
                         image_modified_ns=excluded.image_modified_ns,
                         observed_at=excluded.observed_at,
                         parser_revision=excluded.parser_revision",
                    params![
                        import.fingerprint,
                        import.image_path,
                        to_i64(import.image_size_bytes),
                        import.image_modified_ns,
                        import.observed_at,
                        import.parser_revision,
                    ],
                )
                .map_err(database_error)?;

            observe_root(
                &transaction,
                root_id,
                &import.fingerprint,
                observed_generation,
            )?;

            transaction.commit().map_err(database_error)?;
            Ok(result)
        })
    }

    /// Backfill root ownership for pre-fix indexed rows and remove generations
    /// that are outside every configured Local Library folder. This is
    /// intentionally idempotent: `LibraryDatabase::open` may run many times in
    /// one Qt startup.
    pub(crate) fn reconcile_sacd_catalog_scope(&self) -> Result<(), LibraryError> {
        let (linked, unlinked, removed_images, removed_tracks) = self.with_connection(
            |connection| -> Result<(usize, usize, usize, usize), LibraryError> {
                let transaction =
                    Transaction::new_unchecked(connection, TransactionBehavior::Immediate)
                        .map_err(database_error)?;
                let roots: Vec<(i64, String)> = {
                    let mut statement = transaction
                        .prepare("SELECT id,path FROM library_folders")
                        .map_err(database_error)?;
                    let rows = statement
                        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
                        .map_err(database_error)?
                        .collect::<Result<Vec<_>, _>>()
                        .map_err(database_error)?;
                    rows
                };
                let images: Vec<(String, String)> = {
                    let mut statement = transaction
                        .prepare("SELECT fingerprint,image_path FROM local_sacd_images")
                        .map_err(database_error)?;
                    let rows = statement
                        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
                        .map_err(database_error)?
                        .collect::<Result<Vec<_>, _>>()
                        .map_err(database_error)?;
                    rows
                };

                let mut linked = 0usize;
                for (fingerprint, image_path) in &images {
                    for (root_id, root_path) in &roots {
                        if path_is_within(image_path, root_path) {
                            linked += transaction
                                .execute(
                                    "INSERT OR IGNORE INTO local_sacd_roots
                                         (root_id,fingerprint,observed_generation)
                                     VALUES (?1,?2,0)",
                                    params![root_id, fingerprint],
                                )
                                .map_err(database_error)?;
                        }
                    }
                }

                let mappings: Vec<(i64, String, Option<String>, Option<String>)> = {
                    let mut statement = transaction
                        .prepare(
                            "SELECT owned.root_id,owned.fingerprint,folders.path,images.image_path
                               FROM local_sacd_roots owned
                               LEFT JOIN library_folders folders ON folders.id=owned.root_id
                               LEFT JOIN local_sacd_images images
                                      ON images.fingerprint=owned.fingerprint",
                        )
                        .map_err(database_error)?;
                    let rows = statement
                        .query_map([], |row| {
                            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
                        })
                        .map_err(database_error)?
                        .collect::<Result<Vec<_>, _>>()
                        .map_err(database_error)?;
                    rows
                };
                let mut unlinked = 0usize;
                for (root_id, fingerprint, root_path, image_path) in mappings {
                    let valid = root_path
                        .as_deref()
                        .zip(image_path.as_deref())
                        .is_some_and(|(root, image)| path_is_within(image, root));
                    if !valid {
                        unlinked += transaction
                            .execute(
                                "DELETE FROM local_sacd_roots
                                  WHERE root_id=?1 AND fingerprint=?2",
                                params![root_id, fingerprint],
                            )
                            .map_err(database_error)?;
                    }
                }

                let orphaned: Vec<String> = {
                    let mut statement = transaction
                        .prepare(
                            "SELECT images.fingerprint FROM local_sacd_images images
                              WHERE NOT EXISTS (
                                  SELECT 1 FROM local_sacd_roots owned
                                   WHERE owned.fingerprint=images.fingerprint
                              )",
                        )
                        .map_err(database_error)?;
                    let rows = statement
                        .query_map([], |row| row.get(0))
                        .map_err(database_error)?
                        .collect::<Result<Vec<_>, _>>()
                        .map_err(database_error)?;
                    rows
                };
                let mut removed_tracks = 0usize;
                for fingerprint in &orphaned {
                    removed_tracks += remove_generation(&transaction, fingerprint)?;
                }
                let removed_images = orphaned.len();
                transaction.commit().map_err(database_error)?;
                Ok((linked, unlinked, removed_images, removed_tracks))
            },
        )?;
        if linked > 0 || unlinked > 0 || removed_images > 0 {
            log::info!(
                "[sacd] catalogue scope reconciled: linked={linked} unlinked={unlinked} removed_images={removed_images} removed_tracks={removed_tracks}"
            );
        }
        Ok(())
    }

    /// Drop one root's ownership and remove only generations that no other
    /// registered root owns.
    pub(crate) fn remove_sacd_root(&self, root_id: i64) -> Result<usize, LibraryError> {
        self.with_connection(|connection| {
            let transaction =
                Transaction::new_unchecked(connection, TransactionBehavior::Immediate)
                    .map_err(database_error)?;
            let removed = remove_root_ownership(&transaction, root_id, None)?;
            transaction.commit().map_err(database_error)?;
            Ok(removed)
        })
    }

    /// Complete one successful SACD walk: stale observations under this root
    /// disappear, while overlapping roots retain their shared generations.
    pub(crate) fn finish_sacd_root_scan(
        &self,
        root_id: i64,
        observed_generation: i64,
    ) -> Result<usize, LibraryError> {
        self.with_connection(|connection| {
            let transaction =
                Transaction::new_unchecked(connection, TransactionBehavior::Immediate)
                    .map_err(database_error)?;
            let removed = remove_root_ownership(&transaction, root_id, Some(observed_generation))?;
            transaction.commit().map_err(database_error)?;
            Ok(removed)
        })
    }

    /// A known SACD path that now has a valid non-SACD signature was replaced;
    /// remove its complete generation and every ownership edge.
    pub(crate) fn remove_sacd_image_at_path(
        &self,
        image_path: &str,
    ) -> Result<usize, LibraryError> {
        self.with_connection(|connection| {
            let transaction =
                Transaction::new_unchecked(connection, TransactionBehavior::Immediate)
                    .map_err(database_error)?;
            let fingerprint = transaction
                .query_row(
                    "SELECT fingerprint FROM local_sacd_images WHERE image_path=?1",
                    params![image_path],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(database_error)?;
            let removed = match fingerprint {
                Some(fingerprint) => remove_generation(&transaction, &fingerprint)?,
                None => 0,
            };
            transaction.commit().map_err(database_error)?;
            Ok(removed)
        })
    }
}

fn ensure_registered_root_owns_path(
    transaction: &Transaction<'_>,
    root_id: i64,
    image_path: &str,
) -> Result<(), LibraryError> {
    let root_path = transaction
        .query_row(
            "SELECT path FROM library_folders WHERE id=?1",
            params![root_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(database_error)?
        .ok_or_else(|| LibraryError::InvalidPath("SACD scan root is not registered".to_string()))?;
    if !path_is_within(image_path, &root_path) {
        return Err(LibraryError::InvalidPath(format!(
            "SACD image is outside its registered scan root: {image_path}"
        )));
    }
    Ok(())
}

fn observe_root(
    transaction: &Transaction<'_>,
    root_id: i64,
    fingerprint: &str,
    observed_generation: i64,
) -> Result<(), LibraryError> {
    transaction
        .execute(
            "INSERT INTO local_sacd_roots(root_id,fingerprint,observed_generation)
             VALUES (?1,?2,?3)
             ON CONFLICT(root_id,fingerprint) DO UPDATE SET
                 observed_generation=excluded.observed_generation",
            params![root_id, fingerprint, observed_generation],
        )
        .map_err(database_error)?;
    Ok(())
}

fn remove_root_ownership(
    transaction: &Transaction<'_>,
    root_id: i64,
    keep_generation: Option<i64>,
) -> Result<usize, LibraryError> {
    let fingerprints: Vec<String> = if let Some(generation) = keep_generation {
        let mut statement = transaction
            .prepare(
                "SELECT fingerprint FROM local_sacd_roots
                  WHERE root_id=?1 AND observed_generation<>?2",
            )
            .map_err(database_error)?;
        let rows = statement
            .query_map(params![root_id, generation], |row| row.get(0))
            .map_err(database_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(database_error)?;
        rows
    } else {
        let mut statement = transaction
            .prepare("SELECT fingerprint FROM local_sacd_roots WHERE root_id=?1")
            .map_err(database_error)?;
        let rows = statement
            .query_map(params![root_id], |row| row.get(0))
            .map_err(database_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(database_error)?;
        rows
    };
    match keep_generation {
        Some(generation) => transaction
            .execute(
                "DELETE FROM local_sacd_roots
                  WHERE root_id=?1 AND observed_generation<>?2",
                params![root_id, generation],
            )
            .map_err(database_error)?,
        None => transaction
            .execute(
                "DELETE FROM local_sacd_roots WHERE root_id=?1",
                params![root_id],
            )
            .map_err(database_error)?,
    };

    let mut removed = 0usize;
    for fingerprint in fingerprints {
        let still_owned = transaction
            .query_row(
                "SELECT 1 FROM local_sacd_roots WHERE fingerprint=?1 LIMIT 1",
                params![fingerprint],
                |_| Ok(()),
            )
            .optional()
            .map_err(database_error)?
            .is_some();
        if !still_owned {
            removed += remove_generation(transaction, &fingerprint)?;
        }
    }
    Ok(removed)
}

fn path_is_within(path: &str, root: &str) -> bool {
    !root.is_empty() && Path::new(path).starts_with(Path::new(root))
}

fn validate_import(import: &SacdImageImport) -> Result<Vec<(u32, &LocalTrack)>, LibraryError> {
    if import.fingerprint.trim().is_empty() {
        return Err(LibraryError::Other("SACD fingerprint is empty".to_string()));
    }
    if import.image_path.is_empty() {
        return Err(LibraryError::InvalidPath(
            "SACD image path is empty".to_string(),
        ));
    }
    if import.tracks.is_empty() {
        return Err(LibraryError::Other(
            "SACD generation has no tracks".to_string(),
        ));
    }

    let mut seen = HashSet::with_capacity(import.tracks.len());
    let mut ordered = Vec::with_capacity(import.tracks.len());
    for track in &import.tracks {
        let number = track
            .track_number
            .filter(|number| (1..=255).contains(number))
            .ok_or_else(|| LibraryError::Other("SACD track number is invalid".to_string()))?;
        if !seen.insert(number) {
            return Err(LibraryError::Other(format!(
                "SACD generation repeats track {number}"
            )));
        }
        if track.format != AudioFormat::Dsd {
            return Err(LibraryError::Other(format!(
                "SACD track {number} is not DSD"
            )));
        }
        let (path, path_number) = parse_virtual_path(&track.file_path).ok_or_else(|| {
            LibraryError::InvalidPath(format!("invalid SACD virtual track {number}"))
        })?;
        if path != import.image_path || path_number != number {
            return Err(LibraryError::InvalidPath(format!(
                "SACD virtual track {number} does not match its image"
            )));
        }
        ordered.push((number, track));
    }
    ordered.sort_by_key(|(number, _)| *number);
    Ok(ordered)
}

fn parse_virtual_path(value: &str) -> Option<(&str, u32)> {
    let rest = value.strip_prefix("sacd:")?;
    let (path, number) = rest.rsplit_once('#')?;
    (!path.is_empty()).then_some((path, number.parse().ok()?))
}

fn remove_generation(
    transaction: &Transaction<'_>,
    fingerprint: &str,
) -> Result<usize, LibraryError> {
    let removed = transaction
        .execute(
            "DELETE FROM local_tracks WHERE id IN (
                 SELECT local_track_id FROM local_sacd_tracks WHERE fingerprint=?1
             )",
            params![fingerprint],
        )
        .map_err(database_error)?;
    transaction
        .execute(
            "DELETE FROM local_sacd_tracks WHERE fingerprint=?1",
            params![fingerprint],
        )
        .map_err(database_error)?;
    transaction
        .execute(
            "DELETE FROM local_sacd_roots WHERE fingerprint=?1",
            params![fingerprint],
        )
        .map_err(database_error)?;
    transaction
        .execute(
            "DELETE FROM local_sacd_images WHERE fingerprint=?1",
            params![fingerprint],
        )
        .map_err(database_error)?;
    Ok(removed)
}

fn update_track(
    transaction: &Transaction<'_>,
    id: i64,
    track: &LocalTrack,
) -> Result<(), LibraryError> {
    let format = track.format.to_string();
    let network = i64::from(track.is_network_mount);
    let changed = transaction
        .execute(
            r#"UPDATE local_tracks SET
                file_path=?1,title=?2,artist=?3,album=?4,album_artist=?5,
                track_number=?6,disc_number=?7,year=?8,genre=?9,catalog_number=?10,
                duration_secs=?11,format=?12,bit_depth=?13,sample_rate=?14,
                channels=?15,file_size_bytes=?16,cue_file_path=NULL,
                cue_start_secs=NULL,cue_end_secs=NULL,artwork_path=?17,
                last_modified=?18,indexed_at=?19,album_group_key=?20,
                album_group_title=?21,source='user',qobuz_track_id=NULL,
                is_network_mount=?22
               WHERE id=?23"#,
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
                track.catalog_number,
                track.duration_secs as i64,
                format,
                track.bit_depth,
                track.sample_rate,
                track.channels,
                track.file_size_bytes as i64,
                track.artwork_path,
                track.last_modified,
                track.indexed_at,
                track.album_group_key,
                track.album_group_title,
                network,
                id,
            ],
        )
        .map_err(database_error)?;
    if changed != 1 {
        return Err(LibraryError::Database(format!(
            "SACD mapped track {id} is missing"
        )));
    }
    Ok(())
}

fn insert_track(
    transaction: &Transaction<'_>,
    id: i64,
    track: &LocalTrack,
) -> Result<i64, LibraryError> {
    let format = track.format.to_string();
    let network = i64::from(track.is_network_mount);
    transaction
        .execute(
            r#"INSERT INTO local_tracks
               (id,file_path,title,artist,album,album_artist,track_number,disc_number,
                year,genre,catalog_number,duration_secs,format,bit_depth,sample_rate,
                channels,file_size_bytes,cue_file_path,cue_start_secs,cue_end_secs,
                artwork_path,last_modified,indexed_at,album_group_key,album_group_title,
                source,qobuz_track_id,is_network_mount)
               VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,
                       ?16,?17,NULL,NULL,NULL,?18,?19,?20,?21,?22,'user',NULL,?23)"#,
            params![
                id,
                track.file_path,
                track.title,
                track.artist,
                track.album,
                track.album_artist,
                track.track_number,
                track.disc_number,
                track.year,
                track.genre,
                track.catalog_number,
                track.duration_secs as i64,
                format,
                track.bit_depth,
                track.sample_rate,
                track.channels,
                track.file_size_bytes as i64,
                track.artwork_path,
                track.last_modified,
                track.indexed_at,
                track.album_group_key,
                track.album_group_title,
                network,
            ],
        )
        .map_err(database_error)?;
    Ok(id)
}

fn to_i64(value: u64) -> i64 {
    value.min(i64::MAX as u64) as i64
}

fn database_error(error: rusqlite::Error) -> LibraryError {
    LibraryError::Database(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn fresh_db() -> (TempDir, LibraryDatabase, i64) {
        let temp = TempDir::new().unwrap();
        let db = LibraryDatabase::open(&temp.path().join("library.db")).unwrap();
        let root_id = db
            .add_folder_with_network_info("/music", false, None)
            .unwrap();
        (temp, db, root_id)
    }

    fn image(fingerprint: &str, path: &str, count: u32) -> SacdImageImport {
        SacdImageImport {
            fingerprint: fingerprint.to_string(),
            image_path: path.to_string(),
            image_size_bytes: 4_700_000_000,
            image_modified_ns: 123,
            observed_at: 456,
            parser_revision: 2,
            tracks: (1..=count)
                .map(|number| LocalTrack {
                    file_path: format!("sacd:{path}#{number}"),
                    title: format!("Track {number}"),
                    artist: "Artist".to_string(),
                    album: "Album".to_string(),
                    album_artist: Some("Artist".to_string()),
                    album_group_key: "sacd|||Album".to_string(),
                    album_group_title: "Album".to_string(),
                    track_number: Some(number),
                    disc_number: Some(1),
                    duration_secs: 180,
                    format: AudioFormat::Dsd,
                    bit_depth: Some(1),
                    sample_rate: 2_822_400.0,
                    channels: 2,
                    indexed_at: 456,
                    ..Default::default()
                })
                .collect(),
        }
    }

    fn seed_unowned_generation(db: &LibraryDatabase, import: &SacdImageImport) -> Vec<i64> {
        let ids = import
            .tracks
            .iter()
            .map(|track| db.insert_track(track).unwrap())
            .collect::<Vec<_>>();
        db.with_connection(|connection| {
            connection
                .execute(
                    "INSERT INTO local_sacd_images
                         (fingerprint,image_path,image_size_bytes,image_modified_ns,
                          observed_at,parser_revision)
                     VALUES (?1,?2,?3,?4,?5,?6)",
                    params![
                        import.fingerprint,
                        import.image_path,
                        to_i64(import.image_size_bytes),
                        import.image_modified_ns,
                        import.observed_at,
                        import.parser_revision,
                    ],
                )
                .unwrap();
            for (track, id) in import.tracks.iter().zip(&ids) {
                connection
                    .execute(
                        "INSERT INTO local_sacd_tracks
                             (fingerprint,track_number,local_track_id)
                         VALUES (?1,?2,?3)",
                        params![import.fingerprint, track.track_number, id],
                    )
                    .unwrap();
            }
        });
        ids
    }

    #[test]
    fn catalogue_import_requires_a_registered_owning_root() {
        let (_temp, db, root_id) = fresh_db();
        assert!(db
            .import_sacd_image(9999, 1, &image("unregistered", "/music/disc.iso", 1))
            .is_err());
        assert!(db
            .import_sacd_image(root_id, 1, &image("sibling", "/music-2/disc.iso", 1))
            .is_err());
        let rows: i64 = db
            .with_connection(|connection| {
                connection.query_row("SELECT COUNT(*) FROM local_sacd_images", [], |row| {
                    row.get(0)
                })
            })
            .unwrap();
        assert_eq!(rows, 0);
    }

    #[test]
    fn scope_reconciliation_adopts_scanned_legacy_rows_and_purges_manual_ones() {
        let (_temp, db, root_id) = fresh_db();
        let inside = image("inside", "/music/album/disc.iso", 2);
        let outside = image("outside", "/downloads/opened.iso", 3);
        let inside_ids = seed_unowned_generation(&db, &inside);
        let outside_ids = seed_unowned_generation(&db, &outside);

        db.reconcile_sacd_catalog_scope().unwrap();

        assert!(inside_ids
            .iter()
            .all(|id| db.get_track(*id).unwrap().is_some()));
        assert!(outside_ids
            .iter()
            .all(|id| db.get_track(*id).unwrap().is_none()));
        let state: (i64, i64, i64) = db
            .with_connection(|connection| {
                connection.query_row(
                    "SELECT
                         (SELECT COUNT(*) FROM local_sacd_images),
                         (SELECT COUNT(*) FROM local_sacd_roots WHERE root_id=?1),
                         (SELECT COUNT(*) FROM local_sacd_tracks)",
                    params![root_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
            })
            .unwrap();
        assert_eq!(state, (1, 1, 2));
    }

    #[test]
    fn removing_roots_respects_overlap_and_last_owner_removes_the_disc() {
        let (_temp, db, outer_root) = fresh_db();
        let inner_root = db
            .add_folder_with_network_info("/music/album", false, None)
            .unwrap();
        let import = image("shared", "/music/album/disc.iso", 2);
        let ids = db
            .import_sacd_image(outer_root, 1, &import)
            .unwrap()
            .track_ids;

        db.remove_folder("/music").unwrap();
        assert!(ids.iter().all(|id| db.get_track(*id).unwrap().is_some()));

        db.remove_folder("/music/album").unwrap();
        assert!(ids.iter().all(|id| db.get_track(*id).unwrap().is_none()));
        assert_ne!(inner_root, outer_root);
    }

    #[test]
    fn a_complete_root_generation_prunes_a_disappeared_image() {
        let (_temp, db, root_id) = fresh_db();
        let ids = db
            .import_sacd_image(root_id, 1, &image("gone", "/music/gone.iso", 2))
            .unwrap()
            .track_ids;

        assert_eq!(db.finish_sacd_root_scan(root_id, 2).unwrap(), 2);
        assert!(ids.iter().all(|id| db.get_track(*id).unwrap().is_none()));
    }

    #[test]
    fn import_is_persistent_and_reimporting_a_moved_image_keeps_row_ids() {
        let (_temp, db, root_id) = fresh_db();
        let first = db
            .import_sacd_image(root_id, 1, &image("sacd-a", "/music/old.iso", 3))
            .unwrap();
        assert_eq!((first.inserted, first.updated, first.removed), (3, 0, 0));
        db.add_local_track_to_playlist(7, first.track_ids[1], 0)
            .unwrap();

        let mut moved = image("sacd-a", "/nas/new.iso", 3);
        moved.tracks[1].title = "Corrected".to_string();
        let moved_root = db
            .add_folder_with_network_info("/nas", true, Some("nfs"))
            .unwrap();
        let second = db.import_sacd_image(moved_root, 2, &moved).unwrap();
        assert_eq!((second.inserted, second.updated, second.removed), (0, 3, 0));
        assert_eq!(second.track_ids, first.track_ids);
        assert_eq!(
            db.get_track(first.track_ids[1]).unwrap().unwrap().title,
            "Corrected"
        );
        assert_eq!(
            db.get_track(first.track_ids[1]).unwrap().unwrap().file_path,
            "sacd:/nas/new.iso#2"
        );
        let playlist_title: String = db
            .with_connection(|connection| {
                connection.query_row(
                    "SELECT t.title FROM playlist_local_tracks p
                       JOIN local_tracks t ON t.id=p.local_track_id
                      WHERE p.qobuz_playlist_id=7",
                    [],
                    |row| row.get(0),
                )
            })
            .unwrap();
        assert_eq!(playlist_title, "Corrected");
        let source: String = db
            .with_connection(|connection| {
                connection.query_row(
                    "SELECT source FROM local_tracks WHERE id=?1",
                    params![first.track_ids[0]],
                    |row| row.get(0),
                )
            })
            .unwrap();
        assert_eq!(source, "user", "the existing local router must own the row");
    }

    #[test]
    fn a_parser_revision_invalidates_only_the_scan_cache() {
        let (_temp, db, root_id) = fresh_db();
        let import = image("sacd-a", "/music/disc.iso", 2);
        let ids = db.import_sacd_image(root_id, 1, &import).unwrap().track_ids;

        assert!(db
            .observe_sacd_image(
                root_id,
                2,
                &import.image_path,
                import.image_size_bytes,
                import.image_modified_ns,
                import.parser_revision,
            )
            .unwrap());
        assert!(!db
            .observe_sacd_image(
                root_id,
                3,
                &import.image_path,
                import.image_size_bytes,
                import.image_modified_ns,
                import.parser_revision + 1,
            )
            .unwrap());

        let mut reparsed = import;
        reparsed.parser_revision += 1;
        reparsed.observed_at += 1;
        assert_eq!(
            db.import_sacd_image(root_id, 4, &reparsed)
                .unwrap()
                .track_ids,
            ids
        );
    }

    #[test]
    fn only_a_complete_successful_generation_prunes_obsolete_tracks() {
        let (_temp, db, root_id) = fresh_db();
        let first = db
            .import_sacd_image(root_id, 1, &image("sacd-a", "/music/disc.iso", 4))
            .unwrap();

        let mut invalid = image("sacd-a", "/music/disc.iso", 2);
        invalid.tracks[1].file_path = "sacd:/different.iso#2".to_string();
        assert!(db.import_sacd_image(root_id, 2, &invalid).is_err());
        assert!(db.get_track(first.track_ids[3]).unwrap().is_some());

        let shorter = db
            .import_sacd_image(root_id, 3, &image("sacd-a", "/music/disc.iso", 2))
            .unwrap();
        assert_eq!(shorter.removed, 2);
        assert_eq!(shorter.track_ids, first.track_ids[..2]);
        assert!(db.get_track(first.track_ids[2]).unwrap().is_none());
        assert!(db.get_track(first.track_ids[3]).unwrap().is_none());
    }

    #[test]
    fn an_older_completed_snapshot_cannot_overwrite_a_newer_correction() {
        let (_temp, db, root_id) = fresh_db();
        let mut newer = image("sacd-a", "/music/disc.iso", 2);
        newer.observed_at = 200;
        newer.tracks[0].title = "New naming".to_string();
        db.import_sacd_image(root_id, 2, &newer).unwrap();

        let mut older = image("sacd-a", "/music/disc.iso", 2);
        older.observed_at = 100;
        older.tracks[0].title = "Late old naming".to_string();
        let outcome = db.import_sacd_image(root_id, 1, &older).unwrap();

        assert!(outcome.stale);
        assert_eq!(
            (outcome.inserted, outcome.updated, outcome.removed),
            (0, 0, 0)
        );
        assert_eq!(
            db.get_track(outcome.track_ids[0]).unwrap().unwrap().title,
            "New naming"
        );
    }

    #[test]
    fn a_successfully_parsed_replacement_at_the_same_path_replaces_the_old_disc() {
        let (_temp, db, root_id) = fresh_db();
        let old = db
            .import_sacd_image(root_id, 1, &image("sacd-old", "/music/disc.iso", 2))
            .unwrap();
        let new = db
            .import_sacd_image(root_id, 2, &image("sacd-new", "/music/disc.iso", 3))
            .unwrap();

        assert_eq!(new.removed, 2);
        assert!(
            new.track_ids.iter().all(|id| *id > old.track_ids[1]),
            "a new disc must never inherit a removed disc's row ids"
        );
        for id in old.track_ids {
            assert!(db.get_track(id).unwrap().is_none());
        }
        let old_images: i64 = db
            .with_connection(|connection| {
                connection.query_row(
                    "SELECT COUNT(*) FROM local_sacd_images WHERE fingerprint='sacd-old'",
                    [],
                    |row| row.get(0),
                )
            })
            .unwrap();
        assert_eq!(old_images, 0);
    }

    #[test]
    fn clear_library_removes_tracks_and_sacd_identity_together() {
        let (_temp, db, root_id) = fresh_db();
        db.import_sacd_image(root_id, 1, &image("sacd-a", "/music/disc.iso", 2))
            .unwrap();
        db.clear_all_tracks().unwrap();

        let counts: (i64, i64, i64, i64) = db
            .with_connection(|connection| {
                connection.query_row(
                    "SELECT
                         (SELECT COUNT(*) FROM local_tracks),
                         (SELECT COUNT(*) FROM local_sacd_images),
                         (SELECT COUNT(*) FROM local_sacd_tracks),
                         (SELECT COUNT(*) FROM local_sacd_roots)",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
            })
            .unwrap();
        assert_eq!(counts, (0, 0, 0, 0));
    }

    #[test]
    fn the_full_sacd_track_number_domain_imports_without_a_hidden_cap() {
        let (_temp, db, root_id) = fresh_db();
        let started = std::time::Instant::now();
        let result = db
            .import_sacd_image(root_id, 1, &image("sacd-max", "/music/max.iso", 255))
            .unwrap();
        let sqlite_bytes: i64 = db
            .with_connection(|connection| {
                connection.query_row(
                    "SELECT
                         (SELECT page_size FROM pragma_page_size) *
                         (SELECT page_count FROM pragma_page_count)",
                    [],
                    |row| row.get(0),
                )
            })
            .unwrap();
        eprintln!(
            "sacd_import_metric tracks=255 sqlite_bytes={sqlite_bytes} elapsed_us={}",
            started.elapsed().as_micros()
        );
        assert_eq!(result.inserted, 255);
        assert_eq!(result.track_ids.len(), 255);
        assert_eq!(
            db.get_track(result.track_ids[254])
                .unwrap()
                .unwrap()
                .track_number,
            Some(255)
        );
    }
}
