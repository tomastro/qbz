//! Legacy cached file migration service
//!
//! Handles migration of old numeric-named FLAC files to new organized structure

use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};

use crate::metadata::{
    embed_artwork, fetch_complete_metadata, organize_cached_file, save_album_artwork,
    write_flac_tags,
};
use qbz_library::LibraryDatabase;
use qbz_qobuz::QobuzClient;

#[derive(Default, Serialize, Clone, Debug)]
pub struct MigrationStatus {
    pub has_legacy_files: bool,
    pub total_tracks: usize,
    pub processed: usize,
    pub successful: usize,
    pub failed: usize,
    pub in_progress: bool,
    pub completed: bool,
    pub errors: Vec<MigrationError>,
}

#[derive(Serialize, Clone, Debug)]
pub struct MigrationError {
    pub track_id: u64,
    pub error_message: String,
}

/// Detect legacy cached files (numeric FLAC files in tracks/ folder)
pub fn detect_legacy_cached_files(tracks_dir: &Path) -> Result<Vec<u64>, String> {
    log::info!(
        "Scanning for legacy cached files in: {}",
        tracks_dir.display()
    );

    if !tracks_dir.exists() {
        return Ok(Vec::new());
    }

    let mut track_ids = Vec::new();

    match std::fs::read_dir(tracks_dir) {
        Ok(entries) => {
            for entry in entries.flatten() {
                let path = entry.path();

                // Only process FLAC files
                if path.extension().and_then(|s| s.to_str()) != Some("flac") {
                    continue;
                }

                // Check if filename is purely numeric (track_id)
                if let Some(filename) = path.file_stem().and_then(|s| s.to_str()) {
                    if let Ok(track_id) = filename.parse::<u64>() {
                        track_ids.push(track_id);
                        log::debug!("Found legacy track: {}", track_id);
                    }
                }
            }
        }
        Err(e) => {
            return Err(format!("Failed to read tracks directory: {}", e));
        }
    }

    log::info!("Found {} legacy cached files", track_ids.len());
    Ok(track_ids)
}

/// Migrate a single legacy cached track
async fn migrate_single_track(
    track_id: u64,
    legacy_path: PathBuf,
    offline_root: &str,
    qobuz_client: &QobuzClient,
    library_db: Arc<Mutex<Option<LibraryDatabase>>>,
) -> Result<String, String> {
    log::info!("Migrating track {}", track_id);

    // 1. Fetch complete metadata from Qobuz
    let metadata = fetch_complete_metadata(track_id, qobuz_client).await?;

    // 2. Write FLAC tags
    let legacy_path_str = legacy_path.to_string_lossy().to_string();
    write_flac_tags(&legacy_path_str, &metadata)
        .map_err(|e| format!("Failed to write tags: {}", e))?;

    // 3. Embed artwork if available
    if let Some(artwork_url) = &metadata.artwork_url {
        if let Err(e) = embed_artwork(&legacy_path_str, artwork_url).await {
            log::warn!("Failed to embed artwork for track {}: {}", track_id, e);
        }
    }

    // 4. Organize file into artist/album structure
    let new_path = organize_cached_file(track_id, &legacy_path_str, offline_root, &metadata)?;

    // 5. Save album artwork as cover.jpg
    let artwork_path = if let Some(artwork_url) = &metadata.artwork_url {
        if let Some(parent_dir) = std::path::Path::new(&new_path).parent() {
            match save_album_artwork(parent_dir, artwork_url).await {
                Ok(_) => Some(parent_dir.join("cover.jpg").to_string_lossy().to_string()),
                Err(e) => {
                    log::warn!("Failed to save album artwork for track {}: {}", track_id, e);
                    None
                }
            }
        } else {
            None
        }
    } else {
        None
    };

    // 6. Extract audio properties from FLAC file
    use lofty::prelude::*;
    let (bit_depth, sample_rate) = match lofty::read_from_path(&new_path) {
        Ok(tagged_file) => {
            let properties = tagged_file.properties();
            let bit_depth = properties.bit_depth().map(|bd| bd as u32);
            let sample_rate = properties.sample_rate().map(|sr| sr as f64);
            (bit_depth, sample_rate)
        }
        Err(e) => {
            log::warn!(
                "Failed to read audio properties for track {}: {}",
                track_id,
                e
            );
            (None, None)
        }
    };

    // 7. Insert into local library DB
    let lib_opt__ = library_db.lock().await;
    let lib_guard = lib_opt__
        .as_ref()
        .ok_or("No active session - please log in")?;

    let album_artist = metadata.album_artist.as_ref().unwrap_or(&metadata.artist);
    let album_group_key = format!("{}|{}", metadata.album, album_artist);

    lib_guard
        .insert_qobuz_cached_track_with_grouping(
            track_id,
            &metadata.title,
            &metadata.artist,
            Some(&metadata.album),
            metadata.album_artist.as_deref(),
            metadata.track_number,
            metadata.disc_number,
            metadata.year,
            metadata.duration_secs,
            &new_path,
            &album_group_key,
            &metadata.album,
            bit_depth,
            sample_rate,
            artwork_path.as_deref(),
        )
        .map_err(|e| format!("Failed to insert to library DB: {}", e))?;

    log::info!("Track {} migrated successfully to: {}", track_id, new_path);
    Ok(new_path)
}

/// Migrate all legacy cached files
pub async fn migrate_legacy_cached_files(
    track_ids: Vec<u64>,
    tracks_dir: PathBuf,
    offline_root: String,
    qobuz_client: Arc<RwLock<Option<QobuzClient>>>,
    library_db: Arc<Mutex<Option<LibraryDatabase>>>,
) -> MigrationStatus {
    let total = track_ids.len();
    let mut status = MigrationStatus {
        has_legacy_files: true,
        total_tracks: total,
        in_progress: true,
        ..Default::default()
    };

    for track_id in track_ids {
        let legacy_path = tracks_dir.join(format!("{}.flac", track_id));

        if !legacy_path.exists() {
            log::warn!("Legacy file not found: {}", legacy_path.display());
            status.processed += 1;
            continue;
        }

        // Lock client for this migration
        let client_guard = qobuz_client.read().await;
        let Some(client) = client_guard.as_ref() else {
            status.failed += 1;
            status.errors.push(MigrationError {
                track_id,
                error_message: "QobuzClient not initialized".to_string(),
            });
            status.processed += 1;
            continue;
        };

        match migrate_single_track(
            track_id,
            legacy_path.clone(),
            &offline_root,
            client,
            library_db.clone(),
        )
        .await
        {
            Ok(_) => {
                status.successful += 1;

                // Delete legacy file after successful migration
                if let Err(e) = std::fs::remove_file(&legacy_path) {
                    log::warn!(
                        "Failed to delete legacy file {}: {}",
                        legacy_path.display(),
                        e
                    );
                }
            }
            Err(e) => {
                status.failed += 1;
                status.errors.push(MigrationError {
                    track_id,
                    error_message: e,
                });
                log::error!(
                    "Failed to migrate track {}: {}",
                    track_id,
                    status.errors.last().unwrap().error_message
                );
            }
        }

        drop(client_guard);

        status.processed += 1;
    }

    status.in_progress = false;
    status.completed = true;

    log::info!(
        "Migration complete: {}/{} successful, {} failed",
        status.successful,
        total,
        status.failed
    );

    status
}
