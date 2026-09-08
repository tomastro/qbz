//! The ledger: what earlier runs already wrote into the TARGET account, kept
//! in the target's profile directory. Two jobs: resume/re-run to "0
//! changes", and the old→new playlist id map the local-profile copy needs
//! to remap everything keyed by `qobuz_playlist_id`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::snapshot::SNAPSHOT_DIR;

pub const LEDGER_FILE: &str = "ledger.json";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ledger {
    /// Source playlist id → target playlist id (created, merged into, or
    /// re-subscribed; the last two map an id onto itself or the merge
    /// target).
    #[serde(default)]
    pub playlist_map: BTreeMap<u64, u64>,
    /// Favorite ids already written, per kind (`albums`, `tracks`, ...).
    #[serde(default)]
    pub favorites_done: BTreeMap<String, BTreeSet<String>>,
    #[serde(default)]
    pub subscriptions_done: BTreeSet<u64>,
    /// Source account ids this ledger has seen, for the report.
    #[serde(default)]
    pub sources: BTreeSet<u64>,
}

impl Ledger {
    pub fn path(profile_dir: &Path) -> PathBuf {
        profile_dir.join(SNAPSHOT_DIR).join(LEDGER_FILE)
    }

    /// Missing file = empty ledger; a corrupt file is an error, never
    /// silently a fresh start (that would re-write everything).
    pub fn load(profile_dir: &Path) -> Result<Self, String> {
        let path = Self::path(profile_dir);
        match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text)
                .map_err(|e| format!("ledger {} is corrupt: {e}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(format!("{}: {e}", path.display())),
        }
    }

    pub fn save(&self, profile_dir: &Path) -> Result<(), String> {
        let path = Self::path(profile_dir);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        let json = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(&path, json).map_err(|e| format!("{}: {e}", path.display()))
    }

    pub fn favorite_done(&self, plural: &str, id: &str) -> bool {
        self.favorites_done
            .get(plural)
            .map(|set| set.contains(id))
            .unwrap_or(false)
    }

    pub fn mark_favorite(&mut self, plural: &str, id: &str) {
        self.favorites_done
            .entry(plural.to_string())
            .or_default()
            .insert(id.to_string());
    }
}
