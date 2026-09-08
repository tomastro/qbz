//! Per-user connection settings for the media servers QBZ reads.
//!
//! ONE store for Jellyfin and Subsonic, keyed by a `server` column — the same
//! decision `qbz-media-cache` makes about their tracks, for the same reason:
//! the two carry the same fields, and a store per server means every future one
//! copies a file. Plex keeps its own ([`super::plex`]) until it folds into the
//! shared pair; see that crate's header for why that is a separate change.
//!
//! Mirrors [`super::plex`]'s shape: a small SQLite table, opened globally via
//! [`MediaServerStore::new`] and re-pointed at the active user's directory by
//! [`MediaServerState::init_at`] at login, so credentials are scoped per Qobuz
//! user.
//!
//! # What is stored, and what deliberately is not
//!
//! **Jellyfin** stores its `access_token` — the server issues it and the
//! password is never needed again. **Subsonic has no session**: every request
//! carries `md5(password + salt)`, so the password itself must be kept.
//!
//! That asymmetry is the protocol's, not a shortcut. It is also why `salt` is
//! persisted rather than generated per request: a rolling salt would make every
//! cover URL unique per request, and the artwork cache keys on the URL, so each
//! pass would re-download every cover. The salt is not a secret — it travels in
//! clear beside the token — its job is to keep one password from hashing to the
//! same value on two installs.
//!
//! `device_id` is Jellyfin's, and it is load-bearing: the server keys its
//! SESSION on it, and re-authenticating under the same one revokes the previous
//! token (measured against 10.11.11). Generated once and persisted, so two QBZ
//! installs never log each other out and a relaunch does not drop the token.

use log::info;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::{Arc, Mutex};

/// Which server a settings row describes. The wire values are the same words
/// `qbz_source::SourceId` and `qbz_media_cache::RemoteSource` use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaServerKind {
    Jellyfin,
    Subsonic,
}

impl MediaServerKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            MediaServerKind::Jellyfin => "jellyfin",
            MediaServerKind::Subsonic => "subsonic",
        }
    }

    pub fn from_word(w: &str) -> Option<Self> {
        match w {
            "jellyfin" => Some(MediaServerKind::Jellyfin),
            "subsonic" | "navidrome" | "gonic" | "airsonic" | "astiga" => {
                Some(MediaServerKind::Subsonic)
            }
            _ => None,
        }
    }

    pub const ALL: [MediaServerKind; 2] = [MediaServerKind::Jellyfin, MediaServerKind::Subsonic];
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MediaServerSettings {
    /// Master toggle. Default OFF — integrations are opt-in.
    pub enabled: bool,
    /// Collapse chevron state for the settings panel.
    pub ui_collapsed: bool,
    /// `proto://host:port`, resolved. Empty = not configured.
    pub base_url: String,
    /// Display name reported by the server, for the panel's "connected to …".
    pub server_name: String,
    /// The server's own id, threaded into the cache as `server_id`.
    pub server_id: String,
    /// Jellyfin: the account name. Subsonic: the account name, sent on every
    /// request.
    pub username: String,
    /// Jellyfin: the issued `AccessToken`. Empty for Subsonic.
    pub token: String,
    /// Subsonic ONLY: the password, because the protocol re-derives its token
    /// on every request. Empty for Jellyfin, which never needs it again.
    pub password: String,
    /// Subsonic ONLY: the FIXED per-install salt. See the module header.
    pub salt: String,
    /// Jellyfin ONLY: the stable per-install `DeviceId`. See the module header.
    pub device_id: String,
    /// Picked library / music-folder ids. Empty = none chosen yet, which the
    /// sync reads as "all of them" on its first run.
    pub selected_libraries: Vec<String>,
    /// Unix seconds of the last COMPLETED sweep. 0 = never swept.
    ///
    /// Only a completed sweep updates it, because it is what a delta sync asks
    /// the server about — stamping it after a partial one would silently skip
    /// whatever the interrupted run never saw.
    pub last_sync_at: i64,
    /// Tracks in the cache at the end of that sweep, for the panel's summary.
    pub last_sync_tracks: i64,
}

impl MediaServerSettings {
    /// Is this server usable right now — enabled, addressed, and carrying
    /// whatever credential its protocol needs?
    ///
    /// The two protocols disagree about what "credentialed" means, and this is
    /// the ONE place that knows: Jellyfin needs its issued token, Subsonic
    /// needs a username and password.
    pub fn is_configured(&self, kind: MediaServerKind) -> bool {
        if !self.enabled || self.base_url.trim().is_empty() {
            return false;
        }
        match kind {
            MediaServerKind::Jellyfin => !self.token.trim().is_empty(),
            MediaServerKind::Subsonic => {
                !self.username.trim().is_empty() && !self.password.is_empty()
            }
        }
    }
}

pub struct MediaServerStore {
    conn: Connection,
}

impl MediaServerStore {
    fn open_at(dir: &Path) -> Result<Self, String> {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("Failed to create data directory: {e}"))?;
        let conn = Connection::open(dir.join("media_servers.db"))
            .map_err(|e| format!("Failed to open media server settings: {e}"))?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")
            .map_err(|e| format!("Failed to set pragmas: {e}"))?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS media_server_settings (
                server              TEXT PRIMARY KEY,
                enabled             INTEGER NOT NULL DEFAULT 0,
                ui_collapsed        INTEGER NOT NULL DEFAULT 0,
                base_url            TEXT NOT NULL DEFAULT '',
                server_name         TEXT NOT NULL DEFAULT '',
                server_id           TEXT NOT NULL DEFAULT '',
                username            TEXT NOT NULL DEFAULT '',
                token               TEXT NOT NULL DEFAULT '',
                password            TEXT NOT NULL DEFAULT '',
                salt                TEXT NOT NULL DEFAULT '',
                device_id           TEXT NOT NULL DEFAULT '',
                selected_libraries  TEXT NOT NULL DEFAULT '',
                last_sync_at        INTEGER NOT NULL DEFAULT 0,
                last_sync_tracks    INTEGER NOT NULL DEFAULT 0
            );",
        )
        .map_err(|e| format!("Failed to init media server schema: {e}"))?;
        info!("[MediaServers] Database initialized");
        Ok(Self { conn })
    }

    pub fn new() -> Result<Self, String> {
        let dir = dirs::data_dir()
            .ok_or("Could not determine data directory")?
            .join("qbz");
        Self::open_at(&dir)
    }

    pub fn new_at(base_dir: &Path) -> Result<Self, String> {
        Self::open_at(base_dir)
    }

    /// Read one server's settings. A row that has never been written comes back
    /// as defaults (disabled, unconfigured) rather than an error — "the user
    /// has not set this up" is not a failure.
    pub fn get(&self, kind: MediaServerKind) -> Result<MediaServerSettings, String> {
        let row = self.conn.query_row(
            "SELECT enabled, ui_collapsed, base_url, server_name, server_id, username,
                    token, password, salt, device_id, selected_libraries,
                    last_sync_at, last_sync_tracks
             FROM media_server_settings WHERE server = ?1",
            params![kind.as_str()],
            |r| {
                Ok(MediaServerSettings {
                    enabled: r.get::<_, i64>(0)? != 0,
                    ui_collapsed: r.get::<_, i64>(1)? != 0,
                    base_url: r.get(2)?,
                    server_name: r.get(3)?,
                    server_id: r.get(4)?,
                    username: r.get(5)?,
                    token: r.get(6)?,
                    password: r.get(7)?,
                    salt: r.get(8)?,
                    device_id: r.get(9)?,
                    selected_libraries: split_ids(&r.get::<_, String>(10)?),
                    last_sync_at: r.get(11)?,
                    last_sync_tracks: r.get(12)?,
                })
            },
        );
        match row {
            Ok(s) => Ok(s),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(MediaServerSettings::default()),
            Err(e) => Err(format!("Failed to read media server settings: {e}")),
        }
    }

    /// Write one server's settings wholesale.
    ///
    /// One UPSERT rather than a setter per field: the settings panel edits a
    /// form and saves it, and a dozen single-column setters is how `plex.rs`
    /// grew to 723 lines.
    pub fn put(&self, kind: MediaServerKind, s: &MediaServerSettings) -> Result<(), String> {
        self.conn
            .execute(
                "INSERT INTO media_server_settings
                   (server, enabled, ui_collapsed, base_url, server_name, server_id,
                    username, token, password, salt, device_id, selected_libraries,
                    last_sync_at, last_sync_tracks)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)
                 ON CONFLICT(server) DO UPDATE SET
                    enabled=excluded.enabled, ui_collapsed=excluded.ui_collapsed,
                    base_url=excluded.base_url, server_name=excluded.server_name,
                    server_id=excluded.server_id, username=excluded.username,
                    token=excluded.token, password=excluded.password,
                    salt=excluded.salt, device_id=excluded.device_id,
                    selected_libraries=excluded.selected_libraries,
                    last_sync_at=excluded.last_sync_at,
                    last_sync_tracks=excluded.last_sync_tracks",
                params![
                    kind.as_str(),
                    i64::from(s.enabled),
                    i64::from(s.ui_collapsed),
                    s.base_url.trim(),
                    s.server_name,
                    s.server_id,
                    s.username.trim(),
                    s.token.trim(),
                    s.password,
                    s.salt,
                    s.device_id,
                    s.selected_libraries.join(","),
                    s.last_sync_at,
                    s.last_sync_tracks,
                ],
            )
            .map(|_| ())
            .map_err(|e| format!("Failed to write media server settings: {e}"))
    }

    /// Forget one server's credentials, keeping the row so its UI state and its
    /// picked libraries survive a reconnect.
    pub fn disconnect(&self, kind: MediaServerKind) -> Result<(), String> {
        self.conn
            .execute(
                "UPDATE media_server_settings
                 SET enabled = 0, token = '', password = '', server_id = '',
                     server_name = '', last_sync_at = 0, last_sync_tracks = 0
                 WHERE server = ?1",
                params![kind.as_str()],
            )
            .map(|_| ())
            .map_err(|e| format!("Failed to disconnect media server: {e}"))
    }
}

fn split_ids(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect()
}

/// The process-global store, re-pointed at the active user at login.
#[derive(Clone)]
pub struct MediaServerState {
    store: Arc<Mutex<Option<MediaServerStore>>>,
}

impl Default for MediaServerState {
    fn default() -> Self {
        Self::new()
    }
}

impl MediaServerState {
    pub fn new() -> Self {
        Self {
            store: Arc::new(Mutex::new(None)),
        }
    }

    /// Bind to the active user's directory. Called from the auth path.
    pub fn init_at(&self, dir: &Path) {
        match MediaServerStore::new_at(dir) {
            Ok(s) => {
                if let Ok(mut slot) = self.store.lock() {
                    *slot = Some(s);
                }
            }
            Err(e) => log::error!("[MediaServers] init failed: {e}"),
        }
    }

    /// Drop the handle (logout).
    pub fn reset(&self) {
        if let Ok(mut slot) = self.store.lock() {
            *slot = None;
        }
    }

    /// Read. Unbound or unreadable answers with DEFAULTS — i.e. "disabled" —
    /// which is the safe direction: a settings read that fails must not make an
    /// unconfigured server look configured.
    pub fn get(&self, kind: MediaServerKind) -> MediaServerSettings {
        self.store
            .lock()
            .ok()
            .and_then(|g| g.as_ref().and_then(|s| s.get(kind).ok()))
            .unwrap_or_default()
    }

    pub fn put(&self, kind: MediaServerKind, s: &MediaServerSettings) {
        if let Ok(g) = self.store.lock() {
            if let Some(store) = g.as_ref() {
                if let Err(e) = store.put(kind, s) {
                    log::error!("[MediaServers] write failed: {e}");
                }
            }
        }
    }

    pub fn disconnect(&self, kind: MediaServerKind) {
        if let Ok(g) = self.store.lock() {
            if let Some(store) = g.as_ref() {
                let _ = store.disconnect(kind);
            }
        }
    }

    /// Every server the user has switched on AND finished configuring.
    ///
    /// This is what the Local Library union filters on, so "enabled" alone is
    /// not enough: a server that is toggled on but has no credentials yet would
    /// contribute a `source IN (...)` arm over rows that were never swept.
    pub fn configured_words(&self) -> Vec<&'static str> {
        MediaServerKind::ALL
            .iter()
            .filter(|k| self.get(**k).is_configured(**k))
            .map(|k| k.as_str())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn store() -> (TempDir, MediaServerStore) {
        let t = TempDir::new().unwrap();
        let s = MediaServerStore::new_at(t.path()).unwrap();
        (t, s)
    }

    /// A server the user has never touched reads as defaults, not as an error.
    #[test]
    fn an_unwritten_server_reads_as_disabled() {
        let (_t, s) = store();
        let jf = s.get(MediaServerKind::Jellyfin).unwrap();
        assert!(!jf.enabled);
        assert!(jf.base_url.is_empty());
        assert!(!jf.is_configured(MediaServerKind::Jellyfin));
    }

    #[test]
    fn settings_round_trip_per_server_without_bleeding() {
        let (_t, s) = store();
        let jf = MediaServerSettings {
            enabled: true,
            base_url: "http://jf:8096".into(),
            username: "admin".into(),
            token: "tok".into(),
            device_id: "qbz-abc".into(),
            selected_libraries: vec!["lib1".into(), "lib2".into()],
            ..Default::default()
        };
        s.put(MediaServerKind::Jellyfin, &jf).unwrap();

        let got = s.get(MediaServerKind::Jellyfin).unwrap();
        assert_eq!(got.base_url, "http://jf:8096");
        assert_eq!(got.token, "tok");
        assert_eq!(got.selected_libraries, vec!["lib1", "lib2"]);
        // The other server is untouched.
        assert!(!s.get(MediaServerKind::Subsonic).unwrap().enabled);
    }

    /// The two protocols disagree about what "credentialed" means, and this is
    /// the one place that knows. Getting it backwards would either refuse a
    /// working server or hand the sync a connection it cannot authenticate.
    #[test]
    fn configured_means_a_different_thing_for_each_protocol() {
        // Jellyfin: an issued token, and the password is never needed again.
        let mut jf = MediaServerSettings {
            enabled: true,
            base_url: "http://jf:8096".into(),
            username: "admin".into(),
            ..Default::default()
        };
        assert!(!jf.is_configured(MediaServerKind::Jellyfin), "no token yet");
        jf.token = "tok".into();
        assert!(jf.is_configured(MediaServerKind::Jellyfin));

        // Subsonic: no session at all, so the PASSWORD is what it needs.
        let mut sub = MediaServerSettings {
            enabled: true,
            base_url: "http://nd:4533".into(),
            username: "admin".into(),
            ..Default::default()
        };
        assert!(!sub.is_configured(MediaServerKind::Subsonic), "no password yet");
        sub.password = "pw".into();
        assert!(sub.is_configured(MediaServerKind::Subsonic));
        // ...and a Jellyfin-style token does NOT make it configured.
        let token_only = MediaServerSettings {
            token: "tok".into(),
            ..sub.clone()
        };
        let token_only = MediaServerSettings {
            password: String::new(),
            ..token_only
        };
        assert!(!token_only.is_configured(MediaServerKind::Subsonic));
    }

    /// A disabled or unaddressed server is never configured, whatever else it
    /// holds — that is what makes the master toggle actually remove its rows
    /// from the Local Library union.
    #[test]
    fn the_master_toggle_and_the_address_both_gate_it() {
        let full = MediaServerSettings {
            enabled: true,
            base_url: "http://jf:8096".into(),
            token: "tok".into(),
            ..Default::default()
        };
        assert!(full.is_configured(MediaServerKind::Jellyfin));
        assert!(!MediaServerSettings {
            enabled: false,
            ..full.clone()
        }
        .is_configured(MediaServerKind::Jellyfin));
        assert!(!MediaServerSettings {
            base_url: "   ".into(),
            ..full
        }
        .is_configured(MediaServerKind::Jellyfin));
    }

    /// Disconnect clears the CREDENTIALS and keeps the picked libraries, so a
    /// reconnect does not make the user choose them again.
    #[test]
    fn disconnect_forgets_the_credentials_but_not_the_choices() {
        let (_t, s) = store();
        s.put(
            MediaServerKind::Subsonic,
            &MediaServerSettings {
                enabled: true,
                base_url: "http://nd:4533".into(),
                username: "admin".into(),
                password: "pw".into(),
                salt: "fixed".into(),
                selected_libraries: vec!["1".into()],
                last_sync_tracks: 6678,
                ..Default::default()
            },
        )
        .unwrap();
        s.disconnect(MediaServerKind::Subsonic).unwrap();
        let got = s.get(MediaServerKind::Subsonic).unwrap();
        assert!(!got.enabled);
        assert!(got.password.is_empty(), "the password survived a disconnect");
        assert_eq!(got.last_sync_tracks, 0);
        // Kept on purpose.
        assert_eq!(got.base_url, "http://nd:4533");
        assert_eq!(got.username, "admin");
        assert_eq!(got.salt, "fixed");
        assert_eq!(got.selected_libraries, vec!["1"]);
    }

    #[test]
    fn brand_spellings_fold_to_subsonic() {
        for w in ["subsonic", "navidrome", "gonic", "airsonic", "astiga"] {
            assert_eq!(
                MediaServerKind::from_word(w),
                Some(MediaServerKind::Subsonic),
                "{w}"
            );
        }
        assert_eq!(
            MediaServerKind::from_word("jellyfin"),
            Some(MediaServerKind::Jellyfin)
        );
        assert_eq!(MediaServerKind::from_word("plex"), None);
        assert_eq!(MediaServerKind::from_word("emby"), None);
    }

    /// Only a server that is enabled AND finished gets into the union filter.
    /// A half-configured one would otherwise widen the SQL over rows that were
    /// never swept.
    #[test]
    fn only_fully_configured_servers_reach_the_union_filter() {
        let t = TempDir::new().unwrap();
        let state = MediaServerState::new();
        state.init_at(t.path());
        assert!(state.configured_words().is_empty());

        state.put(
            MediaServerKind::Jellyfin,
            &MediaServerSettings {
                enabled: true,
                base_url: "http://jf:8096".into(),
                token: "tok".into(),
                ..Default::default()
            },
        );
        // Enabled but with no password — not configured.
        state.put(
            MediaServerKind::Subsonic,
            &MediaServerSettings {
                enabled: true,
                base_url: "http://nd:4533".into(),
                username: "admin".into(),
                ..Default::default()
            },
        );
        assert_eq!(state.configured_words(), vec!["jellyfin"]);
    }

    /// An UNBOUND state reads as disabled rather than panicking or looking
    /// configured — the same safe direction the rest of the app takes.
    #[test]
    fn an_unbound_state_reports_nothing_configured() {
        let state = MediaServerState::new();
        assert!(!state.get(MediaServerKind::Jellyfin).enabled);
        assert!(state.configured_words().is_empty());
    }
}
