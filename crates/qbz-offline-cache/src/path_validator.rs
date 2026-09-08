//! Offline cache path validation and management
//!
//! Handles path validation, permission checking, and mount status verification.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PathStatus {
    Valid,
    DoesNotExist,
    NotADirectory,
    NoWritePermission,
    NotMounted,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PathValidationResult {
    pub status: PathStatus,
    pub message: String,
}

/// Validate an offline cache path
pub fn validate_path(path: &str) -> Result<PathValidationResult, String> {
    let path_obj = Path::new(path);

    if !path_obj.exists() {
        return Ok(PathValidationResult {
            status: PathStatus::DoesNotExist,
            message: format!("Path does not exist: {}", path),
        });
    }

    if !path_obj.is_dir() {
        return Ok(PathValidationResult {
            status: PathStatus::NotADirectory,
            message: format!("Path is not a directory: {}", path),
        });
    }

    if !check_mount_status(path)? {
        return Ok(PathValidationResult {
            status: PathStatus::NotMounted,
            message: "Storage device is not mounted".to_string(),
        });
    }

    if !check_permissions(path)? {
        return Ok(PathValidationResult {
            status: PathStatus::NoWritePermission,
            message: "No write permission for this directory".to_string(),
        });
    }

    Ok(PathValidationResult {
        status: PathStatus::Valid,
        message: "Path is valid and writable".to_string(),
    })
}

/// Check if we have write permissions on a path
pub fn check_permissions(path: &str) -> Result<bool, String> {
    let path_obj = Path::new(path);
    let test_file = path_obj.join(".qbz_write_test");

    match fs::write(&test_file, b"test") {
        Ok(_) => {
            let _ = fs::remove_file(&test_file);
            Ok(true)
        }
        Err(e) => {
            if e.kind() == std::io::ErrorKind::PermissionDenied {
                Ok(false)
            } else {
                Err(format!("Failed to check permissions: {}", e))
            }
        }
    }
}

/// Check if the path is on a mounted filesystem
pub fn check_mount_status(path: &str) -> Result<bool, String> {
    let path_obj = Path::new(path);

    // Try to canonicalize the path
    match path_obj.canonicalize() {
        Ok(canonical) => {
            // If we can read the metadata, the mount is accessible
            match canonical.metadata() {
                Ok(_) => Ok(true),
                Err(e) => {
                    if e.kind() == std::io::ErrorKind::NotFound {
                        Ok(false)
                    } else {
                        Err(format!("Failed to check mount status: {}", e))
                    }
                }
            }
        }
        Err(e) => {
            if e.kind() == std::io::ErrorKind::NotFound {
                Ok(false)
            } else {
                Err(format!("Failed to canonicalize path: {}", e))
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MoveReport {
    pub total_files: usize,
    pub moved_count: usize,
    pub failed_files: Vec<String>,
}

/// Move cached files from old path to new path
pub fn move_cached_files_to_new_path(old_root: &str, new_root: &str) -> Result<MoveReport, String> {
    let old_path = Path::new(old_root);
    let new_path = Path::new(new_root);

    // Validate new path
    let validation = validate_path(new_root)?;
    if !matches!(validation.status, PathStatus::Valid) {
        return Err(format!("New path is not valid: {}", validation.message));
    }

    // Create new root if it doesn't exist
    fs::create_dir_all(new_path).map_err(|e| format!("Failed to create new directory: {}", e))?;

    let mut report = MoveReport {
        total_files: 0,
        moved_count: 0,
        failed_files: Vec::new(),
    };

    // Collect all FLAC files recursively
    let files = collect_flac_files(old_path)?;
    report.total_files = files.len();

    for old_file in files {
        // Get relative path from old root
        let relative_path = old_file
            .strip_prefix(old_path)
            .map_err(|e| format!("Failed to get relative path: {}", e))?;

        let new_file = new_path.join(relative_path);

        // Create parent directories in new location
        if let Some(parent) = new_file.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("Failed to create directory: {}", e))?;
        }

        // Move file
        match fs::rename(&old_file, &new_file) {
            Ok(_) => {
                report.moved_count += 1;
            }
            Err(e) => {
                log::warn!("Failed to move file {:?}: {}", old_file, e);
                report
                    .failed_files
                    .push(old_file.to_string_lossy().to_string());
            }
        }
    }

    Ok(report)
}

/// Recursively collect all FLAC files in a directory
fn collect_flac_files(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let mut files = Vec::new();

    if !dir.is_dir() {
        return Ok(files);
    }

    let entries = fs::read_dir(dir).map_err(|e| format!("Failed to read directory: {}", e))?;

    for entry in entries {
        let entry = entry.map_err(|e| format!("Failed to read entry: {}", e))?;
        let path = entry.path();

        if path.is_dir() {
            // Recurse into subdirectories
            files.extend(collect_flac_files(&path)?);
        } else if path.extension().and_then(|s| s.to_str()) == Some("flac") {
            files.push(path);
        }
    }

    Ok(files)
}

// Mount status cache
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

struct MountStatusCache {
    path: String,
    is_mounted: bool,
    last_check: SystemTime,
}

static MOUNT_CACHE: Mutex<Option<MountStatusCache>> = Mutex::new(None);
const CACHE_DURATION: Duration = Duration::from_secs(30);

/// Check mount status with caching (30s cache)
pub fn is_offline_root_available(path: &str) -> Result<bool, String> {
    let mut cache = MOUNT_CACHE
        .lock()
        .map_err(|e| format!("Cache lock error: {}", e))?;

    if let Some(cached) = cache.as_ref() {
        if cached.path == path {
            if let Ok(elapsed) = SystemTime::now().duration_since(cached.last_check) {
                if elapsed < CACHE_DURATION {
                    return Ok(cached.is_mounted);
                }
            }
        }
    }

    // Cache miss or expired, check mount status
    let is_mounted = check_mount_status(path)?;

    *cache = Some(MountStatusCache {
        path: path.to_string(),
        is_mounted,
        last_check: SystemTime::now(),
    });

    Ok(is_mounted)
}
