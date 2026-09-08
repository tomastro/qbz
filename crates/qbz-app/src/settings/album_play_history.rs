//! Local album play history — source of truth for the "Most Played Albums"
//! rail + its View-all page.
//!
//! Mirrors [`crate::play_history`] (the per-artist store): an
//! `album_play_events` table grows one row per track-start whose album is
//! known, plus a side `album_meta` table (id -> title/artist/artwork/quality)
//! refreshed on each play. "Most played" = `COUNT(*) GROUP BY album_id
//! ORDER BY plays DESC`. Counting is PER TRACK-START, like play_history, so an
//! album listened all the way through adds one per track.
//!
//! The Qobuz API exposes no most-played endpoint (verified against the
//! inferred OpenAPI), so the ranking is derived locally from our own plays.
//!
//! SQLite is opened lazily; every read/write swallows errors into a
//! `log::warn!`. A fresh user (no DB yet) yields an empty list, so the rail
//! self-hides — same default the other #566 rails land on.

use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection};

static DB: OnceLock<Mutex<Option<Connection>>> = OnceLock::new();

fn db_path() -> Option<PathBuf> {
    Some(dirs::data_dir()?.join("qbz").join("album_play_history.db"))
}

/// Create the tables + index on a fresh connection (shared by the lazy opener
/// and the in-memory test connections).
fn init_schema(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS album_play_events (
            album_id TEXT NOT NULL,
            occurred_at INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS album_play_events_album
            ON album_play_events(album_id);

        CREATE TABLE IF NOT EXISTS album_meta (
            album_id      TEXT PRIMARY KEY,
            title         TEXT NOT NULL,
            artist        TEXT NOT NULL,
            artist_id     TEXT NOT NULL DEFAULT '',
            artwork_url   TEXT NOT NULL DEFAULT '',
            quality_tier  TEXT NOT NULL DEFAULT '',
            quality_label TEXT NOT NULL DEFAULT '',
            year          TEXT NOT NULL DEFAULT '',
            source        TEXT NOT NULL DEFAULT '',
            updated_at    INTEGER NOT NULL
        );
        "#,
    )?;
    // PLAYBACK CONTEXT of the event — "album" | "playlist" | "" (unknown).
    //
    // Additive, and it has to be: this .db is shared with existing installs,
    // whose tree is frozen and whose INSERT names its columns explicitly
    // (`INSERT INTO album_play_events (album_id, occurred_at)`), so an extra
    // column with a DEFAULT is invisible to it and it simply writes `''`.
    // Every legacy row is `''` too, and the ranking query treats `''` as
    // "album" — so no history is lost or reinterpreted by this migration.
    //
    // SQLite has no `ADD COLUMN IF NOT EXISTS`; running it and swallowing the
    // duplicate-column error is the idiom, and it is cheaper than a
    // `PRAGMA table_info` round trip on every open.
    let _ = conn.execute_batch(
        "ALTER TABLE album_play_events ADD COLUMN context_kind TEXT NOT NULL DEFAULT '';",
    );
    Ok(())
}

fn open_db() -> Option<Connection> {
    let path = db_path()?;
    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            log::warn!("[qbz-app] album_play_history dir create failed: {e}");
            return None;
        }
    }
    let conn = match Connection::open(&path) {
        Ok(c) => c,
        Err(e) => {
            log::warn!("[qbz-app] album_play_history open failed: {e}");
            return None;
        }
    };
    // ADR-002: WAL for any SQLite store touched off the UI thread.
    if let Err(e) = conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA busy_timeout=1000;") {
        log::warn!("[qbz-app] album_play_history pragma failed: {e}");
    }
    if let Err(e) = init_schema(&conn) {
        log::warn!("[qbz-app] album_play_history schema failed: {e}");
        return None;
    }
    Some(conn)
}

fn with_db<F, T>(f: F) -> Option<T>
where
    F: FnOnce(&Connection) -> Option<T>,
{
    let cell = DB.get_or_init(|| Mutex::new(open_db()));
    let guard = cell.lock().ok()?;
    let conn = guard.as_ref()?;
    f(conn)
}

/// Album metadata captured at play time (refreshed on every play, so renames
/// and artwork refreshes converge).
pub struct AlbumPlayMeta<'a> {
    pub album_id: &'a str,
    pub title: &'a str,
    pub artist: &'a str,
    pub artist_id: &'a str,
    pub artwork_url: &'a str,
    pub quality_tier: &'a str,
    pub quality_label: &'a str,
    pub year: &'a str,
    pub source: &'a str,
    /// What the user was PLAYING FROM when this event happened: `"album"`,
    /// `"playlist"`, or `""` when nothing was stamped. NOT the same axis as
    /// [`Self::source`], which is where the audio comes from (qobuz / local /
    /// plex) — an album can be local AND played from a playlist.
    pub context_kind: &'a str,
}

/// One ranked album for the "Most Played Albums" rail / View-all page.
#[derive(Clone, Default, Debug, PartialEq)]
pub struct AlbumPlayRow {
    pub album_id: String,
    pub title: String,
    pub artist: String,
    pub artist_id: String,
    pub artwork_url: String,
    pub quality_tier: String,
    pub quality_label: String,
    pub year: String,
    pub source: String,
    pub plays: u32,
}

/// Insert one play event + upsert the album meta, at an explicit timestamp.
/// Internal so tests can drive it against an in-memory connection.
fn record_on(conn: &Connection, m: &AlbumPlayMeta, now: i64) {
    if let Err(e) = conn.execute(
        "INSERT INTO album_play_events (album_id, occurred_at, context_kind) \
         VALUES (?, ?, ?)",
        params![m.album_id, now, m.context_kind],
    ) {
        log::warn!("[qbz-app] album_play_history insert event failed: {e}");
    }
    if let Err(e) = conn.execute(
        r#"
        INSERT INTO album_meta
            (album_id, title, artist, artist_id, artwork_url,
             quality_tier, quality_label, year, source, updated_at)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        ON CONFLICT(album_id) DO UPDATE SET
            title = excluded.title,
            artist = excluded.artist,
            artist_id = excluded.artist_id,
            artwork_url = excluded.artwork_url,
            quality_tier = excluded.quality_tier,
            quality_label = excluded.quality_label,
            year = excluded.year,
            source = excluded.source,
            updated_at = excluded.updated_at
        "#,
        params![
            m.album_id,
            m.title,
            m.artist,
            m.artist_id,
            m.artwork_url,
            m.quality_tier,
            m.quality_label,
            m.year,
            m.source,
            now
        ],
    ) {
        log::warn!("[qbz-app] album_play_history upsert meta failed: {e}");
    }
}

/// Rank albums by play count (desc), tie-broken by most-recent play so ties
/// are stable and intuitive. `limit` caps the carousel; `None` = full list.
fn query_on(conn: &Connection, limit: Option<u32>) -> Vec<AlbumPlayRow> {
    let sql = format!(
        r#"
        SELECT m.album_id, m.title, m.artist, m.artist_id, m.artwork_url,
               m.quality_tier, m.quality_label, m.year, m.source, p.plays
        FROM album_meta m
        JOIN (
            SELECT album_id, COUNT(*) AS plays, MAX(occurred_at) AS last_at
            FROM album_play_events
            -- Only plays that were ABOUT the album. A playlist of 40 tracks
            -- from 40 albums used to add one play to each of them, which is
            -- how "Most Played Albums" came to be a list of whatever playlist
            -- the user had on. `''` is a legacy row or one written by the
            -- other frontend, and is counted — dropping it would erase
            -- history that is almost entirely genuine album listening.
            WHERE context_kind IN ('', 'album')
            GROUP BY album_id
        ) p ON p.album_id = m.album_id
        ORDER BY p.plays DESC, p.last_at DESC
        {}
        "#,
        limit.map(|n| format!("LIMIT {n}")).unwrap_or_default()
    );
    let out = (|| -> Option<Vec<AlbumPlayRow>> {
        let mut stmt = conn.prepare(&sql).ok()?;
        let rows = stmt
            .query_map([], |row| {
                Ok(AlbumPlayRow {
                    album_id: row.get(0)?,
                    title: row.get(1)?,
                    artist: row.get(2)?,
                    artist_id: row.get(3)?,
                    artwork_url: row.get(4)?,
                    quality_tier: row.get(5)?,
                    quality_label: row.get(6)?,
                    year: row.get(7)?,
                    source: row.get(8)?,
                    plays: row.get::<_, i64>(9)? as u32,
                })
            })
            .ok()?;
        Some(rows.flatten().collect())
    })();
    out.unwrap_or_default()
}

/// Record a play. Called from `playback::record_recent` when a track starts
/// audible playback. No-op when the album id is empty (some local/Plex
/// sources carry none — same guard as the recently-played rail).
pub fn record_album_play(m: AlbumPlayMeta) {
    if m.album_id.is_empty() {
        return;
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    with_db(|conn| {
        record_on(conn, &m, now);
        Some(())
    });
}

/// The top `limit` most-played albums (the carousel).
pub fn top_albums(limit: u32) -> Vec<AlbumPlayRow> {
    with_db(|conn| Some(query_on(conn, Some(limit)))).unwrap_or_default()
}

/// Every played album, ranked (the "View all" page).
pub fn all_albums() -> Vec<AlbumPlayRow> {
    with_db(|conn| Some(query_on(conn, None))).unwrap_or_default()
}

/// Drop every trace of the given albums — the twin of
/// `playlist_play_history::prune_playlists`, for albums whose library folder
/// was removed. Until this existed "Most Played Albums" kept cards that
/// opened onto deleted tracks while Recently Played had already pruned them.
/// Returns how many meta rows went.
pub fn prune_albums(album_ids: &[String]) -> usize {
    if album_ids.is_empty() {
        return 0;
    }
    with_db(|conn| Some(prune_on(conn, album_ids)))
        .unwrap_or(0)
}

fn prune_on(conn: &Connection, album_ids: &[String]) -> usize {
    let mut gone = 0usize;
    for id in album_ids {
        let _ = conn.execute(
            "DELETE FROM album_play_events WHERE album_id = ?",
            rusqlite::params![id],
        );
        gone += conn
            .execute(
                "DELETE FROM album_meta WHERE album_id = ?",
                rusqlite::params![id],
            )
            .unwrap_or(0);
    }
    gone
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta<'a>(id: &'a str, title: &'a str) -> AlbumPlayMeta<'a> {
        AlbumPlayMeta {
            album_id: id,
            title,
            artist: "Artist",
            artist_id: "7",
            artwork_url: "http://art",
            quality_tier: "hires",
            quality_label: "Hi-Res",
            year: "2024",
            source: "qobuz",
            context_kind: "album",
        }
    }

    fn mem() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        init_schema(&c).unwrap();
        c
    }

    #[test]
    fn ranks_by_play_count_desc() {
        let c = mem();
        // A: 12 plays, B: 20 plays, C: 36 plays (per-track-start counting).
        for i in 0..12 {
            record_on(&c, &meta("A", "Album A"), 100 + i);
        }
        for i in 0..20 {
            record_on(&c, &meta("B", "Album B"), 200 + i);
        }
        for i in 0..36 {
            record_on(&c, &meta("C", "Album C"), 300 + i);
        }
        let rows = query_on(&c, None);
        assert_eq!(
            rows.iter().map(|r| (r.album_id.as_str(), r.plays)).collect::<Vec<_>>(),
            vec![("C", 36), ("B", 20), ("A", 12)]
        );
        // Meta round-trips.
        assert_eq!(rows[0].title, "Album C");
        assert_eq!(rows[0].artist, "Artist");
        assert_eq!(rows[0].quality_tier, "hires");
    }

    #[test]
    fn tie_break_prefers_more_recent_play() {
        let c = mem();
        // Both albums have 2 plays; B's last play is later -> B leads.
        record_on(&c, &meta("A", "A"), 10);
        record_on(&c, &meta("A", "A"), 11);
        record_on(&c, &meta("B", "B"), 20);
        record_on(&c, &meta("B", "B"), 21);
        let rows = query_on(&c, None);
        assert_eq!(rows.iter().map(|r| r.album_id.clone()).collect::<Vec<_>>(), vec!["B", "A"]);
    }

    #[test]
    fn limit_caps_the_carousel() {
        let c = mem();
        for n in 0..5 {
            let id = format!("id{n}");
            // n+1 plays so ordering is deterministic (id4 highest).
            for i in 0..=n {
                record_on(&c, &meta(&id, "t"), 100 + n as i64 * 10 + i as i64);
            }
        }
        let top = query_on(&c, Some(3));
        assert_eq!(top.len(), 3);
        assert_eq!(top.iter().map(|r| r.album_id.clone()).collect::<Vec<_>>(), vec!["id4", "id3", "id2"]);
    }

    #[test]
    fn empty_history_is_empty() {
        let c = mem();
        assert!(query_on(&c, None).is_empty());
        assert!(query_on(&c, Some(20)).is_empty());
    }

    #[test]
    fn meta_upsert_refreshes_on_replay() {
        let c = mem();
        record_on(&c, &meta("A", "Old Title"), 1);
        let mut m2 = meta("A", "New Title");
        m2.artwork_url = "http://new-art";
        record_on(&c, &m2, 2);
        let rows = query_on(&c, None);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].title, "New Title");
        assert_eq!(rows[0].artwork_url, "http://new-art");
        assert_eq!(rows[0].plays, 2);
    }
}
