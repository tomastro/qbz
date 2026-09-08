//! First-class LOCAL playlists (offline-mode port, decisions D7/D8).
//!
//! Unlike the `playlist_*` sidecar tables (which enhance a *Qobuz* playlist
//! keyed by its server id), these are standalone entities living entirely in
//! the per-user `library.db`. Ids are TEXT `local:<uuid>` (Mixtape precedent)
//! so they are unrepresentable in any Qobuz-bound call that takes a `u64`
//! playlist id — the type guard demanded by D7.
//!
//! All functions take `&Connection` (the qbz-mixtape repo idiom): no Tauri
//! state, no async runtime — testable with in-memory SQLite. The Slint
//! command layer reaches them through `LibraryDatabase::with_connection`.

use rusqlite::{params, Connection, OptionalExtension, Result};
use uuid::Uuid;

/// Id prefix that marks a local playlist. A `local:<uuid>` id can never
/// parse as the `u64` every Qobuz playlist endpoint takes.
pub const LOCAL_PLAYLIST_PREFIX: &str = "local:";

/// True when `id` names a local playlist (`local:<uuid>` namespace).
pub fn is_local_playlist_id(id: &str) -> bool {
    id.starts_with(LOCAL_PLAYLIST_PREFIX)
}

/// Track source inside a local playlist.
///
/// Jellyfin/Subsonic rows (2026-08-30) store the SERVER ITEM ID in the
/// `local_path` column — the same convention their library projection uses
/// for `file_path`: that column is "the source-native locator", not always a
/// filesystem path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalPlaylistTrackSource {
    Qobuz,
    Local,
    Plex,
    Jellyfin,
    Subsonic,
}

impl LocalPlaylistTrackSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Qobuz => "qobuz",
            Self::Local => "local",
            Self::Plex => "plex",
            Self::Jellyfin => "jellyfin",
            Self::Subsonic => "subsonic",
        }
    }
    fn parse(s: &str) -> Self {
        match s {
            "local" => Self::Local,
            "plex" => Self::Plex,
            "jellyfin" => Self::Jellyfin,
            "subsonic" => Self::Subsonic,
            _ => Self::Qobuz,
        }
    }
}

/// One playlist row (header metadata + per-source counts).
#[derive(Debug, Clone)]
pub struct LocalPlaylist {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    /// D8: never offered for upload, never reaches any Qobuz call or
    /// QConnect queue push.
    pub offline_only: bool,
    /// B3: manager organization flags — the local twin of
    /// `playlist_settings.is_favorite` / `.hidden` (those tables are
    /// keyed by the Qobuz u64 id, unrepresentable for `local:` ids).
    pub favorite: bool,
    /// B3: hidden playlists drop from the sidebar and group under the
    /// manager's "hidden" filter.
    pub hidden: bool,
    pub custom_artwork_path: Option<String>,
    /// Sidebar folder membership (shared `playlist_folders.id`); None = root.
    pub folder_id: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub track_count: u32,
    pub qobuz_count: u32,
    pub local_count: u32,
    pub plex_count: u32,
}

/// One membership row, ordered by `position`.
#[derive(Debug, Clone)]
pub struct LocalPlaylistTrack {
    pub playlist_id: String,
    pub position: i32,
    pub source: LocalPlaylistTrackSource,
    pub qobuz_track_id: Option<u64>,
    pub local_path: Option<String>,
    pub plex_key: Option<String>,
    pub added_at: i64,
}

/// Input for `add_tracks` — exactly one source-native ref per variant.
/// Jellyfin/Subsonic carry the server item id (stored in `local_path`).
#[derive(Debug, Clone)]
pub enum LocalPlaylistTrackInput {
    Qobuz(u64),
    Local(String),
    Plex(String),
    Jellyfin(String),
    Subsonic(String),
}

fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

/// Create the local-playlist tables. Idempotent (`IF NOT EXISTS`), run by
/// `LibraryDatabase::open` next to the rest of the schema.
pub fn init_schema(conn: &Connection) -> Result<()> {
    // `local_playlists.folder_id` references the shared sidebar folders table
    // owned by `LibraryDatabase`. Create it here too so this module's schema
    // is self-contained (unit tests open an in-memory DB and only call
    // `init_schema`; production already creates `playlist_folders` first).
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS playlist_folders (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            icon_type TEXT DEFAULT 'preset',
            icon_preset TEXT DEFAULT 'folder',
            icon_color TEXT DEFAULT '#6366f1',
            custom_image_path TEXT,
            is_hidden INTEGER DEFAULT 0,
            position INTEGER DEFAULT 0,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS local_playlists (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            description TEXT,
            offline_only INTEGER NOT NULL DEFAULT 0,
            favorite INTEGER NOT NULL DEFAULT 0,
            hidden INTEGER NOT NULL DEFAULT 0,
            custom_artwork_path TEXT,
            -- Sidebar folder membership. Points at the SHARED playlist_folders
            -- table (the same folders Qobuz playlists use); folder org is a
            -- QBZ-side concept, so local playlists join the same folders. Local
            -- ids are strings, so they could never live in playlist_settings
            -- (u64 PK) — the membership rides here instead.
            folder_id TEXT REFERENCES playlist_folders(id) ON DELETE SET NULL,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS local_playlist_tracks (
            playlist_id TEXT NOT NULL REFERENCES local_playlists(id) ON DELETE CASCADE,
            position INTEGER NOT NULL,
            source TEXT NOT NULL,           -- 'qobuz' | 'local' | 'plex'
            qobuz_track_id INTEGER,
            local_path TEXT,
            plex_key TEXT,
            added_at INTEGER NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_local_playlist_tracks_playlist
            ON local_playlist_tracks(playlist_id, position);

        -- Inverse direction for the Add-to-Playlist picker's containment
        -- question (playlist_membership.rs). Partial: each row fills exactly
        -- one of the three identity columns.
        CREATE INDEX IF NOT EXISTS idx_local_playlist_tracks_qobuz
            ON local_playlist_tracks(qobuz_track_id, playlist_id)
            WHERE qobuz_track_id IS NOT NULL;
        CREATE INDEX IF NOT EXISTS idx_local_playlist_tracks_path
            ON local_playlist_tracks(local_path, playlist_id)
            WHERE local_path IS NOT NULL;
        CREATE INDEX IF NOT EXISTS idx_local_playlist_tracks_plex
            ON local_playlist_tracks(plex_key, playlist_id)
            WHERE plex_key IS NOT NULL;
        "#,
    )?;
    // Additive migration (B3): DBs created before the favorite/hidden
    // columns existed. Pragma-guarded ALTER, the database.rs idiom.
    let has_favorite: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('local_playlists') WHERE name = 'favorite'",
            [],
            |r| r.get::<_, i32>(0),
        )
        .map(|count| count > 0)
        .unwrap_or(false);
    if !has_favorite {
        conn.execute_batch(
            "ALTER TABLE local_playlists ADD COLUMN favorite INTEGER NOT NULL DEFAULT 0;
             ALTER TABLE local_playlists ADD COLUMN hidden INTEGER NOT NULL DEFAULT 0;",
        )?;
    }
    // Additive migration (folder membership): DBs created before the folder_id
    // column. The REFERENCES clause is accepted by ALTER because the default is
    // NULL and the app's connections don't enable the foreign_keys pragma.
    let has_folder: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('local_playlists') WHERE name = 'folder_id'",
            [],
            |r| r.get::<_, i32>(0),
        )
        .map(|count| count > 0)
        .unwrap_or(false);
    if !has_folder {
        conn.execute_batch(
            "ALTER TABLE local_playlists ADD COLUMN folder_id TEXT \
             REFERENCES playlist_folders(id) ON DELETE SET NULL;",
        )?;
    }
    Ok(())
}

// ──────────────────────────── Playlist CRUD ────────────────────────────

/// Create a playlist; returns its `local:<uuid>` id.
pub fn create(
    conn: &Connection,
    name: &str,
    description: Option<&str>,
    offline_only: bool,
) -> Result<String> {
    let id = format!("{LOCAL_PLAYLIST_PREFIX}{}", Uuid::new_v4());
    let ts = now_ms();
    conn.execute(
        "INSERT INTO local_playlists (id, name, description, offline_only, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
        params![id, name, description, offline_only as i64, ts],
    )?;
    Ok(id)
}

pub fn rename(conn: &Connection, id: &str, new_name: &str) -> Result<()> {
    conn.execute(
        "UPDATE local_playlists SET name = ?1, updated_at = ?2 WHERE id = ?3",
        params![new_name, now_ms(), id],
    )?;
    Ok(())
}

pub fn set_description(conn: &Connection, id: &str, description: Option<&str>) -> Result<()> {
    conn.execute(
        "UPDATE local_playlists SET description = ?1, updated_at = ?2 WHERE id = ?3",
        params![description, now_ms(), id],
    )?;
    Ok(())
}

pub fn set_offline_only(conn: &Connection, id: &str, offline_only: bool) -> Result<()> {
    conn.execute(
        "UPDATE local_playlists SET offline_only = ?1, updated_at = ?2 WHERE id = ?3",
        params![offline_only as i64, now_ms(), id],
    )?;
    Ok(())
}

/// B3: flip the manager's favorite flag (local twin of
/// `playlist_settings.is_favorite`).
pub fn set_favorite(conn: &Connection, id: &str, favorite: bool) -> Result<()> {
    conn.execute(
        "UPDATE local_playlists SET favorite = ?1, updated_at = ?2 WHERE id = ?3",
        params![favorite as i64, now_ms(), id],
    )?;
    Ok(())
}

/// B3: flip the manager's hidden flag (local twin of
/// `playlist_settings.hidden`). Hidden playlists drop from the sidebar.
pub fn set_hidden(conn: &Connection, id: &str, hidden: bool) -> Result<()> {
    conn.execute(
        "UPDATE local_playlists SET hidden = ?1, updated_at = ?2 WHERE id = ?3",
        params![hidden as i64, now_ms(), id],
    )?;
    Ok(())
}

/// Move a local playlist into a folder (`Some(folder_id)`) or back to the
/// sidebar root (`None`). The folder lives in the shared `playlist_folders`
/// table — the same folders Qobuz playlists use.
pub fn move_to_folder(conn: &Connection, id: &str, folder_id: Option<&str>) -> Result<()> {
    conn.execute(
        "UPDATE local_playlists SET folder_id = ?1, updated_at = ?2 WHERE id = ?3",
        params![folder_id, now_ms(), id],
    )?;
    Ok(())
}

/// Null the `folder_id` of every local playlist that pointed at `folder_id`.
/// Called when a folder is deleted: the schema's `ON DELETE SET NULL` only
/// fires when the `foreign_keys` pragma is on, and the app's connections keep
/// it off, so do it explicitly (the same reason `delete` clears tracks by hand).
pub fn clear_folder(conn: &Connection, folder_id: &str) -> Result<()> {
    conn.execute(
        "UPDATE local_playlists SET folder_id = NULL WHERE folder_id = ?1",
        params![folder_id],
    )?;
    Ok(())
}

pub fn set_custom_artwork(conn: &Connection, id: &str, path: Option<&str>) -> Result<()> {
    conn.execute(
        "UPDATE local_playlists SET custom_artwork_path = ?1, updated_at = ?2 WHERE id = ?3",
        params![path, now_ms(), id],
    )?;
    Ok(())
}

/// Delete the playlist. Membership rows are removed explicitly as well as
/// by the FK cascade — `PRAGMA foreign_keys` is connection-scoped and the
/// app's connections don't enable it, so don't rely on the cascade alone.
pub fn delete(conn: &Connection, id: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM local_playlist_tracks WHERE playlist_id = ?1",
        params![id],
    )?;
    conn.execute("DELETE FROM local_playlists WHERE id = ?1", params![id])?;
    Ok(())
}

fn row_to_playlist(r: &rusqlite::Row) -> Result<LocalPlaylist> {
    Ok(LocalPlaylist {
        id: r.get("id")?,
        name: r.get("name")?,
        description: r.get("description")?,
        offline_only: r.get::<_, i64>("offline_only")? != 0,
        favorite: r.get::<_, i64>("favorite")? != 0,
        hidden: r.get::<_, i64>("hidden")? != 0,
        custom_artwork_path: r.get("custom_artwork_path")?,
        folder_id: r.get("folder_id")?,
        created_at: r.get("created_at")?,
        updated_at: r.get("updated_at")?,
        track_count: 0,
        qobuz_count: 0,
        local_count: 0,
        plex_count: 0,
    })
}

/// Fill the per-source counts on a loaded playlist header.
fn hydrate_counts(conn: &Connection, p: &mut LocalPlaylist) -> Result<()> {
    let mut stmt = conn.prepare(
        "SELECT source, COUNT(*) FROM local_playlist_tracks
         WHERE playlist_id = ?1 GROUP BY source",
    )?;
    let rows = stmt.query_map(params![p.id], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, u32>(1)?))
    })?;
    for row in rows {
        let (source, count) = row?;
        match LocalPlaylistTrackSource::parse(&source) {
            LocalPlaylistTrackSource::Qobuz => p.qobuz_count = count,
            LocalPlaylistTrackSource::Local => p.local_count = count,
            // The three server-backed sources share one bucket: every
            // consumer of these counts distinguishes "on this machine" from
            // "needs a server", not server brands.
            LocalPlaylistTrackSource::Plex
            | LocalPlaylistTrackSource::Jellyfin
            | LocalPlaylistTrackSource::Subsonic => p.plex_count += count,
        }
        p.track_count += count;
    }
    Ok(())
}

/// All local playlists (counts hydrated), newest first.
pub fn list(conn: &Connection) -> Result<Vec<LocalPlaylist>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, description, offline_only, favorite, hidden,
                custom_artwork_path, folder_id, created_at, updated_at
           FROM local_playlists
          ORDER BY created_at DESC",
    )?;
    let mut out: Vec<LocalPlaylist> = Vec::new();
    for r in stmt.query_map([], row_to_playlist)? {
        out.push(r?);
    }
    for p in out.iter_mut() {
        hydrate_counts(conn, p)?;
    }
    Ok(out)
}

/// One playlist header (counts hydrated), or None.
pub fn get(conn: &Connection, id: &str) -> Result<Option<LocalPlaylist>> {
    let maybe = conn
        .query_row(
            "SELECT id, name, description, offline_only, favorite, hidden,
                    custom_artwork_path, folder_id, created_at, updated_at
               FROM local_playlists WHERE id = ?1",
            params![id],
            row_to_playlist,
        )
        .optional()?;
    match maybe {
        Some(mut p) => {
            hydrate_counts(conn, &mut p)?;
            Ok(Some(p))
        }
        None => Ok(None),
    }
}

// ──────────────────────────── Track CRUD ────────────────────────────

/// Membership rows in position order.
pub fn get_tracks(conn: &Connection, playlist_id: &str) -> Result<Vec<LocalPlaylistTrack>> {
    let mut stmt = conn.prepare(
        "SELECT playlist_id, position, source, qobuz_track_id, local_path, plex_key, added_at
           FROM local_playlist_tracks
          WHERE playlist_id = ?1
          ORDER BY position ASC",
    )?;
    let mut out = Vec::new();
    for r in stmt.query_map(params![playlist_id], |r| {
        Ok(LocalPlaylistTrack {
            playlist_id: r.get("playlist_id")?,
            position: r.get("position")?,
            source: LocalPlaylistTrackSource::parse(&r.get::<_, String>("source")?),
            qobuz_track_id: r.get::<_, Option<i64>>("qobuz_track_id")?.map(|v| v as u64),
            local_path: r.get("local_path")?,
            plex_key: r.get("plex_key")?,
            added_at: r.get("added_at")?,
        })
    })? {
        out.push(r?);
    }
    Ok(out)
}

/// Append tracks at the end (positions continue after the current max).
/// Exact duplicates (same source + same ref already in the playlist) are
/// skipped silently (the sidecar tables' UNIQUE idempotence, kept here by
/// an explicit existence check since position is part of every row).
/// Returns the number of rows actually inserted.
pub fn add_tracks(
    conn: &Connection,
    playlist_id: &str,
    entries: &[LocalPlaylistTrackInput],
) -> Result<usize> {
    let mut next_pos: i32 = conn.query_row(
        "SELECT COALESCE(MAX(position), -1) + 1
           FROM local_playlist_tracks WHERE playlist_id = ?1",
        params![playlist_id],
        |r| r.get(0),
    )?;
    let ts = now_ms();
    let mut inserted = 0usize;
    for entry in entries {
        let (source, qobuz_id, local_path, plex_key): (
            &str,
            Option<i64>,
            Option<&str>,
            Option<&str>,
        ) = match entry {
            LocalPlaylistTrackInput::Qobuz(id) => ("qobuz", Some(*id as i64), None, None),
            LocalPlaylistTrackInput::Local(path) => ("local", None, Some(path.as_str()), None),
            LocalPlaylistTrackInput::Plex(key) => ("plex", None, None, Some(key.as_str())),
            LocalPlaylistTrackInput::Jellyfin(item) => {
                ("jellyfin", None, Some(item.as_str()), None)
            }
            LocalPlaylistTrackInput::Subsonic(item) => {
                ("subsonic", None, Some(item.as_str()), None)
            }
        };
        let exists: bool = conn
            .prepare(
                "SELECT 1 FROM local_playlist_tracks
                  WHERE playlist_id = ?1 AND source = ?2
                    AND COALESCE(qobuz_track_id, -1) = COALESCE(?3, -1)
                    AND COALESCE(local_path, '') = COALESCE(?4, '')
                    AND COALESCE(plex_key, '') = COALESCE(?5, '')
                  LIMIT 1",
            )?
            .exists(params![playlist_id, source, qobuz_id, local_path, plex_key])?;
        if exists {
            continue;
        }
        conn.execute(
            "INSERT INTO local_playlist_tracks
                (playlist_id, position, source, qobuz_track_id, local_path, plex_key, added_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![playlist_id, next_pos, source, qobuz_id, local_path, plex_key, ts],
        )?;
        next_pos += 1;
        inserted += 1;
    }
    if inserted > 0 {
        conn.execute(
            "UPDATE local_playlists SET updated_at = ?1 WHERE id = ?2",
            params![ts, playlist_id],
        )?;
    }
    Ok(inserted)
}

/// Move the row at `from` to `to` (both repo positions), shifting the rows
/// in between by one — remove-then-insert list semantics, so the moved row
/// lands at exactly position `to` (B2: the local detail's custom reorder).
/// `from == to`, or either position not naming an existing row, is a no-op.
pub fn reorder(conn: &Connection, playlist_id: &str, from: i32, to: i32) -> Result<()> {
    if from == to {
        return Ok(());
    }
    // Both endpoints must name existing rows, or the shift below would
    // corrupt the order (and -1 is the in-flight parking slot).
    let mut exists_stmt = conn.prepare(
        "SELECT 1 FROM local_playlist_tracks
          WHERE playlist_id = ?1 AND position = ?2 LIMIT 1",
    )?;
    if !exists_stmt.exists(params![playlist_id, from])?
        || !exists_stmt.exists(params![playlist_id, to])?
    {
        return Ok(());
    }
    conn.execute(
        "UPDATE local_playlist_tracks SET position = -1
          WHERE playlist_id = ?1 AND position = ?2",
        params![playlist_id, from],
    )?;
    if from < to {
        conn.execute(
            "UPDATE local_playlist_tracks SET position = position - 1
              WHERE playlist_id = ?1 AND position > ?2 AND position <= ?3",
            params![playlist_id, from, to],
        )?;
    } else {
        conn.execute(
            "UPDATE local_playlist_tracks SET position = position + 1
              WHERE playlist_id = ?1 AND position >= ?2 AND position < ?3",
            params![playlist_id, to, from],
        )?;
    }
    conn.execute(
        "UPDATE local_playlist_tracks SET position = ?2
          WHERE playlist_id = ?1 AND position = -1",
        params![playlist_id, to],
    )?;
    conn.execute(
        "UPDATE local_playlists SET updated_at = ?1 WHERE id = ?2",
        params![now_ms(), playlist_id],
    )?;
    Ok(())
}

/// Remove the row at `position` and compact the positions above it.
pub fn remove_track(conn: &Connection, playlist_id: &str, position: i32) -> Result<()> {
    conn.execute(
        "DELETE FROM local_playlist_tracks WHERE playlist_id = ?1 AND position = ?2",
        params![playlist_id, position],
    )?;
    conn.execute(
        "UPDATE local_playlist_tracks SET position = position - 1
          WHERE playlist_id = ?1 AND position > ?2",
        params![playlist_id, position],
    )?;
    conn.execute(
        "UPDATE local_playlists SET updated_at = ?1 WHERE id = ?2",
        params![now_ms(), playlist_id],
    )?;
    Ok(())
}

// ──────────────────────────── Tests ────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn fresh_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();
        conn
    }

    #[test]
    fn create_assigns_namespaced_id_and_roundtrips() {
        let conn = fresh_db();
        let id = create(&conn, "Road Trip", Some("desc"), false).unwrap();
        assert!(is_local_playlist_id(&id), "id must carry the local: prefix");
        let p = get(&conn, &id).unwrap().unwrap();
        assert_eq!(p.name, "Road Trip");
        assert_eq!(p.description.as_deref(), Some("desc"));
        assert!(!p.offline_only);
        assert_eq!(p.track_count, 0);
    }

    #[test]
    fn offline_only_flag_persists_and_flips() {
        let conn = fresh_db();
        let id = create(&conn, "Vault", None, true).unwrap();
        assert!(get(&conn, &id).unwrap().unwrap().offline_only);
        set_offline_only(&conn, &id, false).unwrap();
        assert!(!get(&conn, &id).unwrap().unwrap().offline_only);
    }

    #[test]
    fn rename_and_description_update() {
        let conn = fresh_db();
        let id = create(&conn, "Old", None, false).unwrap();
        rename(&conn, &id, "New").unwrap();
        set_description(&conn, &id, Some("hello")).unwrap();
        let p = get(&conn, &id).unwrap().unwrap();
        assert_eq!(p.name, "New");
        assert_eq!(p.description.as_deref(), Some("hello"));
    }

    #[test]
    fn add_tracks_appends_positions_across_sources() {
        let conn = fresh_db();
        let id = create(&conn, "Mixed", None, false).unwrap();
        let n = add_tracks(
            &conn,
            &id,
            &[
                LocalPlaylistTrackInput::Qobuz(111),
                LocalPlaylistTrackInput::Local("/music/a.flac".into()),
                LocalPlaylistTrackInput::Plex("plex-key-9".into()),
            ],
        )
        .unwrap();
        assert_eq!(n, 3);
        // Second batch continues the position sequence.
        let n2 = add_tracks(&conn, &id, &[LocalPlaylistTrackInput::Qobuz(222)]).unwrap();
        assert_eq!(n2, 1);

        let rows = get_tracks(&conn, &id).unwrap();
        assert_eq!(rows.len(), 4);
        assert_eq!(
            rows.iter().map(|r| r.position).collect::<Vec<_>>(),
            vec![0, 1, 2, 3]
        );
        assert_eq!(rows[0].qobuz_track_id, Some(111));
        assert_eq!(rows[1].local_path.as_deref(), Some("/music/a.flac"));
        assert_eq!(rows[2].plex_key.as_deref(), Some("plex-key-9"));
        assert_eq!(rows[3].qobuz_track_id, Some(222));

        let p = get(&conn, &id).unwrap().unwrap();
        assert_eq!(p.track_count, 4);
        assert_eq!(p.qobuz_count, 2);
        assert_eq!(p.local_count, 1);
        assert_eq!(p.plex_count, 1);
    }

    #[test]
    fn add_tracks_skips_exact_duplicates() {
        let conn = fresh_db();
        let id = create(&conn, "Dedupe", None, false).unwrap();
        add_tracks(&conn, &id, &[LocalPlaylistTrackInput::Qobuz(7)]).unwrap();
        let n = add_tracks(
            &conn,
            &id,
            &[
                LocalPlaylistTrackInput::Qobuz(7),
                LocalPlaylistTrackInput::Local("/x.flac".into()),
            ],
        )
        .unwrap();
        assert_eq!(n, 1, "duplicate qobuz id skipped, new local row inserted");
        assert_eq!(get_tracks(&conn, &id).unwrap().len(), 2);
    }

    #[test]
    fn remove_track_compacts_positions() {
        let conn = fresh_db();
        let id = create(&conn, "Compact", None, false).unwrap();
        add_tracks(
            &conn,
            &id,
            &[
                LocalPlaylistTrackInput::Qobuz(1),
                LocalPlaylistTrackInput::Qobuz(2),
                LocalPlaylistTrackInput::Qobuz(3),
            ],
        )
        .unwrap();
        remove_track(&conn, &id, 1).unwrap();
        let rows = get_tracks(&conn, &id).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].qobuz_track_id, Some(1));
        assert_eq!(rows[0].position, 0);
        assert_eq!(rows[1].qobuz_track_id, Some(3));
        assert_eq!(rows[1].position, 1);
    }

    /// The repo positions, in row order, with the qobuz id per row — the
    /// shape every reorder assertion checks.
    fn qobuz_order(conn: &Connection, id: &str) -> Vec<(i32, u64)> {
        get_tracks(conn, id)
            .unwrap()
            .iter()
            .map(|r| (r.position, r.qobuz_track_id.unwrap()))
            .collect()
    }

    fn seeded_playlist(conn: &Connection, ids: &[u64]) -> String {
        let id = create(conn, "Reorder", None, false).unwrap();
        let entries: Vec<LocalPlaylistTrackInput> = ids
            .iter()
            .map(|&tid| LocalPlaylistTrackInput::Qobuz(tid))
            .collect();
        add_tracks(conn, &id, &entries).unwrap();
        id
    }

    #[test]
    fn reorder_moves_down_with_compaction() {
        let conn = fresh_db();
        let id = seeded_playlist(&conn, &[1, 2, 3, 4]);
        // Move the first row to slot 2: [1,2,3,4] -> [2,3,1,4].
        reorder(&conn, &id, 0, 2).unwrap();
        assert_eq!(
            qobuz_order(&conn, &id),
            vec![(0, 2), (1, 3), (2, 1), (3, 4)]
        );
    }

    #[test]
    fn reorder_moves_up_with_compaction() {
        let conn = fresh_db();
        let id = seeded_playlist(&conn, &[1, 2, 3, 4]);
        // Move the last row to slot 1: [1,2,3,4] -> [1,4,2,3].
        reorder(&conn, &id, 3, 1).unwrap();
        assert_eq!(
            qobuz_order(&conn, &id),
            vec![(0, 1), (1, 4), (2, 2), (3, 3)]
        );
    }

    #[test]
    fn reorder_adjacent_swap() {
        let conn = fresh_db();
        let id = seeded_playlist(&conn, &[1, 2, 3]);
        reorder(&conn, &id, 1, 2).unwrap();
        assert_eq!(qobuz_order(&conn, &id), vec![(0, 1), (1, 3), (2, 2)]);
        reorder(&conn, &id, 2, 1).unwrap();
        assert_eq!(qobuz_order(&conn, &id), vec![(0, 1), (1, 2), (2, 3)]);
    }

    #[test]
    fn reorder_noop_on_same_or_missing_positions() {
        let conn = fresh_db();
        let id = seeded_playlist(&conn, &[1, 2, 3]);
        let before = qobuz_order(&conn, &id);
        reorder(&conn, &id, 1, 1).unwrap(); // same slot
        reorder(&conn, &id, 7, 0).unwrap(); // from doesn't exist
        reorder(&conn, &id, 0, 7).unwrap(); // to doesn't exist
        reorder(&conn, &id, 0, -1).unwrap(); // negative target
        assert_eq!(qobuz_order(&conn, &id), before);
    }

    #[test]
    fn reorder_scoped_to_its_playlist() {
        let conn = fresh_db();
        let a = seeded_playlist(&conn, &[1, 2, 3]);
        let b = seeded_playlist(&conn, &[10, 20, 30]);
        reorder(&conn, &a, 0, 2).unwrap();
        assert_eq!(qobuz_order(&conn, &a), vec![(0, 2), (1, 3), (2, 1)]);
        // The sibling playlist's rows are untouched.
        assert_eq!(
            qobuz_order(&conn, &b),
            vec![(0, 10), (1, 20), (2, 30)]
        );
    }

    #[test]
    fn favorite_and_hidden_default_false_and_flip() {
        let conn = fresh_db();
        let id = create(&conn, "Flags", None, false).unwrap();
        let p = get(&conn, &id).unwrap().unwrap();
        assert!(!p.favorite);
        assert!(!p.hidden);
        set_favorite(&conn, &id, true).unwrap();
        set_hidden(&conn, &id, true).unwrap();
        let p = get(&conn, &id).unwrap().unwrap();
        assert!(p.favorite);
        assert!(p.hidden);
        set_favorite(&conn, &id, false).unwrap();
        let p = get(&conn, &id).unwrap().unwrap();
        assert!(!p.favorite);
        assert!(p.hidden, "flags flip independently");
        // `list` carries the flags too.
        let all = list(&conn).unwrap();
        assert!(all.iter().any(|p| p.id == id && p.hidden && !p.favorite));
    }

    #[test]
    fn init_schema_migrates_pre_b3_table() {
        // A DB created with the pre-B3 shape (no favorite/hidden columns)
        // plus an existing row: init_schema adds the columns with their
        // defaults and leaves the row readable.
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE local_playlists (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                description TEXT,
                offline_only INTEGER NOT NULL DEFAULT 0,
                custom_artwork_path TEXT,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            );
            INSERT INTO local_playlists (id, name, offline_only, created_at, updated_at)
            VALUES ('local:pre-b3', 'Old Row', 0, 1, 1);
            "#,
        )
        .unwrap();
        init_schema(&conn).unwrap();
        let p = get(&conn, "local:pre-b3").unwrap().unwrap();
        assert_eq!(p.name, "Old Row");
        assert!(!p.favorite);
        assert!(!p.hidden);
        // The migrated columns are writable.
        set_hidden(&conn, "local:pre-b3", true).unwrap();
        assert!(get(&conn, "local:pre-b3").unwrap().unwrap().hidden);
        // Idempotent: a second init_schema doesn't re-ALTER.
        init_schema(&conn).unwrap();
    }

    #[test]
    fn delete_cascades_membership_rows() {
        let conn = fresh_db();
        let id = create(&conn, "Doomed", None, true).unwrap();
        add_tracks(&conn, &id, &[LocalPlaylistTrackInput::Qobuz(42)]).unwrap();
        delete(&conn, &id).unwrap();
        assert!(get(&conn, &id).unwrap().is_none());
        assert!(get_tracks(&conn, &id).unwrap().is_empty());
        // The membership table holds no orphans.
        let orphans: i64 = conn
            .query_row("SELECT COUNT(*) FROM local_playlist_tracks", [], |r| r.get(0))
            .unwrap();
        assert_eq!(orphans, 0);
    }

    #[test]
    fn list_returns_all_with_counts() {
        let conn = fresh_db();
        let a = create(&conn, "A", None, false).unwrap();
        let b = create(&conn, "B", None, true).unwrap();
        add_tracks(&conn, &a, &[LocalPlaylistTrackInput::Qobuz(5)]).unwrap();
        let all = list(&conn).unwrap();
        assert_eq!(all.len(), 2);
        let pa = all.iter().find(|p| p.id == a).unwrap();
        let pb = all.iter().find(|p| p.id == b).unwrap();
        assert_eq!(pa.track_count, 1);
        assert!(!pa.offline_only);
        assert_eq!(pb.track_count, 0);
        assert!(pb.offline_only);
    }
}
