//! Per-user data path management.
//!
//! Each Qobuz user gets their own subdirectory under the app's data/cache paths.
//! This module provides the central path provider that host shells use to
//! determine where to store per-user databases and cache files.

use std::path::PathBuf;
use std::sync::RwLock;

/// Central path provider for per-user data isolation.
///
/// Holds the current user_id and provides methods to get user-scoped data and
/// cache directories.
pub struct UserDataPaths {
    user_id: RwLock<Option<u64>>,
}

impl UserDataPaths {
    pub fn new() -> Self {
        Self {
            user_id: RwLock::new(None),
        }
    }

    /// Set the current user after login.
    pub fn set_user(&self, user_id: u64) {
        *self
            .user_id
            .write()
            .expect("UserDataPaths write lock poisoned") = Some(user_id);
        log::info!("UserDataPaths: active user set");
    }

    /// Clear the current user on logout.
    pub fn clear_user(&self) {
        *self
            .user_id
            .write()
            .expect("UserDataPaths write lock poisoned") = None;
        log::info!("UserDataPaths: active user cleared");
    }

    /// Get the current user ID, if set.
    pub fn current_user_id(&self) -> Option<u64> {
        *self
            .user_id
            .read()
            .expect("UserDataPaths read lock poisoned")
    }

    /// Get the user-scoped data directory: ~/.local/share/qbz/users/{uid}/
    pub fn user_data_dir(&self) -> Result<PathBuf, String> {
        let uid = self
            .user_id
            .read()
            .map_err(|e| format!("UserDataPaths read lock error: {}", e))?
            .ok_or("No active user - please log in")?;

        let base = dirs::data_dir()
            .ok_or("Could not determine data directory")?
            .join("qbz")
            .join("users")
            .join(uid.to_string());

        Ok(base)
    }

    /// Get the user-scoped cache directory: ~/.cache/qbz/users/{uid}/
    pub fn user_cache_dir(&self) -> Result<PathBuf, String> {
        let uid = self
            .user_id
            .read()
            .map_err(|e| format!("UserDataPaths read lock error: {}", e))?
            .ok_or("No active user - please log in")?;

        let base = dirs::cache_dir()
            .ok_or("Could not determine cache directory")?
            .join("qbz")
            .join("users")
            .join(uid.to_string());

        Ok(base)
    }

    /// Data directory for an ARBITRARY user id (no active-user requirement):
    /// ~/.local/share/qbz/users/{uid}/ — the same layout `user_data_dir`
    /// resolves for the active user. Used by the guest-profile adoption
    /// (#553), which must compare two users' paths before either is active.
    pub fn data_dir_for(user_id: u64) -> Result<PathBuf, String> {
        Ok(Self::global_data_dir()?
            .join("users")
            .join(user_id.to_string()))
    }

    /// Cache twin of [`Self::data_dir_for`]: ~/.cache/qbz/users/{uid}/.
    pub fn cache_dir_for(user_id: u64) -> Result<PathBuf, String> {
        Ok(Self::global_cache_dir()?
            .join("users")
            .join(user_id.to_string()))
    }

    /// Get the global (non-user-scoped) data directory: ~/.local/share/qbz/
    pub fn global_data_dir() -> Result<PathBuf, String> {
        dirs::data_dir()
            .ok_or_else(|| "Could not determine data directory".to_string())
            .map(|d| d.join("qbz"))
    }

    /// Get the global (non-user-scoped) cache directory: ~/.cache/qbz/
    pub fn global_cache_dir() -> Result<PathBuf, String> {
        dirs::cache_dir()
            .ok_or_else(|| "Could not determine cache directory".to_string())
            .map(|d| d.join("qbz"))
    }

    /// Save the last active user_id to a flat-path file so the session can be
    /// restored on next app launch when remember-me is active.
    pub fn save_last_user_id(user_id: u64) -> Result<(), String> {
        let path = Self::last_user_id_path()?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|e| format!("Failed to create global data directory: {}", e))?;
        }
        std::fs::write(&path, user_id.to_string())
            .map_err(|e| format!("Failed to save last user id: {}", e))?;
        log::info!("Saved last_user_id marker");
        Ok(())
    }

    /// Read the last active user_id. Returns None if the file is missing or
    /// invalid.
    pub fn load_last_user_id() -> Option<u64> {
        let path = Self::last_user_id_path().ok()?;
        let contents = std::fs::read_to_string(&path).ok()?;
        contents.trim().parse::<u64>().ok()
    }

    fn last_user_id_path() -> Result<PathBuf, String> {
        let dir = Self::global_data_dir()?;
        Ok(dir.join("last_user_id"))
    }
}

impl Default for UserDataPaths {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_without_active_user() {
        let paths = UserDataPaths::new();

        assert_eq!(paths.current_user_id(), None);
        assert!(paths.user_data_dir().is_err());
        assert!(paths.user_cache_dir().is_err());
    }

    #[test]
    fn set_and_clear_user_updates_current_user() {
        let paths = UserDataPaths::new();

        paths.set_user(10385965);
        assert_eq!(paths.current_user_id(), Some(10385965));

        paths.clear_user();
        assert_eq!(paths.current_user_id(), None);
    }

    #[test]
    fn user_dirs_are_scoped_by_user_id() {
        let paths = UserDataPaths::new();
        paths.set_user(42);

        let data_dir = paths.user_data_dir().expect("data dir");
        let cache_dir = paths.user_cache_dir().expect("cache dir");

        assert!(data_dir.ends_with("qbz/users/42"));
        assert!(cache_dir.ends_with("qbz/users/42"));
    }

    #[test]
    fn global_dirs_are_scoped_to_qbz() {
        let data_dir = UserDataPaths::global_data_dir().expect("global data dir");
        let cache_dir = UserDataPaths::global_cache_dir().expect("global cache dir");

        assert!(data_dir.ends_with("qbz"));
        assert!(cache_dir.ends_with("qbz"));
    }
}
