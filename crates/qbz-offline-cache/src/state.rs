//! The offline-cache state holder (moved from `src-tauri/src/offline_cache/mod.rs`).
//!
//! A plain struct (no Tauri): the open SQLite index, the stream fetcher,
//! the cache-dir path, the size limit, the download concurrency semaphore,
//! and a separate library-DB connection for download post-processing.
//! Both the Tauri frontend (`tauri::State`) and the Slint frontend own one.

use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use tokio::sync::{Mutex, Semaphore};

use crate::db::OfflineCacheDb;
use crate::downloader::StreamFetcher;

/// Offline cache state manager
pub struct OfflineCacheState {
    pub db: Arc<Mutex<Option<OfflineCacheDb>>>,
    pub fetcher: Arc<StreamFetcher>,
    pub cache_dir: Arc<RwLock<PathBuf>>,
    /// Cache limit in bytes (None = unlimited)
    pub limit_bytes: Arc<Mutex<Option<u64>>>,
    pub cache_semaphore: Arc<Semaphore>,
    /// Separate library DB connection for download post-processing writes.
    /// This avoids contending with the main library DB mutex used by UI queries.
    pub library_db: Arc<Mutex<Option<qbz_library::LibraryDatabase>>>,
}

impl OfflineCacheState {
    /// Initialize the offline cache at the platform cache dir.
    pub fn new() -> Result<Self, String> {
        let cache_dir = dirs::cache_dir()
            .ok_or("Could not determine cache directory")?
            .join("qbz")
            .join("audio");

        // Create directories
        std::fs::create_dir_all(&cache_dir)
            .map_err(|e| format!("Failed to create cache directory: {}", e))?;
        std::fs::create_dir_all(cache_dir.join("tracks"))
            .map_err(|e| format!("Failed to create tracks directory: {}", e))?;
        std::fs::create_dir_all(cache_dir.join("artwork"))
            .map_err(|e| format!("Failed to create artwork directory: {}", e))?;

        let db_path = cache_dir.join("index.db");
        let db = OfflineCacheDb::new(&db_path)?;

        // Default limit: 5GB
        let default_limit = Some(5 * 1024 * 1024 * 1024u64);

        let state = Self {
            db: Arc::new(Mutex::new(Some(db))),
            fetcher: Arc::new(StreamFetcher::new()),
            cache_dir: Arc::new(RwLock::new(cache_dir.clone())),
            limit_bytes: Arc::new(Mutex::new(default_limit)),
            cache_semaphore: Arc::new(Semaphore::new(3)),
            library_db: Arc::new(Mutex::new(None)),
        };

        log::info!("Offline cache initialized at: {:?}", cache_dir);

        Ok(state)
    }

    pub fn new_empty() -> Self {
        let cache_dir = dirs::cache_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("qbz")
            .join("audio");
        Self {
            db: Arc::new(Mutex::new(None)),
            fetcher: Arc::new(StreamFetcher::new()),
            cache_dir: Arc::new(RwLock::new(cache_dir)),
            limit_bytes: Arc::new(Mutex::new(Some(5 * 1024 * 1024 * 1024u64))),
            cache_semaphore: Arc::new(Semaphore::new(3)),
            library_db: Arc::new(Mutex::new(None)),
        }
    }

    pub async fn init_at(&self, cache_base_dir: &std::path::Path) -> Result<(), String> {
        let cache_dir = cache_base_dir.join("audio");
        std::fs::create_dir_all(&cache_dir)
            .map_err(|e| format!("Failed to create cache directory: {}", e))?;
        std::fs::create_dir_all(cache_dir.join("tracks"))
            .map_err(|e| format!("Failed to create tracks directory: {}", e))?;
        std::fs::create_dir_all(cache_dir.join("artwork"))
            .map_err(|e| format!("Failed to create artwork directory: {}", e))?;
        let db_path = cache_dir.join("index.db");
        let new_db = OfflineCacheDb::new(&db_path)?;
        let mut guard = self.db.lock().await;
        *guard = Some(new_db);
        // Update cache_dir to user-scoped path
        if let Ok(mut dir_guard) = self.cache_dir.write() {
            *dir_guard = cache_dir.clone();
        }
        log::info!("Offline cache initialized at: {:?}", cache_dir);
        Ok(())
    }

    /// Open a separate library DB connection for download post-processing.
    /// Must be called after library.init_at() so the schema exists.
    pub async fn init_library_connection(&self, data_dir: &std::path::Path) -> Result<(), String> {
        let db_path = data_dir.join("library.db");
        let lib_db = qbz_library::LibraryDatabase::open(&db_path)
            .map_err(|e| format!("Failed to open download library connection: {}", e))?;
        let mut guard = self.library_db.lock().await;
        *guard = Some(lib_db);
        log::info!(
            "Offline cache: separate library DB connection opened at {:?}",
            db_path
        );
        Ok(())
    }

    pub async fn teardown(&self) {
        // Close library connection first (before main teardown)
        {
            let mut lib_guard = self.library_db.lock().await;
            *lib_guard = None;
        }
        let mut guard = self.db.lock().await;
        *guard = None;
    }

    /// Get the path for a track's audio file
    pub fn track_file_path(&self, track_id: u64, format: &str) -> PathBuf {
        let dir = self.cache_dir.read().unwrap();
        dir.join("tracks").join(format!("{}.{}", track_id, format))
    }

    /// Get the path for an album's artwork
    pub fn artwork_path(&self, album_id: &str) -> PathBuf {
        let dir = self.cache_dir.read().unwrap();
        dir.join("artwork").join(format!("{}.jpg", album_id))
    }

    /// Get the cache directory path
    pub fn get_cache_path(&self) -> String {
        let dir = self.cache_dir.read().unwrap();
        dir.to_string_lossy().to_string()
    }

    /// Seed the in-memory `limit_bytes` from a persisted value (read by the
    /// caller from the offline_settings DB) so the user's previously chosen
    /// limit survives across restarts. `None` keeps the in-memory default
    /// (5 GB) seeded by `new`/`init_at`.
    pub async fn apply_persisted_limit(&self, persisted: Option<u64>) {
        if let Some(bytes) = persisted {
            let mut limit = self.limit_bytes.lock().await;
            *limit = Some(bytes);
            log::info!(
                "Offline cache: applied persisted size limit ({} bytes)",
                bytes
            );
        } else {
            log::info!("Offline cache: no persisted size limit, keeping in-memory default");
        }
    }
}
