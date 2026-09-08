//! The account snapshot: everything the migration moves, captured while
//! signed in with that account, as one JSON file.
//!
//! The same capture serves both sides: taken on the SOURCE it is the
//! snapshot the user keeps; taken on the TARGET (in memory, right before
//! applying) it is the live state the delta is computed against.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::api::MigrationApi;
use crate::sink::{MigrationEvent, MigrationPhase, MigrationSink};

pub const KIND: &str = "qbz-account-snapshot";
pub const SCHEMA_VERSION: u32 = 1;

/// Favorites page size and the per-kind cap, the same policy as the
/// Library's reader (`library_qt::fetch_favorites`).
const PAGE_SIZE: u32 = 500;
const MAX_ITEMS: usize = 10_000;

/// The five favorite kinds Qobuz keeps, in the order they are migrated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FavoriteKind {
    Albums,
    Tracks,
    Artists,
    Labels,
    Awards,
}

impl FavoriteKind {
    pub const ALL: [FavoriteKind; 5] = [
        FavoriteKind::Albums,
        FavoriteKind::Tracks,
        FavoriteKind::Artists,
        FavoriteKind::Labels,
        FavoriteKind::Awards,
    ];

    /// The `type` query value of `getUserFavorites` and the envelope key.
    pub fn plural(self) -> &'static str {
        match self {
            FavoriteKind::Albums => "albums",
            FavoriteKind::Tracks => "tracks",
            FavoriteKind::Artists => "artists",
            FavoriteKind::Labels => "labels",
            FavoriteKind::Awards => "awards",
        }
    }

    /// The `<singular>_ids` key of `favorite/create`.
    pub fn singular(self) -> &'static str {
        match self {
            FavoriteKind::Albums => "album",
            FavoriteKind::Tracks => "track",
            FavoriteKind::Artists => "artist",
            FavoriteKind::Labels => "label",
            FavoriteKind::Awards => "award",
        }
    }
}

/// One favorite. `id` is a string for every kind because album ids are
/// alphanumeric; numeric ids round-trip through their decimal form.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FavoriteItem {
    pub id: String,
    /// Unix seconds when the account favorited it, when Qobuz sends it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub favorited_at: Option<i64>,
    /// A display name for reports: album/track title, artist/label name.
    #[serde(default)]
    pub title: String,
    /// The position in the server's listing (newest first), the fallback
    /// ordering when `favorited_at` is absent.
    #[serde(default)]
    pub server_index: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Favorites {
    #[serde(default)]
    pub albums: Vec<FavoriteItem>,
    #[serde(default)]
    pub tracks: Vec<FavoriteItem>,
    #[serde(default)]
    pub artists: Vec<FavoriteItem>,
    #[serde(default)]
    pub labels: Vec<FavoriteItem>,
    #[serde(default)]
    pub awards: Vec<FavoriteItem>,
}

impl Favorites {
    pub fn of(&self, kind: FavoriteKind) -> &[FavoriteItem] {
        match kind {
            FavoriteKind::Albums => &self.albums,
            FavoriteKind::Tracks => &self.tracks,
            FavoriteKind::Artists => &self.artists,
            FavoriteKind::Labels => &self.labels,
            FavoriteKind::Awards => &self.awards,
        }
    }

    pub fn of_mut(&mut self, kind: FavoriteKind) -> &mut Vec<FavoriteItem> {
        match kind {
            FavoriteKind::Albums => &mut self.albums,
            FavoriteKind::Tracks => &mut self.tracks,
            FavoriteKind::Artists => &mut self.artists,
            FavoriteKind::Labels => &mut self.labels,
            FavoriteKind::Awards => &mut self.awards,
        }
    }

    pub fn ids(&self, kind: FavoriteKind) -> HashSet<&str> {
        self.of(kind).iter().map(|f| f.id.as_str()).collect()
    }

    pub fn total(&self) -> usize {
        FavoriteKind::ALL.iter().map(|k| self.of(*k).len()).sum()
    }
}

/// A playlist the account OWNS, with its track ids in order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnedPlaylist {
    pub id: u64,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default)]
    pub is_public: bool,
    #[serde(default)]
    pub track_ids: Vec<u64>,
}

/// A playlist the account FOLLOWS (someone else's). Same id on every
/// account, so it is re-subscribed, never copied.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubscribedPlaylist {
    pub id: u64,
    pub name: String,
    #[serde(default)]
    pub owner_id: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotSource {
    pub user_id: u64,
    #[serde(default)]
    pub display_name: String,
    /// Stored only to help the owner identify bundles belonging to accounts
    /// with similar display names. Older snapshots deserialize without it.
    #[serde(default)]
    pub email: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountSnapshot {
    pub kind: String,
    pub schema_version: u32,
    /// RFC 3339.
    pub created_at: String,
    pub source: SnapshotSource,
    #[serde(default)]
    pub favorites: Favorites,
    #[serde(default)]
    pub playlists: Vec<OwnedPlaylist>,
    #[serde(default)]
    pub subscriptions: Vec<SubscribedPlaylist>,
}

impl AccountSnapshot {
    pub fn empty(user_id: u64, display_name: &str) -> Self {
        Self {
            kind: KIND.to_string(),
            schema_version: SCHEMA_VERSION,
            created_at: chrono::Utc::now().to_rfc3339(),
            source: SnapshotSource {
                user_id,
                display_name: display_name.to_string(),
                email: String::new(),
            },
            favorites: Favorites::default(),
            playlists: Vec::new(),
            subscriptions: Vec::new(),
        }
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        let snap: AccountSnapshot =
            serde_json::from_str(text).map_err(|e| format!("not a QBZ account snapshot: {e}"))?;
        if snap.kind != KIND {
            return Err(format!("not a QBZ account snapshot (kind `{}`)", snap.kind));
        }
        if snap.schema_version > SCHEMA_VERSION {
            return Err(format!(
                "snapshot schema {} is newer than this QBZ understands ({})",
                snap.schema_version, SCHEMA_VERSION
            ));
        }
        Ok(snap)
    }

    pub fn read(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Self::parse(&text)
    }

    pub fn write(&self, path: &Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        let json = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(path, json).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// `created_at` in the machine's local time, `YYYY-MM-DD HH:MM`, for
    /// listings; the raw RFC 3339 string when it does not parse.
    pub fn created_at_local(&self) -> String {
        chrono::DateTime::parse_from_rfc3339(&self.created_at)
            .map(|t| {
                t.with_timezone(&chrono::Local)
                    .format("%Y-%m-%d %H:%M")
                    .to_string()
            })
            .unwrap_or_else(|_| self.created_at.clone())
    }

    pub fn owned_ids(&self) -> HashSet<u64> {
        self.playlists.iter().map(|p| p.id).collect()
    }

    pub fn subscribed_ids(&self) -> HashSet<u64> {
        self.subscriptions.iter().map(|p| p.id).collect()
    }
}

/// Where snapshots live inside a profile directory (`users/<uid>/`).
pub const SNAPSHOT_DIR: &str = "account_migration";

/// `<profile>/account_migration/snapshot-YYYYMMDD-HHMMSS.json`.
pub fn snapshot_path(profile_dir: &Path) -> PathBuf {
    profile_dir.join(SNAPSHOT_DIR).join(format!(
        "snapshot-{}.json",
        chrono::Utc::now().format("%Y%m%d-%H%M%S")
    ))
}

fn id_string(v: Option<&Value>) -> Option<String> {
    match v? {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn item_title(item: &Value) -> String {
    for key in ["title", "name"] {
        if let Some(s) = item.get(key).and_then(Value::as_str) {
            return s.to_string();
        }
    }
    String::new()
}

/// Page one favorites kind until exhausted (short page, `total` reached,
/// or the cap), preserving the server order in `server_index`.
async fn fetch_favorites<A: MigrationApi>(
    api: &A,
    kind: FavoriteKind,
) -> Result<Vec<FavoriteItem>, String> {
    let mut items: Vec<FavoriteItem> = Vec::new();
    let mut offset = 0u32;
    loop {
        let value = api.favorites_page(kind.plural(), PAGE_SIZE, offset).await?;
        let branch = value.get(kind.plural());
        let total = branch
            .and_then(|b| b.get("total"))
            .and_then(Value::as_u64)
            .unwrap_or(0) as usize;
        let page: Vec<Value> = branch
            .and_then(|b| b.get("items"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let page_len = page.len();
        for item in page {
            let Some(id) = id_string(item.get("id")) else {
                continue;
            };
            items.push(FavoriteItem {
                id,
                favorited_at: item.get("favorited_at").and_then(Value::as_i64),
                title: item_title(&item),
                server_index: items.len(),
            });
        }
        offset += page_len as u32;
        if page_len < PAGE_SIZE as usize
            || (total > 0 && offset as usize >= total)
            || items.len() >= MAX_ITEMS
        {
            break;
        }
    }
    Ok(items)
}

/// Capture the active session's account. `user_id` splits owned playlists
/// from followed ones (`owner.id == user_id`). A favorites kind that fails
/// (labels have 400'd for months at times) is logged and left empty; a
/// playlist whose track ids cannot be read is skipped with a log line —
/// the snapshot says what it holds, never guesses.
pub async fn capture<A: MigrationApi, S: MigrationSink>(
    api: &A,
    user_id: u64,
    display_name: &str,
    email: &str,
    sink: &S,
) -> Result<AccountSnapshot, String> {
    sink.emit(MigrationEvent::Phase(MigrationPhase::ReadingSource));
    let mut snap = AccountSnapshot::empty(user_id, display_name);
    snap.source.email = email.to_string();
    for (i, kind) in FavoriteKind::ALL.iter().enumerate() {
        sink.emit(MigrationEvent::Progress {
            done: i,
            total: FavoriteKind::ALL.len(),
            label: kind.plural().to_string(),
        });
        match fetch_favorites(api, *kind).await {
            Ok(items) => *snap.favorites.of_mut(*kind) = items,
            Err(e) => log::warn!(
                "[account-migration] favorites {} unreadable: {e}",
                kind.plural()
            ),
        }
    }
    let playlists = api.user_playlists().await?;
    let total = playlists.len();
    for (i, playlist) in playlists.into_iter().enumerate() {
        sink.emit(MigrationEvent::Progress {
            done: i,
            total,
            label: playlist.name.clone(),
        });
        if playlist.owner.id == user_id {
            match api.playlist_track_ids(playlist.id).await {
                Ok(with_ids) => snap.playlists.push(OwnedPlaylist {
                    id: playlist.id,
                    name: playlist.name,
                    description: playlist.description,
                    is_public: playlist.is_public,
                    track_ids: with_ids.track_ids,
                }),
                Err(e) => log::warn!(
                    "[account-migration] playlist {} ({}) unreadable, skipped: {e}",
                    playlist.id,
                    playlist.name
                ),
            }
        } else {
            snap.subscriptions.push(SubscribedPlaylist {
                id: playlist.id,
                name: playlist.name,
                owner_id: playlist.owner.id,
            });
        }
    }
    Ok(snap)
}

/// Every snapshot file under `<data_root>/users/*/account_migration/`,
/// newest first, with the profile (uid) it was found in.
pub fn list_snapshots(data_root: &Path) -> Vec<(u64, PathBuf)> {
    let mut found: Vec<(u64, PathBuf, std::time::SystemTime)> = Vec::new();
    let Ok(users) = std::fs::read_dir(data_root.join("users")) else {
        return Vec::new();
    };
    for user in users.flatten() {
        let Some(uid) = user
            .file_name()
            .to_str()
            .and_then(|s| s.parse::<u64>().ok())
        else {
            continue;
        };
        let Ok(files) = std::fs::read_dir(user.path().join(SNAPSHOT_DIR)) else {
            continue;
        };
        for file in files.flatten() {
            let path = file.path();
            let is_snapshot = path
                .file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with("snapshot-") && n.ends_with(".json"))
                .unwrap_or(false);
            if !is_snapshot {
                continue;
            }
            let modified = file
                .metadata()
                .and_then(|m| m.modified())
                .unwrap_or(std::time::UNIX_EPOCH);
            found.push((uid, path, modified));
        }
    }
    found.sort_by(|a, b| b.2.cmp(&a.2));
    found
        .into_iter()
        .map(|(uid, path, _)| (uid, path))
        .collect()
}

/// Delete one snapshot returned by [`list_snapshots`].
///
/// The exact-membership check is intentional: the UI passes a path as a
/// string, so accepting an arbitrary path (or merely checking its suffix)
/// would turn a cleanup button into a general file-delete primitive. This
/// removes only the snapshot bundle; the source profile and all of its other
/// user data remain untouched.
pub fn delete_snapshot(data_root: &Path, requested: &Path) -> Result<(), String> {
    let allowed = list_snapshots(data_root)
        .into_iter()
        .any(|(_, path)| path == requested);
    if !allowed {
        return Err(format!(
            "refusing to delete a path outside the migration snapshot list: {}",
            requested.display()
        ));
    }
    std::fs::remove_file(requested).map_err(|e| format!("{}: {e}", requested.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "qbz-account-migration-{label}-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ))
    }

    #[test]
    fn snapshots_created_before_email_identity_remain_readable() {
        let snapshot = AccountSnapshot::empty(42, "Old account");
        let mut value = serde_json::to_value(snapshot).unwrap();
        value["source"].as_object_mut().unwrap().remove("email");

        let parsed = AccountSnapshot::parse(&value.to_string()).unwrap();

        assert_eq!(parsed.source.display_name, "Old account");
        assert_eq!(parsed.source.user_id, 42);
        assert!(parsed.source.email.is_empty());
    }

    #[test]
    fn delete_snapshot_removes_only_a_listed_bundle() {
        let root = temp_root("delete");
        let dir = root.join("users/42").join(SNAPSHOT_DIR);
        std::fs::create_dir_all(&dir).unwrap();
        let snapshot = dir.join("snapshot-20260902-120000.json");
        let keep = root.join("users/42/profile.db");
        std::fs::write(&snapshot, "{}").unwrap();
        std::fs::write(&keep, "profile").unwrap();

        delete_snapshot(&root, &snapshot).unwrap();

        assert!(!snapshot.exists());
        assert!(keep.exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn delete_snapshot_refuses_an_unlisted_path() {
        let root = temp_root("refuse");
        let profile = root.join("users/42");
        std::fs::create_dir_all(&profile).unwrap();
        let keep = profile.join("profile.db");
        std::fs::write(&keep, "profile").unwrap();

        let error = delete_snapshot(&root, &keep).unwrap_err();

        assert!(error.contains("outside the migration snapshot list"));
        assert!(keep.exists());
        std::fs::remove_dir_all(root).unwrap();
    }
}
