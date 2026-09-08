//! The local half: copy a QBZ profile directory (`users/<old>/`) into
//! another (`users/<new>/`), row by row, remapping everything keyed by a
//! Qobuz playlist id through the [`Ledger`]'s old→new map.
//!
//! Rules (contract §2.2, §3.7):
//! - the SOURCE is opened read-only and never modified;
//! - the DESTINATION is never replaced wholesale: rows are inserted with
//!   `INSERT OR IGNORE` (collections) or `INSERT OR REPLACE` (single-row
//!   preference tables), files only when absent;
//! - anything keyed by `qobuz_playlist_id` is remapped; a row whose old id
//!   the ledger does not know is skipped and counted;
//! - the caches of the server's truth, the compliance clock, the session
//!   and the derived catalog generations are never copied.
//!
//! Copies are by column intersection (`PRAGMA table_info` on both sides),
//! so a schema that grew a column on one side still copies what both have.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use rusqlite::types::Value;
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};

use crate::ledger::Ledger;

/// What the user opted into beyond settings and library. All default on;
/// the panel asks (owner decision 2026-09-02).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalOptions {
    /// Plex / Jellyfin / Subsonic connections (carry credentials).
    pub media_servers: bool,
    /// Last.fm / ListenBrainz accounts (carry credentials).
    pub scrobblers: bool,
    /// Listen log and recommendation events.
    pub listening_history: bool,
}

impl Default for LocalOptions {
    fn default() -> Self {
        Self {
            media_servers: true,
            scrobblers: true,
            listening_history: true,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct LocalReport {
    /// `file/table` → rows inserted.
    pub copied: BTreeMap<String, usize>,
    /// Rows keyed by a playlist id the ledger does not map.
    pub unmapped_playlist_rows: usize,
    /// The destination already had library folders: only missing folders
    /// were added; tracks and per-track links were not copied.
    pub needs_rescan: bool,
    /// Meaningful Plex / Jellyfin / Subsonic connection records present in
    /// the source stores. Default placeholder rows are deliberately excluded.
    pub media_connections_found: usize,
    /// Source connection records whose containing store copied successfully.
    pub media_connections_copied: usize,
    /// Human-readable notes (missing source files, skipped tables).
    pub notes: Vec<String>,
}

impl LocalReport {
    pub fn total_rows(&self) -> usize {
        self.copied.values().sum()
    }

    fn add(&mut self, key: impl Into<String>, n: usize) {
        if n > 0 {
            *self.copied.entry(key.into()).or_default() += n;
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Conflict {
    /// Keep the destination's row (collections, ids).
    Ignore,
    /// The source's row wins (single-row preference tables).
    Replace,
}

fn open_ro(path: &Path) -> Result<Connection, String> {
    Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| format!("{}: {e}", path.display()))
}

fn open_rw(path: &Path) -> Result<Connection, String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    let conn = Connection::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000;")
        .map_err(|e| e.to_string())?;
    Ok(conn)
}

fn columns(conn: &Connection, table: &str) -> Result<Vec<String>, String> {
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info(\"{table}\")"))
        .map_err(|e| e.to_string())?;
    let cols = stmt
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .collect();
    Ok(cols)
}

fn user_tables(conn: &Connection) -> Result<Vec<String>, String> {
    let mut stmt = conn
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'")
        .map_err(|e| e.to_string())?;
    let names = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .collect();
    Ok(names)
}

/// Count rows that contain actual connection state rather than the empty
/// placeholder inserted when a settings store is first opened. The candidate
/// columns are intersected with the live schema so this also works with older
/// QBZ profile databases.
fn meaningful_connection_rows(
    src_dir: &Path,
    file: &str,
    table: &str,
    candidate_columns: &[&str],
) -> Result<usize, String> {
    let path = src_dir.join(file);
    if !path.is_file() {
        return Ok(0);
    }
    let conn = open_ro(&path)?;
    let present = columns(&conn, table)?;
    let present: HashSet<&str> = present.iter().map(String::as_str).collect();
    let selected: Vec<&str> = candidate_columns
        .iter()
        .copied()
        .filter(|column| present.contains(column))
        .collect();
    if selected.is_empty() {
        return Ok(0);
    }
    let projection = selected
        .iter()
        .map(|column| format!("\"{column}\""))
        .collect::<Vec<_>>()
        .join(", ");
    let mut statement = conn
        .prepare(&format!("SELECT {projection} FROM \"{table}\""))
        .map_err(|e| e.to_string())?;
    let mut rows = statement.query([]).map_err(|e| e.to_string())?;
    let mut count = 0;
    while let Some(row) = rows.next().map_err(|e| e.to_string())? {
        let mut meaningful = false;
        for index in 0..selected.len() {
            meaningful |= match row.get::<_, Value>(index).map_err(|e| e.to_string())? {
                Value::Null => false,
                Value::Integer(value) => value != 0,
                Value::Real(value) => value != 0.0,
                Value::Text(value) => {
                    let value = value.trim();
                    !value.is_empty() && value != "[]"
                }
                Value::Blob(value) => !value.is_empty(),
            };
        }
        count += usize::from(meaningful);
    }
    Ok(count)
}

/// Copy one table by column intersection. `remap` names a column holding
/// a source playlist id and the map to translate it; rows whose id is not
/// in the map are skipped and counted in `unmapped`.
fn copy_table(
    src: &Connection,
    dst: &Connection,
    table: &str,
    conflict: Conflict,
    remap: Option<(&str, &BTreeMap<u64, u64>)>,
    unmapped: &mut usize,
) -> Result<usize, String> {
    let src_cols = columns(src, table)?;
    let dst_cols = columns(dst, table)?;
    if src_cols.is_empty() || dst_cols.is_empty() {
        return Ok(0);
    }
    let dst_set: HashSet<&str> = dst_cols.iter().map(String::as_str).collect();
    let cols: Vec<&str> = src_cols
        .iter()
        .map(String::as_str)
        .filter(|c| dst_set.contains(c))
        .collect();
    if cols.is_empty() {
        return Ok(0);
    }
    let remap_index = remap.and_then(|(col, _)| cols.iter().position(|c| *c == col));
    if remap.is_some() && remap_index.is_none() {
        return Ok(0);
    }
    let quoted: Vec<String> = cols.iter().map(|c| format!("\"{c}\"")).collect();
    let select = format!("SELECT {} FROM \"{table}\"", quoted.join(", "));
    let verb = match conflict {
        Conflict::Ignore => "INSERT OR IGNORE",
        Conflict::Replace => "INSERT OR REPLACE",
    };
    let placeholders: Vec<&str> = cols.iter().map(|_| "?").collect();
    let insert = format!(
        "{verb} INTO \"{table}\" ({}) VALUES ({})",
        quoted.join(", "),
        placeholders.join(", ")
    );

    let mut read = src.prepare(&select).map_err(|e| e.to_string())?;
    let mut write = dst.prepare(&insert).map_err(|e| e.to_string())?;
    let mut rows = read.query([]).map_err(|e| e.to_string())?;
    let mut inserted = 0usize;
    while let Some(row) = rows.next().map_err(|e| e.to_string())? {
        let mut values: Vec<Value> = Vec::with_capacity(cols.len());
        for i in 0..cols.len() {
            values.push(row.get::<_, Value>(i).map_err(|e| e.to_string())?);
        }
        if let (Some(idx), Some((_, map))) = (remap_index, remap) {
            let old = match &values[idx] {
                Value::Integer(n) => Some(*n as u64),
                Value::Text(s) => s.parse::<u64>().ok(),
                _ => None,
            };
            match old.and_then(|o| map.get(&o)) {
                Some(new) => {
                    values[idx] = match &values[idx] {
                        Value::Text(_) => Value::Text(new.to_string()),
                        _ => Value::Integer(*new as i64),
                    }
                }
                None => {
                    *unmapped += 1;
                    continue;
                }
            }
        }
        inserted += write
            .execute(rusqlite::params_from_iter(values.iter()))
            .map_err(|e| format!("{table}: {e}"))?;
    }
    Ok(inserted)
}

/// Copy every user table of a store, one conflict policy for all.
fn copy_store(
    src_dir: &Path,
    dst_dir: &Path,
    file: &str,
    conflict: Conflict,
    skip_tables: &[&str],
    report: &mut LocalReport,
) -> Result<(), String> {
    let src_path = src_dir.join(file);
    if !src_path.is_file() {
        report
            .notes
            .push(format!("{file}: not in the source profile"));
        return Ok(());
    }
    let src = open_ro(&src_path)?;
    let dst = open_rw(&dst_dir.join(file))?;
    // Make sure the destination has the same tables; a store the target
    // app has not opened yet is an empty file, so create from the source
    // DDL (tables only — indexes come back when the app opens it).
    for table in user_tables(&src)? {
        if skip_tables.contains(&table.as_str()) {
            continue;
        }
        if columns(&dst, &table)?.is_empty() {
            let ddl: String = src
                .query_row(
                    "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = ?1",
                    [&table],
                    |row| row.get(0),
                )
                .map_err(|e| e.to_string())?;
            dst.execute_batch(&ddl)
                .map_err(|e| format!("{file}/{table}: {e}"))?;
        }
        let mut unmapped = 0;
        let n = copy_table(&src, &dst, &table, conflict, None, &mut unmapped)?;
        report.add(format!("{file}/{table}"), n);
    }
    Ok(())
}

/// Copy a file only when the destination does not have it.
fn copy_file_if_absent(src_dir: &Path, dst_dir: &Path, rel: &str, report: &mut LocalReport) {
    let src = src_dir.join(rel);
    let dst = dst_dir.join(rel);
    if !src.is_file() {
        return;
    }
    if dst.exists() {
        report.notes.push(format!("{rel}: kept the existing one"));
        return;
    }
    if let Some(parent) = dst.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match std::fs::copy(&src, &dst) {
        Ok(_) => report.add(rel, 1),
        Err(e) => report.notes.push(format!("{rel}: copy failed: {e}")),
    }
}

/// The LOCAL half of `library.db` (folders, scanned tracks, mixtapes...).
const LIBRARY_LOCAL_TABLES: &[&str] = &[
    "library_folders",
    "local_tracks",
    "local_scan_cue_refs",
    "local_sacd_images",
    "local_sacd_tracks",
    "album_settings",
    "custom_album_covers",
    "artist_images",
    "library_kv",
    "local_playlists",
    "local_playlist_tracks",
    "mixtape_collections",
    "mixtape_collection_items",
];

/// The ACCOUNT half of `library.db`: tables keyed by `qobuz_playlist_id`.
/// `playlist_local_tracks` rides on `local_tracks` ids, so only on the fast
/// path.
const LIBRARY_PLAYLIST_TABLES: &[&str] = &[
    "playlist_settings",
    "playlist_track_custom_order",
    "playlist_plex_tracks",
    "playlist_remote_tracks",
    "playlist_stats",
    "copied_playlists",
];

/// Preference stores copied whole, the source's rows winning on conflict
/// (single-row tables).
const PREF_STORES: &[&str] = &[
    "discover_prefs.db",
    "favorites_preferences.db",
    "tray_settings.db",
    "remote_control_settings.db",
    "playback_preferences.db",
];

/// Collection stores copied additively (ids are catalog-global).
const COLLECTION_STORES: &[(&str, &[&str])] = &[
    ("artist_blacklist.db", &["blacklist_settings"]),
    ("local_favorites.db", &[]),
];

/// JSON sidecars copied only when the destination lacks them.
const JSON_IF_ABSENT: &[&str] = &[
    "lyrics_prefs.json",
    "reco_dismiss.json",
    "collection_view_prefs.json",
    "myqbz_branding.json",
    "collection_open_rows.json",
];

fn library_folder_count(conn: &Connection) -> usize {
    conn.query_row("SELECT COUNT(*) FROM library_folders", [], |r| {
        r.get::<_, i64>(0)
    })
    .unwrap_or(0) as usize
}

fn copy_library(
    src_dir: &Path,
    dst_dir: &Path,
    map: &BTreeMap<u64, u64>,
    report: &mut LocalReport,
) -> Result<(), String> {
    let src_path = src_dir.join("library.db");
    if !src_path.is_file() {
        report
            .notes
            .push("library.db: not in the source profile".into());
        return Ok(());
    }
    let src = open_ro(&src_path)?;
    let dst = open_rw(&dst_dir.join("library.db"))?;
    let src_tables: HashSet<String> = user_tables(&src)?.into_iter().collect();
    let dst_tables: HashSet<String> = user_tables(&dst)?.into_iter().collect();
    // Tables the destination app has not created yet come from the source
    // DDL, so a profile that never opened Local Library still receives it.
    for table in LIBRARY_LOCAL_TABLES
        .iter()
        .chain(LIBRARY_PLAYLIST_TABLES.iter())
        .chain(["playlist_folders", "playlist_local_tracks"].iter())
    {
        if src_tables.contains(*table) && !dst_tables.contains(*table) {
            let ddl: String = src
                .query_row(
                    "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = ?1",
                    [table],
                    |row| row.get(0),
                )
                .map_err(|e| e.to_string())?;
            dst.execute_batch(&ddl)
                .map_err(|e| format!("library.db/{table}: {e}"))?;
        }
    }

    let fast_path = library_folder_count(&dst) == 0;
    let mut unmapped = 0usize;
    if fast_path {
        for table in LIBRARY_LOCAL_TABLES {
            if !src_tables.contains(*table) {
                continue;
            }
            let n = copy_table(&src, &dst, table, Conflict::Ignore, None, &mut unmapped)?;
            report.add(format!("library.db/{table}"), n);
        }
    } else {
        report.needs_rescan = true;
        report.notes.push(
            "library.db: the destination already has library folders; only missing folders were added, run a rescan"
                .into(),
        );
        if src_tables.contains("library_folders") {
            let n = copy_table(
                &src,
                &dst,
                "library_folders",
                Conflict::Ignore,
                None,
                &mut unmapped,
            )?;
            report.add("library.db/library_folders", n);
        }
        for table in [
            "album_settings",
            "custom_album_covers",
            "artist_images",
            "mixtape_collections",
            "mixtape_collection_items",
        ] {
            if src_tables.contains(table) {
                let n = copy_table(&src, &dst, table, Conflict::Ignore, None, &mut unmapped)?;
                report.add(format!("library.db/{table}"), n);
            }
        }
    }

    // Playlist folders keep their uuids; the playlist rows they hold are
    // remapped below.
    if src_tables.contains("playlist_folders") {
        let n = copy_table(
            &src,
            &dst,
            "playlist_folders",
            Conflict::Ignore,
            None,
            &mut unmapped,
        )?;
        report.add("library.db/playlist_folders", n);
    }
    for table in LIBRARY_PLAYLIST_TABLES {
        if !src_tables.contains(*table) {
            continue;
        }
        let n = copy_table(
            &src,
            &dst,
            table,
            Conflict::Ignore,
            Some(("qobuz_playlist_id", map)),
            &mut unmapped,
        )?;
        report.add(format!("library.db/{table}"), n);
    }
    if fast_path && src_tables.contains("playlist_local_tracks") {
        let n = copy_table(
            &src,
            &dst,
            "playlist_local_tracks",
            Conflict::Ignore,
            Some(("qobuz_playlist_id", map)),
            &mut unmapped,
        )?;
        report.add("library.db/playlist_local_tracks", n);
    }
    report.unmapped_playlist_rows += unmapped;
    Ok(())
}

fn copy_pinned(
    src_dir: &Path,
    dst_dir: &Path,
    map: &BTreeMap<u64, u64>,
    report: &mut LocalReport,
) -> Result<(), String> {
    let src_path = src_dir.join("pinned_items.db");
    if !src_path.is_file() {
        return Ok(());
    }
    let src = open_ro(&src_path)?;
    let dst = open_rw(&dst_dir.join("pinned_items.db"))?;
    if columns(&dst, "pinned_items")?.is_empty() {
        let ddl: String = src
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'pinned_items'",
                [],
                |row| row.get(0),
            )
            .map_err(|e| e.to_string())?;
        dst.execute_batch(&ddl).map_err(|e| e.to_string())?;
    }
    let mut read = src
        .prepare("SELECT kind, id, title, subtitle, artwork_url, pinned_at FROM pinned_items")
        .map_err(|e| e.to_string())?;
    let mut write = dst
        .prepare(
            "INSERT OR IGNORE INTO pinned_items (kind, id, title, subtitle, artwork_url, pinned_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )
        .map_err(|e| e.to_string())?;
    let mut rows = read.query([]).map_err(|e| e.to_string())?;
    let mut n = 0usize;
    while let Some(row) = rows.next().map_err(|e| e.to_string())? {
        let kind: String = row.get(0).map_err(|e| e.to_string())?;
        let mut id: String = row.get(1).map_err(|e| e.to_string())?;
        if kind == "playlist" {
            match id.parse::<u64>().ok().and_then(|old| map.get(&old)) {
                Some(new) => id = new.to_string(),
                None => {
                    report.unmapped_playlist_rows += 1;
                    continue;
                }
            }
        }
        let title: String = row.get(2).map_err(|e| e.to_string())?;
        let subtitle: Option<String> = row.get(3).map_err(|e| e.to_string())?;
        let artwork: Option<String> = row.get(4).map_err(|e| e.to_string())?;
        let pinned_at: i64 = row.get(5).map_err(|e| e.to_string())?;
        n += write
            .execute(rusqlite::params![
                kind, id, title, subtitle, artwork, pinned_at
            ])
            .map_err(|e| e.to_string())?;
    }
    report.add("pinned_items.db/pinned_items", n);
    Ok(())
}

/// `playlist_orders.json`: `{ "<playlist id>": [track ids] }` — remap the
/// keys, keep the destination's entry when it already has one.
fn copy_playlist_orders(
    src_dir: &Path,
    dst_dir: &Path,
    map: &BTreeMap<u64, u64>,
    report: &mut LocalReport,
) -> Result<(), String> {
    let src_path = src_dir.join("playlist_orders.json");
    if !src_path.is_file() {
        return Ok(());
    }
    let src: serde_json::Map<String, serde_json::Value> =
        serde_json::from_str(&std::fs::read_to_string(&src_path).map_err(|e| e.to_string())?)
            .map_err(|e| format!("playlist_orders.json: {e}"))?;
    let dst_path = dst_dir.join("playlist_orders.json");
    let mut dst: serde_json::Map<String, serde_json::Value> = if dst_path.is_file() {
        serde_json::from_str(&std::fs::read_to_string(&dst_path).map_err(|e| e.to_string())?)
            .unwrap_or_default()
    } else {
        serde_json::Map::new()
    };
    let mut n = 0usize;
    for (old, order) in src {
        let Some(new) = old.parse::<u64>().ok().and_then(|o| map.get(&o)) else {
            report.unmapped_playlist_rows += 1;
            continue;
        };
        let key = new.to_string();
        if !dst.contains_key(&key) {
            dst.insert(key, order);
            n += 1;
        }
    }
    if n > 0 {
        std::fs::write(
            &dst_path,
            serde_json::to_string_pretty(&serde_json::Value::Object(dst))
                .map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
    }
    report.add("playlist_orders.json", n);
    Ok(())
}

/// Run the local copy. `map` is the ledger's `playlist_map` (source id →
/// target id; subscriptions map onto themselves). Each store is
/// best-effort: a failure is a note, the rest proceeds.
pub fn copy_profile(
    src_dir: &Path,
    dst_dir: &Path,
    ledger: &Ledger,
    options: LocalOptions,
) -> Result<LocalReport, String> {
    if !src_dir.is_dir() {
        return Err(format!("{}: not a profile directory", src_dir.display()));
    }
    if src_dir == dst_dir {
        return Err("source and destination are the same profile".into());
    }
    std::fs::create_dir_all(dst_dir).map_err(|e| format!("{}: {e}", dst_dir.display()))?;
    let map = &ledger.playlist_map;
    let mut report = LocalReport::default();

    if let Err(e) = copy_library(src_dir, dst_dir, map, &mut report) {
        report.notes.push(format!("library.db: {e}"));
    }
    if let Err(e) = copy_pinned(src_dir, dst_dir, map, &mut report) {
        report.notes.push(format!("pinned_items.db: {e}"));
    }
    if let Err(e) = copy_playlist_orders(src_dir, dst_dir, map, &mut report) {
        report.notes.push(format!("playlist_orders.json: {e}"));
    }
    for file in PREF_STORES {
        if let Err(e) = copy_store(src_dir, dst_dir, file, Conflict::Replace, &[], &mut report) {
            report.notes.push(format!("{file}: {e}"));
        }
    }
    // offline_settings.db: the single preference row only, never the
    // scrobble queue or a pending playlist sync of another account.
    if let Err(e) = copy_store(
        src_dir,
        dst_dir,
        "offline_settings.db",
        Conflict::Replace,
        &["scrobble_queue", "pending_playlist_sync", "sqlite_sequence"],
        &mut report,
    ) {
        report.notes.push(format!("offline_settings.db: {e}"));
    }
    for (file, skip) in COLLECTION_STORES {
        if let Err(e) = copy_store(src_dir, dst_dir, file, Conflict::Ignore, skip, &mut report) {
            report.notes.push(format!("{file}: {e}"));
        }
    }
    if options.media_servers {
        const CONNECTION_STORES: [(&str, &str, &[&str]); 2] = [
            (
                "media_servers.db",
                "media_server_settings",
                &[
                    "enabled",
                    "base_url",
                    "server_name",
                    "server_id",
                    "username",
                    "token",
                    "password",
                    "selected_libraries",
                ],
            ),
            (
                "plex_settings.db",
                "plex_settings",
                &[
                    "enabled",
                    "base_url",
                    // Pre-profile schema used this name.
                    "server_url",
                    "token",
                    "selected_section_key",
                    "selected_section_keys",
                    "machine_id",
                ],
            ),
        ];
        for (file, table, candidate_columns) in CONNECTION_STORES {
            let found = match meaningful_connection_rows(src_dir, file, table, candidate_columns) {
                Ok(found) => found,
                Err(error) => {
                    report.notes.push(format!(
                        "{file}: could not inspect connection records: {error}"
                    ));
                    0
                }
            };
            report.media_connections_found += found;
            match copy_store(src_dir, dst_dir, file, Conflict::Replace, &[], &mut report) {
                Ok(()) => report.media_connections_copied += found,
                Err(e) => report.notes.push(format!("{file}: {e}")),
            }
        }
    }
    if options.scrobblers {
        if let Err(e) = copy_store(
            src_dir,
            dst_dir,
            "scrobbler_settings.db",
            Conflict::Replace,
            &[],
            &mut report,
        ) {
            report.notes.push(format!("scrobbler_settings.db: {e}"));
        }
    }
    if options.listening_history {
        for file in [
            "listen/listen_log.db",
            "reco/events.db",
            "external_reco_cache.db",
        ] {
            if let Err(e) = copy_store(
                src_dir,
                dst_dir,
                file,
                Conflict::Ignore,
                &["sqlite_sequence"],
                &mut report,
            ) {
                report.notes.push(format!("{file}: {e}"));
            }
        }
    }
    for rel in JSON_IF_ABSENT {
        copy_file_if_absent(src_dir, dst_dir, rel, &mut report);
    }
    Ok(report)
}

/// Profile directories under `<data_root>/users/` other than `current`,
/// with their last-modified time, newest first.
pub fn other_profiles(data_root: &Path, current: u64) -> Vec<(u64, PathBuf)> {
    let Ok(users) = std::fs::read_dir(data_root.join("users")) else {
        return Vec::new();
    };
    let mut found: Vec<(u64, PathBuf, std::time::SystemTime)> = users
        .flatten()
        .filter_map(|entry| {
            let uid = entry.file_name().to_str()?.parse::<u64>().ok()?;
            if uid == current || uid == 0 || !entry.path().is_dir() {
                return None;
            }
            let modified = entry
                .metadata()
                .and_then(|m| m.modified())
                .unwrap_or(std::time::UNIX_EPOCH);
            Some((uid, entry.path(), modified))
        })
        .collect();
    found.sort_by(|a, b| b.2.cmp(&a.2));
    found.into_iter().map(|(uid, p, _)| (uid, p)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db(path: &Path, ddl: &str) -> Connection {
        let c = Connection::open(path).unwrap();
        c.execute_batch(ddl).unwrap();
        c
    }

    const LIBRARY_DDL: &str = "
        CREATE TABLE library_folders (id INTEGER PRIMARY KEY, path TEXT UNIQUE NOT NULL, enabled INTEGER DEFAULT 1, last_scan INTEGER);
        CREATE TABLE local_tracks (id INTEGER PRIMARY KEY, file_path TEXT NOT NULL, title TEXT NOT NULL, UNIQUE(file_path));
        CREATE TABLE playlist_folders (id TEXT PRIMARY KEY, name TEXT NOT NULL, position INTEGER DEFAULT 0);
        CREATE TABLE playlist_settings (qobuz_playlist_id INTEGER PRIMARY KEY, folder_id TEXT, position INTEGER DEFAULT 0, is_favorite INTEGER DEFAULT 0);
        CREATE TABLE playlist_local_tracks (id INTEGER PRIMARY KEY, qobuz_playlist_id INTEGER NOT NULL, local_track_id INTEGER NOT NULL, position INTEGER NOT NULL, UNIQUE(qobuz_playlist_id, local_track_id));
        CREATE TABLE downloaded_purchases (track_id INTEGER PRIMARY KEY, file_path TEXT NOT NULL);
    ";

    fn ledger(pairs: &[(u64, u64)]) -> Ledger {
        let mut l = Ledger::default();
        for (a, b) in pairs {
            l.playlist_map.insert(*a, *b);
        }
        l
    }

    #[test]
    fn fast_path_copies_the_library_and_remaps_playlist_rows() {
        let tmp = tempfile::tempdir().unwrap();
        let src_dir = tmp.path().join("10");
        let dst_dir = tmp.path().join("20");
        std::fs::create_dir_all(&src_dir).unwrap();
        std::fs::create_dir_all(&dst_dir).unwrap();
        let src = db(&src_dir.join("library.db"), LIBRARY_DDL);
        src.execute_batch(
            "INSERT INTO library_folders (id, path) VALUES (2, '/mnt/nas/music'), (4, '/home/u/Music');
             INSERT INTO local_tracks (id, file_path, title) VALUES (7, '/mnt/nas/music/a.flac', 'A');
             INSERT INTO playlist_folders (id, name, position) VALUES ('f1', 'Rock', 3);
             INSERT INTO playlist_settings (qobuz_playlist_id, folder_id, position, is_favorite) VALUES (100, 'f1', 1, 1), (200, NULL, 2, 0);
             INSERT INTO playlist_local_tracks (id, qobuz_playlist_id, local_track_id, position) VALUES (1, 100, 7, 0);
             INSERT INTO downloaded_purchases (track_id, file_path) VALUES (9, '/x');",
        )
        .unwrap();
        drop(src);
        // The destination app created the schema but holds nothing yet.
        db(&dst_dir.join("library.db"), LIBRARY_DDL);

        let report = copy_profile(
            &src_dir,
            &dst_dir,
            &ledger(&[(100, 555)]),
            LocalOptions::default(),
        )
        .unwrap();
        assert!(!report.needs_rescan);
        assert_eq!(report.copied["library.db/library_folders"], 2);
        assert_eq!(report.copied["library.db/local_tracks"], 1);
        assert_eq!(report.copied["library.db/playlist_folders"], 1);
        assert_eq!(report.copied["library.db/playlist_settings"], 1);
        assert_eq!(report.copied["library.db/playlist_local_tracks"], 1);
        // 200 had no mapping: skipped and counted.
        assert_eq!(report.unmapped_playlist_rows, 1);

        let dst = Connection::open(dst_dir.join("library.db")).unwrap();
        let mapped: (i64, String, i64) = dst
            .query_row(
                "SELECT qobuz_playlist_id, folder_id, is_favorite FROM playlist_settings",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(mapped, (555, "f1".into(), 1));
        let link: i64 = dst
            .query_row(
                "SELECT qobuz_playlist_id FROM playlist_local_tracks",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(link, 555);
        // Purchases belong to the account: never copied.
        let purchases: i64 = dst
            .query_row("SELECT COUNT(*) FROM downloaded_purchases", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(purchases, 0);
        // Source untouched (still 2 folders, opened read-only).
        let src = Connection::open(src_dir.join("library.db")).unwrap();
        assert_eq!(library_folder_count(&src), 2);
    }

    #[test]
    fn slow_path_adds_missing_folders_only_and_flags_a_rescan() {
        let tmp = tempfile::tempdir().unwrap();
        let src_dir = tmp.path().join("10");
        let dst_dir = tmp.path().join("20");
        std::fs::create_dir_all(&src_dir).unwrap();
        std::fs::create_dir_all(&dst_dir).unwrap();
        let src = db(&src_dir.join("library.db"), LIBRARY_DDL);
        src.execute_batch(
            "INSERT INTO library_folders (id, path) VALUES (1, '/a'), (2, '/b');
             INSERT INTO local_tracks (id, file_path, title) VALUES (7, '/a/x.flac', 'X');",
        )
        .unwrap();
        drop(src);
        let dst = db(&dst_dir.join("library.db"), LIBRARY_DDL);
        dst.execute_batch("INSERT INTO library_folders (id, path) VALUES (9, '/b');")
            .unwrap();
        drop(dst);

        let report = copy_profile(
            &src_dir,
            &dst_dir,
            &Ledger::default(),
            LocalOptions::default(),
        )
        .unwrap();
        assert!(report.needs_rescan);
        // '/b' existed (UNIQUE path): only '/a' was added; tracks skipped.
        assert_eq!(report.copied["library.db/library_folders"], 1);
        assert!(!report.copied.contains_key("library.db/local_tracks"));
        let dst = Connection::open(dst_dir.join("library.db")).unwrap();
        assert_eq!(library_folder_count(&dst), 2);
    }

    #[test]
    fn preferences_replace_collections_merge_and_options_gate_credentials() {
        let tmp = tempfile::tempdir().unwrap();
        let src_dir = tmp.path().join("10");
        let dst_dir = tmp.path().join("20");
        std::fs::create_dir_all(&src_dir).unwrap();
        std::fs::create_dir_all(&dst_dir).unwrap();
        let prefs_ddl = "CREATE TABLE tray_settings (id INTEGER PRIMARY KEY CHECK (id = 1), enabled INTEGER NOT NULL DEFAULT 0);";
        db(&src_dir.join("tray_settings.db"), prefs_ddl)
            .execute_batch("INSERT INTO tray_settings (id, enabled) VALUES (1, 1);")
            .unwrap();
        db(&dst_dir.join("tray_settings.db"), prefs_ddl)
            .execute_batch("INSERT INTO tray_settings (id, enabled) VALUES (1, 0);")
            .unwrap();
        let bl_ddl = "CREATE TABLE artist_blacklist (artist_id INTEGER PRIMARY KEY, artist_name TEXT NOT NULL);
                      CREATE TABLE blacklist_settings (id INTEGER PRIMARY KEY CHECK (id = 1), enabled INTEGER NOT NULL DEFAULT 1);";
        db(&src_dir.join("artist_blacklist.db"), bl_ddl)
            .execute_batch("INSERT INTO artist_blacklist VALUES (1, 'A'), (2, 'B'); INSERT INTO blacklist_settings VALUES (1, 0);")
            .unwrap();
        db(&dst_dir.join("artist_blacklist.db"), bl_ddl)
            .execute_batch("INSERT INTO artist_blacklist VALUES (2, 'B mine'); INSERT INTO blacklist_settings VALUES (1, 1);")
            .unwrap();
        let ms_ddl = "CREATE TABLE media_server_settings (server TEXT PRIMARY KEY, token TEXT NOT NULL DEFAULT '');";
        db(&src_dir.join("media_servers.db"), ms_ddl)
            .execute_batch(
                "INSERT INTO media_server_settings VALUES ('jellyfin', 'fixture-token');",
            )
            .unwrap();
        let plex_ddl = "CREATE TABLE plex_settings (id INTEGER PRIMARY KEY CHECK (id = 1), server_url TEXT NOT NULL DEFAULT '', token TEXT NOT NULL DEFAULT '');";
        db(&src_dir.join("plex_settings.db"), plex_ddl)
            .execute_batch(
                "INSERT INTO plex_settings VALUES (1, 'http://plex.test', 'fixture-plex-token');",
            )
            .unwrap();
        std::fs::write(src_dir.join("lyrics_prefs.json"), "{\"font\":\"x\"}").unwrap();

        let report = copy_profile(
            &src_dir,
            &dst_dir,
            &Ledger::default(),
            LocalOptions {
                media_servers: false,
                ..LocalOptions::default()
            },
        )
        .unwrap();
        // Preference row: the source wins.
        let tray: i64 = Connection::open(dst_dir.join("tray_settings.db"))
            .unwrap()
            .query_row("SELECT enabled FROM tray_settings", [], |r| r.get(0))
            .unwrap();
        assert_eq!(tray, 1);
        // Blacklist: additive, the destination's row and its flag kept.
        let bl = Connection::open(dst_dir.join("artist_blacklist.db")).unwrap();
        let name: String = bl
            .query_row(
                "SELECT artist_name FROM artist_blacklist WHERE artist_id = 2",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(name, "B mine");
        let count: i64 = bl
            .query_row("SELECT COUNT(*) FROM artist_blacklist", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 2);
        let flag: i64 = bl
            .query_row("SELECT enabled FROM blacklist_settings", [], |r| r.get(0))
            .unwrap();
        assert_eq!(flag, 1);
        // Media servers were opted out: nothing written.
        assert!(!dst_dir.join("media_servers.db").exists());
        assert!(!dst_dir.join("plex_settings.db").exists());
        // JSON sidecar copied because absent.
        assert_eq!(report.copied["lyrics_prefs.json"], 1);
        assert!(dst_dir.join("lyrics_prefs.json").is_file());
        // Never-copied stores are not even created.
        assert!(!dst_dir.join("favorites_cache.db").exists());
        assert!(!dst_dir.join("subscription_state.db").exists());

        // A later repair run with the option enabled must still copy both
        // server stores, including credentials. This is the local-only rerun
        // the UI now allows after the cloud plan reaches zero changes.
        let repair = copy_profile(
            &src_dir,
            &dst_dir,
            &Ledger::default(),
            LocalOptions::default(),
        )
        .unwrap();
        assert_eq!(repair.media_connections_found, 2);
        assert_eq!(repair.media_connections_copied, 2);
        let media_token: String = Connection::open(dst_dir.join("media_servers.db"))
            .unwrap()
            .query_row(
                "SELECT token FROM media_server_settings WHERE server = 'jellyfin'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(media_token, "fixture-token");
        let plex: (String, String) = Connection::open(dst_dir.join("plex_settings.db"))
            .unwrap()
            .query_row("SELECT server_url, token FROM plex_settings", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!(
            plex,
            ("http://plex.test".into(), "fixture-plex-token".into())
        );
    }

    #[test]
    fn empty_media_placeholders_are_not_reported_as_copied_connections() {
        let tmp = tempfile::tempdir().unwrap();
        let src_dir = tmp.path().join("10");
        let dst_dir = tmp.path().join("20");
        std::fs::create_dir_all(&src_dir).unwrap();
        std::fs::create_dir_all(&dst_dir).unwrap();
        let media = "CREATE TABLE media_server_settings (
            server TEXT PRIMARY KEY, enabled INTEGER NOT NULL DEFAULT 0,
            base_url TEXT NOT NULL DEFAULT '', token TEXT NOT NULL DEFAULT '',
            username TEXT NOT NULL DEFAULT '', password TEXT NOT NULL DEFAULT '',
            selected_libraries TEXT NOT NULL DEFAULT '');";
        db(&src_dir.join("media_servers.db"), media)
            .execute_batch(
                "INSERT INTO media_server_settings (server) VALUES ('jellyfin'), ('subsonic');",
            )
            .unwrap();
        let plex = "CREATE TABLE plex_settings (
            id INTEGER PRIMARY KEY CHECK (id = 1), enabled INTEGER NOT NULL DEFAULT 0,
            base_url TEXT NOT NULL DEFAULT '', token TEXT NOT NULL DEFAULT '',
            selected_section_keys TEXT NOT NULL DEFAULT '[]');";
        db(&src_dir.join("plex_settings.db"), plex)
            .execute_batch("INSERT INTO plex_settings (id) VALUES (1);")
            .unwrap();

        let report = copy_profile(
            &src_dir,
            &dst_dir,
            &Ledger::default(),
            LocalOptions::default(),
        )
        .unwrap();

        assert_eq!(report.media_connections_found, 0);
        assert_eq!(report.media_connections_copied, 0);
    }

    #[test]
    fn pinned_playlists_and_orders_are_remapped() {
        let tmp = tempfile::tempdir().unwrap();
        let src_dir = tmp.path().join("10");
        let dst_dir = tmp.path().join("20");
        std::fs::create_dir_all(&src_dir).unwrap();
        std::fs::create_dir_all(&dst_dir).unwrap();
        let ddl = "CREATE TABLE pinned_items (kind TEXT NOT NULL, id TEXT NOT NULL, title TEXT NOT NULL, subtitle TEXT, artwork_url TEXT, pinned_at INTEGER NOT NULL, PRIMARY KEY (kind, id));";
        db(&src_dir.join("pinned_items.db"), ddl)
            .execute_batch(
                "INSERT INTO pinned_items VALUES ('album', 'abc', 'Album', NULL, NULL, 1), ('playlist', '100', 'Mix', NULL, NULL, 2), ('playlist', '300', 'Lost', NULL, NULL, 3);",
            )
            .unwrap();
        std::fs::write(
            src_dir.join("playlist_orders.json"),
            "{\"100\": [1, 2], \"300\": [3]}",
        )
        .unwrap();
        let report = copy_profile(
            &src_dir,
            &dst_dir,
            &ledger(&[(100, 555)]),
            LocalOptions::default(),
        )
        .unwrap();
        assert_eq!(report.copied["pinned_items.db/pinned_items"], 2);
        assert_eq!(report.copied["playlist_orders.json"], 1);
        assert_eq!(report.unmapped_playlist_rows, 2);
        let ids: Vec<String> = Connection::open(dst_dir.join("pinned_items.db"))
            .unwrap()
            .prepare("SELECT id FROM pinned_items ORDER BY pinned_at")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(ids, vec!["abc", "555"]);
        let orders: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(dst_dir.join("playlist_orders.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(orders["555"], serde_json::json!([1, 2]));
        assert!(orders.get("100").is_none());
    }
}
