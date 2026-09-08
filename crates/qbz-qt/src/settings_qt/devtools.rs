//! Settings > Developer, Settings > Blacklist and the Flatpak/Snap section.
//!
//! ADR-006 again: the log file is `qbz_log::install::log_file_path()` (the
//! sink `main.rs` installs), the export bundle is the shared
//! `qbz_app::settings::bundle` engine (the same one `qbzd` uses), and the
//! blacklist counters come from the per-user `artist_blacklist` singleton —
//! the same store the Blacklist Manager view mutates, so the row cannot go
//! stale behind it.
//!
//! Qt adaptations:
//! - QConnect diagnostics and the in-app log viewer are live; this module
//!   supplies the settings actions while their documents live in
//!   `diagnostics_qt` and `log_viewer_qt`.
//! - The settings export keeps the reference's include-auth gate, defaulting
//!   to OFF, as an inline toggle instead of a separate modal.

use serde::Serialize;
use std::sync::{LazyLock, Mutex};

static STATUS: LazyLock<Mutex<String>> = LazyLock::new(|| Mutex::new(String::new()));

#[derive(Clone, Default, Serialize)]
pub struct Snapshot {
    /// Absolute path of the current run's log file ("" when unavailable).
    #[serde(rename = "logPath")]
    pub log_path: String,
    /// Result line of the last developer action (export…).
    pub status: String,
    #[serde(rename = "blacklistEnabled")]
    pub blacklist_enabled: bool,
    #[serde(rename = "blacklistArtists")]
    pub blacklist_artists: i32,
    #[serde(rename = "blacklistAlbums")]
    pub blacklist_albums: i32,
    /// "flatpak" | "snap" | "" — gates the sandbox sub-nav entry.
    #[serde(rename = "installMethod")]
    pub install_method: String,
}

/// Sandboxed-install detection (the Slint's `SandboxState.install-method`):
/// Flatpak exposes `/.flatpak-info`, Snap exports `$SNAP`.
fn install_method() -> String {
    if std::path::Path::new("/.flatpak-info").exists() {
        return "flatpak".to_string();
    }
    if std::env::var_os("SNAP").is_some() {
        return "snap".to_string();
    }
    String::new()
}

/// Blacklist counters, read-only, straight off the per-user singleton bound at
/// session activation (`auth_qt::bind_per_user_stores`).
///
/// This used to open a throwaway `BlacklistService` on every settings publish.
/// It no longer can: the manager view mutates the SAME store, so a second
/// handle would serve stale counts the moment the user removed an artist.
///
/// Fail-open is unchanged. `artist_blacklist`'s accessors return
/// `(true, 0, 0)` for exactly the three cases the deleted early-returns
/// covered — no session bound, no store file, store open failed — so the
/// no-session and fresh-account behaviours do not move. The counts
/// deliberately ignore the enabled flag: the row reports what is stored, and
/// the flag is rendered separately.
fn blacklist_counts() -> (bool, i32, i32) {
    crate::blacklist_qt::counts()
}

pub fn snapshot() -> Snapshot {
    let (blacklist_enabled, blacklist_artists, blacklist_albums) = blacklist_counts();
    Snapshot {
        log_path: qbz_log::install::log_file_path()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default(),
        status: STATUS.lock().unwrap_or_else(|e| e.into_inner()).clone(),
        blacklist_enabled,
        blacklist_artists,
        blacklist_albums,
        install_method: install_method(),
    }
}

/// Developer > Application logs (and the sub-nav "Share logs"): reveal the
/// on-disk log so it can be attached to an issue.
pub fn open_log_file() {
    let Some(path) = qbz_log::install::log_file_path() else {
        return;
    };
    if let Err(e) = open::that(&path) {
        log::warn!("[qbz-qt] could not open the log file: {e}");
        *STATUS.lock().unwrap_or_else(|e| e.into_inner()) =
            qbz_i18n::t("Could not open the log file.");
    }
}
