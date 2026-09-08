//! Discover per-tab section preferences (the "configurator" model).
//!
//! Frontend-agnostic port of the Tauri `discovery-v2/sectionPrefs.ts` store
//! (ADR-006): the ordered, per-tab list of `{ id, enabled }` that drives which
//! Discover rows show and in what order on each of the three tabs
//! (Home / Editor's Picks / For You). All tabs render from the SAME fetched
//! data — a tab is just a curated, ordered subset.
//!
//! Persistence is a single JSON blob in a per-user SQLite database
//! (`<base>/discover_prefs.db`), mirroring the other per-user settings stores
//! in this module. The blob shape is identical to the Tauri localStorage value
//! (`{ "home": [{id,enabled}], "editorPicks": [...], "forYou": [...] }`), so the
//! migration/reconcile logic ports verbatim and a profile could be shared.
//!
//! The model logic (defaults, migrate, reconcile, toggle, move, reset) is PURE
//! and headless-testable; the store is a thin wrapper.

use rusqlite::{params, Connection};
use serde_json::{json, Value};
use std::path::Path;
use std::sync::{Arc, Mutex};

// ---------------------------------------------------------------------------
// Tabs
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DiscoveryTab {
    Home,
    EditorPicks,
    ForYou,
}

impl DiscoveryTab {
    /// JSON / persistence key (matches the Tauri localStorage object keys).
    pub fn as_key(&self) -> &'static str {
        match self {
            DiscoveryTab::Home => "home",
            DiscoveryTab::EditorPicks => "editorPicks",
            DiscoveryTab::ForYou => "forYou",
        }
    }

    pub fn from_key(s: &str) -> Option<Self> {
        match s {
            "home" => Some(DiscoveryTab::Home),
            "editorPicks" => Some(DiscoveryTab::EditorPicks),
            "forYou" => Some(DiscoveryTab::ForYou),
            _ => None,
        }
    }

    pub const ALL: [DiscoveryTab; 3] =
        [DiscoveryTab::Home, DiscoveryTab::EditorPicks, DiscoveryTab::ForYou];
}

// ---------------------------------------------------------------------------
// Section ids
// ---------------------------------------------------------------------------

/// The `DiscoverySectionId` union: the 19 Tauri members (`sectionPrefs.ts`)
/// plus the Slint-era `Pinned` (user-pinned albums/artists/playlists — no
/// Tauri counterpart) and the local `MostPlayedAlbums` (top albums by local
/// play count). `editorPicks` is BOTH a tab and a section id (the "Albums
/// of the Week" section).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DiscoverySectionId {
    NewReleases,
    PressAwards,
    QobuzPlaylists,
    RecentlyPlayedAlbums,
    ContinueListening,
    IdealDiscography,
    MostStreamed,
    ReleaseWatch,
    EditorPicks,
    Qobuzissimes,
    TopArtists,
    FavoriteAlbums,
    QobuzMixes,
    RadioStations,
    SimilarAlbums,
    RediscoverLibrary,
    EssentialsByGenre,
    ArtistsToFollow,
    ArtistSpotlight,
    Pinned,
    /// "Most Played Albums" — top albums by local play count
    /// (`qbz_app::settings::album_play_history`). Home + For You, default off.
    MostPlayedAlbums,
    /// "Recently Played Playlists" — the playlists the user actually chose to
    /// play, by last play (`qbz_app::settings::playlist_play_history`).
    ///
    /// Its sibling `RecentlyPlayedAlbums` used to absorb these: every track
    /// start wrote its ALBUM into the recent list whatever the user had put on,
    /// so playing one 40-track playlist filled a 24-entry album rail with that
    /// playlist's contents. Plays are separated by context now, and this is
    /// where the playlist half surfaces. Home + For You, default ON — it is the
    /// answer to a question the album rail was silently answering wrong.
    RecentlyPlayedPlaylists,
}

impl DiscoverySectionId {
    pub fn as_str(&self) -> &'static str {
        use DiscoverySectionId::*;
        match self {
            NewReleases => "newReleases",
            PressAwards => "pressAwards",
            QobuzPlaylists => "qobuzPlaylists",
            RecentlyPlayedAlbums => "recentlyPlayedAlbums",
            ContinueListening => "continueListening",
            IdealDiscography => "idealDiscography",
            MostStreamed => "mostStreamed",
            ReleaseWatch => "releaseWatch",
            EditorPicks => "editorPicks",
            Qobuzissimes => "qobuzissimes",
            TopArtists => "topArtists",
            FavoriteAlbums => "favoriteAlbums",
            QobuzMixes => "qobuzMixes",
            RadioStations => "radioStations",
            SimilarAlbums => "similarAlbums",
            RediscoverLibrary => "rediscoverLibrary",
            EssentialsByGenre => "essentialsByGenre",
            ArtistsToFollow => "artistsToFollow",
            ArtistSpotlight => "artistSpotlight",
            Pinned => "pinned",
            MostPlayedAlbums => "mostPlayedAlbums",
            RecentlyPlayedPlaylists => "recentlyPlayedPlaylists",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        use DiscoverySectionId::*;
        Some(match s {
            "newReleases" => NewReleases,
            "pressAwards" => PressAwards,
            "qobuzPlaylists" => QobuzPlaylists,
            "recentlyPlayedAlbums" => RecentlyPlayedAlbums,
            "continueListening" => ContinueListening,
            "idealDiscography" => IdealDiscography,
            "mostStreamed" => MostStreamed,
            "releaseWatch" => ReleaseWatch,
            "editorPicks" => EditorPicks,
            "qobuzissimes" => Qobuzissimes,
            "topArtists" => TopArtists,
            "favoriteAlbums" => FavoriteAlbums,
            "qobuzMixes" => QobuzMixes,
            "radioStations" => RadioStations,
            "similarAlbums" => SimilarAlbums,
            "rediscoverLibrary" => RediscoverLibrary,
            "essentialsByGenre" => EssentialsByGenre,
            "artistsToFollow" => ArtistsToFollow,
            "artistSpotlight" => ArtistSpotlight,
            "pinned" => Pinned,
            "mostPlayedAlbums" => MostPlayedAlbums,
            "recentlyPlayedPlaylists" => RecentlyPlayedPlaylists,
            _ => return None,
        })
    }
}

// ---------------------------------------------------------------------------
// Prefs model
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SectionPref {
    pub id: DiscoverySectionId,
    pub enabled: bool,
}

const fn pref(id: DiscoverySectionId, enabled: bool) -> SectionPref {
    SectionPref { id, enabled }
}

/// The per-tab ordered preference lists. Field order is irrelevant; the Vec
/// order within each tab is the render order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoverPrefs {
    pub home: Vec<SectionPref>,
    pub editor_picks: Vec<SectionPref>,
    pub for_you: Vec<SectionPref>,
    /// Opt-out: show the external "Recommendations" tab in Discover. Default on.
    pub show_recommendations: bool,
    /// Recommendations results-cache window, in hours. One of {24,36,48,72};
    /// drives how long the built reco rows are served from cache before a
    /// rebuild. Default 48.
    pub reco_cache_ttl_hours: i64,
}

/// The exact `DEFAULT_PREFS` from `sectionPrefs.ts`.
pub fn default_prefs() -> DiscoverPrefs {
    use DiscoverySectionId::*;
    DiscoverPrefs {
        // home: first 8 ON, the rest OFF (the Tauri sectionPrefs.ts:63-77
        // list plus the Slint-era `pinned` — enabled by default; its arm
        // self-hides while the user has no pins — and the local
        // `mostPlayedAlbums` addition, default off). All 13 Tauri ids render
        // on Home since #566 completed the port: qobuzMixes / releaseWatch /
        // topArtists / favoriteAlbums were genuine Tauri-Home sections whose
        // Slint render arms + data pipelines were missing.
        home: vec![
            pref(NewReleases, true),
            pref(PressAwards, true),
            pref(QobuzPlaylists, true),
            pref(RecentlyPlayedAlbums, true),
            pref(RecentlyPlayedPlaylists, true),
            pref(ContinueListening, true),
            pref(IdealDiscography, true),
            pref(MostStreamed, true),
            pref(Pinned, true),
            pref(QobuzMixes, false),
            pref(ReleaseWatch, false),
            pref(EditorPicks, false),
            pref(Qobuzissimes, false),
            pref(TopArtists, false),
            pref(FavoriteAlbums, false),
            pref(MostPlayedAlbums, false),
        ],
        // editorPicks: all ON.
        editor_picks: vec![
            pref(NewReleases, true),
            pref(EditorPicks, true),
            pref(Qobuzissimes, true),
            pref(PressAwards, true),
            pref(MostStreamed, true),
            pref(IdealDiscography, true),
            pref(QobuzPlaylists, true),
        ],
        // forYou: all ON, qobuzMixes first, pinned right after (near the top —
        // it is the user's own curation; self-hides while empty).
        for_you: vec![
            pref(QobuzMixes, true),
            pref(Pinned, true),
            pref(ReleaseWatch, true),
            pref(RadioStations, true),
            pref(ContinueListening, true),
            pref(RecentlyPlayedAlbums, true),
            pref(RecentlyPlayedPlaylists, true),
            pref(TopArtists, true),
            pref(FavoriteAlbums, true),
            pref(SimilarAlbums, true),
            pref(RediscoverLibrary, true),
            pref(EssentialsByGenre, true),
            pref(ArtistsToFollow, true),
            pref(ArtistSpotlight, true),
            pref(MostPlayedAlbums, false),
        ],
        show_recommendations: true,
        reco_cache_ttl_hours: 48,
    }
}

impl DiscoverPrefs {
    pub fn tab(&self, tab: DiscoveryTab) -> &Vec<SectionPref> {
        match tab {
            DiscoveryTab::Home => &self.home,
            DiscoveryTab::EditorPicks => &self.editor_picks,
            DiscoveryTab::ForYou => &self.for_you,
        }
    }

    pub fn tab_mut(&mut self, tab: DiscoveryTab) -> &mut Vec<SectionPref> {
        match tab {
            DiscoveryTab::Home => &mut self.home,
            DiscoveryTab::EditorPicks => &mut self.editor_picks,
            DiscoveryTab::ForYou => &mut self.for_you,
        }
    }

    /// Flip `enabled` on the matching id. No minimum-enabled guard (can reach 0).
    pub fn toggle(&mut self, tab: DiscoveryTab, id: DiscoverySectionId) {
        if let Some(p) = self.tab_mut(tab).iter_mut().find(|p| p.id == id) {
            p.enabled = !p.enabled;
        }
    }

    /// Move a section one step (`dir` = -1 up / +1 down) with boundary clamp.
    /// No-op if the id is absent or already at the boundary. The `enabled`
    /// flag travels with the entry (the whole `SectionPref` is swapped).
    pub fn move_section(&mut self, tab: DiscoveryTab, id: DiscoverySectionId, dir: i8) {
        let list = self.tab_mut(tab);
        let Some(idx) = list.iter().position(|p| p.id == id) else {
            return;
        };
        if dir < 0 && idx > 0 {
            list.swap(idx, idx - 1);
        } else if dir > 0 && idx + 1 < list.len() {
            list.swap(idx, idx + 1);
        }
    }

    /// Replace one tab's list with a FRESH clone of its defaults.
    pub fn reset_tab(&mut self, tab: DiscoveryTab) {
        let defaults = default_prefs();
        *self.tab_mut(tab) = defaults.tab(tab).clone();
    }

    pub fn is_enabled(&self, tab: DiscoveryTab, id: DiscoverySectionId) -> bool {
        self.tab(tab)
            .iter()
            .find(|p| p.id == id)
            .map(|p| p.enabled)
            .unwrap_or(false)
    }

    pub fn enabled_count(&self, tab: DiscoveryTab) -> usize {
        self.tab(tab).iter().filter(|p| p.enabled).count()
    }

    /// The ordered list of ENABLED section ids for a tab — drives the render
    /// loop in the frontend.
    pub fn enabled_ordered(&self, tab: DiscoveryTab) -> Vec<DiscoverySectionId> {
        self.tab(tab)
            .iter()
            .filter(|p| p.enabled)
            .map(|p| p.id)
            .collect()
    }

    /// The set of section ids a tab offers (= the ids in its DEFAULT order).
    /// The configurator only ever shows these for the tab.
    pub fn available_ids(tab: DiscoveryTab) -> Vec<DiscoverySectionId> {
        default_prefs().tab(tab).iter().map(|p| p.id).collect()
    }

    // ---- JSON (persistence + migration) ----

    /// Serialize to the Tauri-compatible by-tab JSON object.
    pub fn to_json(&self) -> Value {
        let arr = |list: &[SectionPref]| -> Value {
            Value::Array(
                list.iter()
                    .map(|p| json!({ "id": p.id.as_str(), "enabled": p.enabled }))
                    .collect(),
            )
        };
        json!({
            "home": arr(&self.home),
            "editorPicks": arr(&self.editor_picks),
            "forYou": arr(&self.for_you),
            "showRecommendations": self.show_recommendations,
            "recoCacheTtlHours": self.reco_cache_ttl_hours,
        })
    }

    /// Migrate any persisted value into a complete, valid `DiscoverPrefs`.
    /// Three branches, IN ORDER (mirrors `migrate` in `sectionPrefs.ts`):
    ///   1. Array  -> legacy V1 home-only: reconcile as Home; the other two
    ///      tabs get raw defaults.
    ///   2. Object -> reconcile each of the 3 tabs against its defaults
    ///      (a missing tab key reconciles to that tab's defaults).
    ///   3. Anything else (null / number / string / parse failure upstream)
    ///      -> full defaults.
    pub fn migrate(value: &Value) -> DiscoverPrefs {
        let defaults = default_prefs();
        if let Some(arr) = value.as_array() {
            DiscoverPrefs {
                home: reconcile_list(Some(arr), &defaults.home),
                editor_picks: defaults.editor_picks,
                for_you: defaults.for_you,
                // Legacy V1 (home-only array) predates the flag -> default on.
                show_recommendations: true,
                // Legacy V1 predates the cache-window setting -> default 48h.
                reco_cache_ttl_hours: 48,
            }
        } else if value.is_object() {
            let get = |key: &str| value.get(key).and_then(|v| v.as_array());
            DiscoverPrefs {
                home: reconcile_list(get("home"), &defaults.home),
                editor_picks: reconcile_list(get("editorPicks"), &defaults.editor_picks),
                for_you: reconcile_list(get("forYou"), &defaults.for_you),
                // Missing key (older persisted blob) -> default on.
                show_recommendations: value
                    .get("showRecommendations")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true),
                // Validate against the offered set; unknown / missing -> 48h.
                reco_cache_ttl_hours: value
                    .get("recoCacheTtlHours")
                    .and_then(|v| v.as_i64())
                    .filter(|h| [24, 36, 48, 72].contains(h))
                    .unwrap_or(48),
            }
        } else {
            defaults
        }
    }
}

/// Reconcile a persisted per-tab array against the tab's fallback defaults.
///
/// Output = (valid persisted ids in stored order) ++ (default ids not seen,
/// appended in default order). An entry is kept only if it is an object with a
/// string `id` that (a) maps to a known section, (b) is in the fallback id set,
/// and (c) has not already been seen (first-occurrence wins). `enabled` is
/// coerced to a strict bool (missing / non-bool -> false).
pub fn reconcile_list(persisted: Option<&Vec<Value>>, fallback: &[SectionPref]) -> Vec<SectionPref> {
    let Some(arr) = persisted else {
        return fallback.to_vec();
    };
    let allowed: std::collections::HashSet<DiscoverySectionId> =
        fallback.iter().map(|p| p.id).collect();

    let mut seen: std::collections::HashSet<DiscoverySectionId> = std::collections::HashSet::new();
    let mut out: Vec<SectionPref> = Vec::new();
    for entry in arr {
        let Some(obj) = entry.as_object() else {
            continue;
        };
        let Some(id) = obj.get("id").and_then(|v| v.as_str()).and_then(DiscoverySectionId::from_str)
        else {
            continue;
        };
        if !allowed.contains(&id) || seen.contains(&id) {
            continue;
        }
        let enabled = obj.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false);
        seen.insert(id);
        out.push(SectionPref { id, enabled });
    }
    // Append every fallback entry whose id was not seen, in fallback order.
    for p in fallback {
        if !seen.contains(&p.id) {
            out.push(*p);
        }
    }
    out
}

// ---------------------------------------------------------------------------
// SQLite store
// ---------------------------------------------------------------------------

/// The rail sizes the UI offers, IN THE ORDER THE UI OFFERS THEM — the selector
/// sends back an index into this array. Reordering is safe ONLY because what is
/// STORED is the size and never the index (`save_rail_size` takes the count,
/// `rail_size_index_of` derives the index back), so a stored 15 keeps meaning
/// 15 whatever position it sits in. Keep that invariant or this comment becomes
/// a trap.
///
/// There is no "uncapped" entry, and that is deliberate. It used to read "All",
/// which was a lie twice over: an "all" in Qobuz terms is what the View-all
/// page holds, and /discover/index hands each rail a DIFFERENT number anyway —
/// measured on a live response, new_releases 26, ideal_discography 26,
/// playlists 20, most_streamed 15, and qobuzissims / album_of_the_week /
/// press_awards 10. One label could not be honest about all of them. So the
/// default is now a real, uniform cap of ten and the entry says so.
pub const RAIL_SIZE_PRESETS: [i64; 4] = [10, 15, 20, 25];

pub const DEFAULT_RAIL_SIZE: i64 = 0;

pub struct DiscoverPrefsStore {
    conn: Connection,
}

impl DiscoverPrefsStore {
    fn open_at(dir: &Path, db_name: &str) -> Result<Self, String> {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("Failed to create data directory: {}", e))?;

        let db_path = dir.join(db_name);
        let conn = Connection::open(&db_path)
            .map_err(|e| format!("Failed to open discover prefs database: {}", e))?;

        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA busy_timeout=1000;")
            .map_err(|e| format!("Failed to enable WAL for discover prefs database: {}", e))?;

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS discover_prefs (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                prefs_json TEXT NOT NULL
            );",
        )
        .map_err(|e| format!("Failed to create discover prefs table: {}", e))?;

        conn.execute(
            "INSERT OR IGNORE INTO discover_prefs (id, prefs_json) VALUES (1, ?1)",
            params![default_prefs().to_json().to_string()],
        )
        .map_err(|e| format!("Failed to initialize discover prefs: {}", e))?;

        // Rail size lives in its OWN TABLE, not as a key inside `prefs_json`,
        // and that is a data-safety decision rather than a stylistic one.
        //
        // `DiscoverPrefs::to_json` rebuilds the blob FROM SCRATCH out of the
        // keys the running binary knows, and `save` overwrites the whole
        // column with it. This .db is SHARED with the Slint build, whose tree
        // is frozen and will not be recompiled — so the installed Slint binary
        // has the old `to_json` permanently. A new key inside the blob would
        // therefore survive exactly until the user touched any toggle over
        // there, and then vanish, with nothing logged anywhere.
        //
        // A separate table cannot be hit that way: the old binary never names
        // it in any statement, and `CREATE TABLE IF NOT EXISTS` is idempotent
        // in whichever build opens the file first. WAL + busy_timeout are
        // already set above, so two binaries on one file is the existing
        // design, not something this adds.
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS discover_rail_sizes (
                section_id TEXT PRIMARY KEY,
                rail_size  INTEGER NOT NULL
            );",
        )
        .map_err(|e| format!("Failed to create discover rail size table: {}", e))?;
        // No seed row: ABSENT means the default, which is uncapped. Writing a
        // row per section at open would put twenty rows in the file to say
        // "unchanged", and would then have to be kept in step with the enum.

        Ok(Self { conn })
    }

    pub fn new() -> Result<Self, String> {
        let data_dir = dirs::data_dir()
            .ok_or("Could not determine data directory")?
            .join("qbz");
        Self::open_at(&data_dir, "discover_prefs.db")
    }

    /// Open the store in a specific (per-user) base directory.
    pub fn new_at(base_dir: &Path) -> Result<Self, String> {
        Self::open_at(base_dir, "discover_prefs.db")
    }

    /// Load and migrate the persisted prefs. A missing / corrupt / unparseable
    /// blob yields defaults (never an error to the caller).
    pub fn load(&self) -> DiscoverPrefs {
        let raw: Result<String, _> = self.conn.query_row(
            "SELECT prefs_json FROM discover_prefs WHERE id = 1",
            [],
            |row| row.get(0),
        );
        match raw {
            Ok(text) => match serde_json::from_str::<Value>(&text) {
                Ok(value) => DiscoverPrefs::migrate(&value),
                Err(_) => default_prefs(),
            },
            Err(_) => default_prefs(),
        }
    }

    /// How many items each Discover rail may show, by section id. `0` = no
    /// cap, and a section ABSENT from the map is uncapped too.
    ///
    /// Never an error to the caller: a missing table (an older file) or a value
    /// outside the presets yields an empty map / drops that row.
    pub fn load_rail_sizes(&self) -> std::collections::HashMap<String, i64> {
        let out = (|| -> Option<std::collections::HashMap<String, i64>> {
            let mut stmt = self
                .conn
                .prepare("SELECT section_id, rail_size FROM discover_rail_sizes")
                .ok()?;
            let rows = stmt
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
                })
                .ok()?;
            Some(
                rows.flatten()
                    .filter(|(_, n)| RAIL_SIZE_PRESETS.contains(n))
                    .collect(),
            )
        })();
        out.unwrap_or_default()
    }

    /// Persist ONE rail's size. Values outside the presets are rejected rather
    /// than stored, so what loads back and the UI's index can never disagree.
    /// The DEFAULT deletes the row instead of storing it — the file then only
    /// carries the rails the user actually changed.
    pub fn save_rail_size(&self, section_id: &str, n: i64) -> Result<(), String> {
        if !RAIL_SIZE_PRESETS.contains(&n) {
            return Err(format!("rail size {n} is not one of {RAIL_SIZE_PRESETS:?}"));
        }
        let sql = if n == DEFAULT_RAIL_SIZE {
            self.conn.execute(
                "DELETE FROM discover_rail_sizes WHERE section_id = ?1",
                params![section_id],
            )
        } else {
            self.conn.execute(
                "INSERT INTO discover_rail_sizes (section_id, rail_size) VALUES (?1, ?2)
                 ON CONFLICT(section_id) DO UPDATE SET rail_size = excluded.rail_size",
                params![section_id, n],
            )
        };
        sql.map_err(|e| format!("Failed to save discover rail size: {}", e))?;
        Ok(())
    }

    /// Persist the whole prefs blob (upsert row 1).
    pub fn save(&self, prefs: &DiscoverPrefs) -> Result<(), String> {
        self.conn
            .execute(
                "INSERT INTO discover_prefs (id, prefs_json) VALUES (1, ?1)
                 ON CONFLICT(id) DO UPDATE SET prefs_json = excluded.prefs_json",
                params![prefs.to_json().to_string()],
            )
            .map_err(|e| format!("Failed to save discover prefs: {}", e))?;
        Ok(())
    }
}

pub type DiscoverPrefsState = Arc<Mutex<Option<DiscoverPrefsStore>>>;

pub fn create_empty_discover_prefs_state() -> DiscoverPrefsState {
    Arc::new(Mutex::new(None))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use DiscoverySectionId::*;

    fn ids(list: &[SectionPref]) -> Vec<DiscoverySectionId> {
        list.iter().map(|p| p.id).collect()
    }

    fn unique_test_dir(name: &str) -> std::path::PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("qbz-app-{name}-{}-{nonce}", std::process::id()))
    }

    // --- Group 1: default ordering + enabled flags ---

    #[test]
    fn defaults_match_spec_exactly() {
        let d = default_prefs();
        // home: 16 entries, first 9 ON (Tauri sectionPrefs.ts + Slint `pinned`
        // + the local mostPlayedAlbums, default off, + recentlyPlayedPlaylists
        // sitting right after its album sibling, default on).
        assert_eq!(
            ids(&d.home),
            vec![
                NewReleases, PressAwards, QobuzPlaylists, RecentlyPlayedAlbums,
                RecentlyPlayedPlaylists,
                ContinueListening, IdealDiscography, MostStreamed, Pinned,
                QobuzMixes, ReleaseWatch, EditorPicks, Qobuzissimes, TopArtists,
                FavoriteAlbums, MostPlayedAlbums,
            ]
        );
        assert_eq!(d.enabled_count(DiscoveryTab::Home), 9);
        assert!(d.is_enabled(DiscoveryTab::Home, MostStreamed));
        assert!(!d.is_enabled(DiscoveryTab::Home, Qobuzissimes));
        // editorPicks: 7 entries, all ON.
        assert_eq!(
            ids(&d.editor_picks),
            vec![NewReleases, EditorPicks, Qobuzissimes, PressAwards, MostStreamed, IdealDiscography, QobuzPlaylists]
        );
        assert_eq!(d.enabled_count(DiscoveryTab::EditorPicks), 7);
        // forYou: 15 entries, qobuzMixes first, pinned second; the 13
        // Tauri+Slint ones plus recentlyPlayedPlaylists ON, mostPlayedAlbums
        // (local addition) OFF.
        assert_eq!(d.for_you.len(), 15);
        assert_eq!(d.for_you[0].id, QobuzMixes);
        assert_eq!(d.for_you[1].id, Pinned);
        assert_eq!(d.for_you[6].id, RecentlyPlayedPlaylists);
        assert_eq!(d.for_you[14].id, MostPlayedAlbums);
        assert_eq!(d.enabled_count(DiscoveryTab::ForYou), 14);
    }

    // --- Group 2: reconcile_list ---

    #[test]
    fn reconcile_none_returns_fallback() {
        let fb = default_prefs().home;
        assert_eq!(reconcile_list(None, &fb), fb);
    }

    #[test]
    fn reconcile_drops_unknown_dedupes_coerces_and_appends_missing() {
        let fb = default_prefs().home;
        // Persisted: a reordered + partial list with an unknown id, a dupe,
        // a non-bool enabled, and an id not valid for this tab.
        let persisted = vec![
            json!({ "id": "mostStreamed", "enabled": false }),
            json!({ "id": "totallyUnknown", "enabled": true }),
            json!({ "id": "newReleases", "enabled": true }),
            json!({ "id": "newReleases", "enabled": false }), // dupe -> dropped
            json!({ "id": "radioStations", "enabled": true }), // not in home defaults -> dropped
            json!({ "id": "pressAwards" }),                    // missing enabled -> false
        ];
        let out = reconcile_list(Some(&persisted), &fb);
        // Order: valid persisted first (mostStreamed, newReleases, pressAwards),
        // then the remaining home defaults in default order.
        assert_eq!(out[0], SectionPref { id: MostStreamed, enabled: false });
        assert_eq!(out[1], SectionPref { id: NewReleases, enabled: true });
        assert_eq!(out[2], SectionPref { id: PressAwards, enabled: false });
        // No unknown / cross-tab id leaked in.
        assert!(!ids(&out).contains(&RadioStations));
        // Every home default id is present exactly once.
        let mut got = ids(&out);
        got.sort_by_key(|i| i.as_str());
        let mut want = ids(&fb);
        want.sort_by_key(|i| i.as_str());
        assert_eq!(got, want);
        assert_eq!(out.len(), fb.len());
    }

    // --- Group 3: migrate (3 branches) ---

    #[test]
    fn migrate_legacy_array_is_home_only() {
        let legacy = json!([
            { "id": "qobuzPlaylists", "enabled": false },
            { "id": "newReleases", "enabled": true },
        ]);
        let m = DiscoverPrefs::migrate(&legacy);
        // Home reconciled from the array (qobuzPlaylists first, disabled).
        assert_eq!(m.home[0], SectionPref { id: QobuzPlaylists, enabled: false });
        assert_eq!(m.home[1], SectionPref { id: NewReleases, enabled: true });
        // The other two tabs are raw defaults.
        assert_eq!(m.editor_picks, default_prefs().editor_picks);
        assert_eq!(m.for_you, default_prefs().for_you);
    }

    #[test]
    fn migrate_object_reconciles_all_three_tabs() {
        let obj = json!({
            "home": [{ "id": "newReleases", "enabled": false }],
            // editorPicks missing -> defaults; forYou present but empty -> defaults appended.
            "forYou": [],
        });
        let m = DiscoverPrefs::migrate(&obj);
        assert_eq!(m.home[0], SectionPref { id: NewReleases, enabled: false });
        assert_eq!(m.home.len(), default_prefs().home.len()); // missing appended
        assert_eq!(m.editor_picks, default_prefs().editor_picks);
        assert_eq!(m.for_you, default_prefs().for_you); // empty array -> all defaults appended
    }

    #[test]
    fn migrate_garbage_returns_defaults() {
        assert_eq!(DiscoverPrefs::migrate(&json!(null)), default_prefs());
        assert_eq!(DiscoverPrefs::migrate(&json!(42)), default_prefs());
        assert_eq!(DiscoverPrefs::migrate(&json!("nope")), default_prefs());
    }

    // --- Group 4: move_section ---

    #[test]
    fn move_section_clamps_and_carries_enabled() {
        let mut d = default_prefs();
        // Up at index 0 is a no-op.
        d.move_section(DiscoveryTab::Home, NewReleases, -1);
        assert_eq!(d.home[0].id, NewReleases);
        // Down at last index is a no-op.
        let last = d.home.last().unwrap().id;
        d.move_section(DiscoveryTab::Home, last, 1);
        assert_eq!(d.home.last().unwrap().id, last);
        // Moving pressAwards (idx 1, enabled) up swaps with newReleases; enabled travels.
        d.move_section(DiscoveryTab::Home, PressAwards, -1);
        assert_eq!(d.home[0], SectionPref { id: PressAwards, enabled: true });
        assert_eq!(d.home[1], SectionPref { id: NewReleases, enabled: true });
        // Unknown id for the tab is a no-op (radioStations not in home).
        let before = d.home.clone();
        d.move_section(DiscoveryTab::Home, RadioStations, 1);
        assert_eq!(d.home, before);
    }

    // --- Group 5: toggle (no floor) ---

    #[test]
    fn toggle_can_reach_zero_enabled() {
        let mut d = default_prefs();
        for p in d.editor_picks.clone() {
            d.toggle(DiscoveryTab::EditorPicks, p.id);
        }
        assert_eq!(d.enabled_count(DiscoveryTab::EditorPicks), 0);
        // Toggling back on works too.
        d.toggle(DiscoveryTab::EditorPicks, NewReleases);
        assert!(d.is_enabled(DiscoveryTab::EditorPicks, NewReleases));
    }

    #[test]
    fn reset_tab_restores_defaults_only_for_that_tab() {
        let mut d = default_prefs();
        d.toggle(DiscoveryTab::Home, NewReleases);
        d.toggle(DiscoveryTab::ForYou, QobuzMixes);
        d.reset_tab(DiscoveryTab::Home);
        assert_eq!(d.home, default_prefs().home);
        // ForYou untouched by the Home reset.
        assert!(!d.is_enabled(DiscoveryTab::ForYou, QobuzMixes));
    }

    // --- Group 6: store round-trip + corruption ---

    #[test]
    fn store_roundtrip_and_corruption_recovery() {
        let dir = unique_test_dir("discover-prefs");
        {
            let store = DiscoverPrefsStore::new_at(&dir).expect("open store");
            // Fresh store returns defaults.
            assert_eq!(store.load(), default_prefs());
            // Mutate + save.
            let mut prefs = store.load();
            prefs.toggle(DiscoveryTab::Home, QobuzPlaylists);
            prefs.move_section(DiscoveryTab::Home, MostStreamed, -1);
            store.save(&prefs).expect("save");
            // Same-handle load is identity.
            assert_eq!(store.load(), prefs);
        }
        // Reopen -> persisted survives.
        {
            let store = DiscoverPrefsStore::new_at(&dir).expect("reopen");
            let prefs = store.load();
            assert!(!prefs.is_enabled(DiscoveryTab::Home, QobuzPlaylists));
            assert_eq!(prefs.home[0].id, NewReleases); // mostStreamed moved up from idx 6 to 5, not to 0
        }
        // Corrupt the blob -> load recovers to defaults.
        {
            let store = DiscoverPrefsStore::new_at(&dir).expect("reopen2");
            store
                .conn
                .execute(
                    "UPDATE discover_prefs SET prefs_json = ?1 WHERE id = 1",
                    params!["{not valid json"],
                )
                .expect("corrupt");
            assert_eq!(store.load(), default_prefs());
        }
        let _ = std::fs::remove_dir_all(dir);
    }
}
