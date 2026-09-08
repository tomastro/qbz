//! Local PLAYLIST play history — source of truth for the "Recently Played
//! Playlists" rail.
//!
//! The structural twin of [`crate::settings::album_play_history`], and it
//! exists because that module answered the wrong question. Playing a playlist
//! of 40 tracks drawn from 40 albums used to write 40 ALBUM plays, so
//! "Recently Played" became a list of whatever playlist was on and the
//! playlist itself was recorded nowhere. Contexts are now separated at the
//! write edge (`recently_qt::record_queue_track` reads `QueueTrack
//! ::context_kind`), and this is where the playlist half lands.
//!
//! # Why a NEW file and not a column somewhere
//!
//! `recently_played.json` is a DESTRUCTIVE round trip between the two
//! frontends: the Slint build deserializes it into its own structs, serde
//! drops the fields it does not know, and its next write puts the truncated
//! object back. Anything Qt added there would disappear the first time the
//! other build played a track, intermittently and with nothing logged. A .db
//! the frozen tree has never heard of cannot be destroyed that way — it never
//! opens it.
//!
//! One event per TRACK-START, exactly like the album store (see its header):
//! the de-dup for "recently played" happens in the `GROUP BY`, so listening to
//! forty tracks of one playlist yields ONE card with `plays = 40`.
//!
//! SQLite is opened lazily and every read/write swallows its errors into a
//! `log::warn!`; a fresh user with no .db yet gets an empty list, so the rail
//! self-hides.

use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection};

static DB: OnceLock<Mutex<Option<Connection>>> = OnceLock::new();

fn db_path() -> Option<PathBuf> {
    Some(dirs::data_dir()?.join("qbz").join("playlist_play_history.db"))
}

/// Create the tables + index on a fresh connection (shared by the lazy opener
/// and the in-memory test connections).
fn init_schema(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS playlist_play_events (
            playlist_id TEXT NOT NULL,
            occurred_at INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS playlist_play_events_pl
            ON playlist_play_events(playlist_id);

        CREATE TABLE IF NOT EXISTS playlist_meta (
            playlist_id TEXT PRIMARY KEY,
            title       TEXT NOT NULL,
            owner       TEXT NOT NULL DEFAULT '',
            owner_id    TEXT NOT NULL DEFAULT '',
            artwork_url TEXT NOT NULL DEFAULT '',
            track_count INTEGER NOT NULL DEFAULT 0,
            source      TEXT NOT NULL DEFAULT '',
            updated_at  INTEGER NOT NULL
        );
        "#,
    )?;
    // COVERS + OWN_IMAGE ARE A MIGRATION, not part of the CREATE above, and
    // the difference is the whole bug this fixes.
    //
    // They were originally inlined into the CREATE. `CREATE TABLE IF NOT
    // EXISTS` does NOTHING when the table is already there, so anyone who had
    // run the previous build kept the old six-column table, every upsert then
    // failed on "no such column: covers" — into a swallowed `log::warn!` — and
    // `playlist_meta` stayed empty while `playlist_play_events` filled up
    // normally. The rail JOINs the two, so it rendered its empty state through
    // eight tracks, a restart and a manual refresh, with the plays sitting
    // right there in the other table.
    //
    // A brand-new table is only new ONCE: the moment a build runs anywhere, its
    // schema is legacy and every later column is a migration. Same idempotent
    // ADD COLUMN the album history already uses.
    //
    // Up to four MEMBER covers as a JSON array, for the card's mosaic arm —
    // most playlists have no graphic of their own, which is exactly why the
    // detail page falls back to a collage.
    let _ = conn.execute_batch(
        "ALTER TABLE playlist_meta ADD COLUMN covers TEXT NOT NULL DEFAULT '[]';",
    );
    // The playlist HAS a single graphic of its own (or a custom cover), which
    // the card contain-fits instead of building a mosaic.
    let _ = conn.execute_batch(
        "ALTER TABLE playlist_meta ADD COLUMN own_image INTEGER NOT NULL DEFAULT 0;",
    );
    Ok(())
}

fn open_db() -> Option<Connection> {
    let path = db_path()?;
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let conn = match Connection::open(&path) {
        Ok(c) => c,
        Err(e) => {
            log::warn!("[qbz] playlist_play_history open failed: {e}");
            return None;
        }
    };
    // ADR-002: WAL, so a read from the render path never blocks on a write.
    let _ = conn.execute_batch(
        "PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA busy_timeout=1000;",
    );
    if let Err(e) = init_schema(&conn) {
        log::warn!("[qbz] playlist_play_history schema failed: {e}");
        return None;
    }
    Some(conn)
}

fn with_db<T, F>(f: F) -> Option<T>
where
    F: FnOnce(&Connection) -> Option<T>,
{
    let cell = DB.get_or_init(|| Mutex::new(open_db()));
    let guard = cell.lock().ok()?;
    let conn = guard.as_ref()?;
    f(conn)
}

/// Playlist metadata captured at play time (refreshed on every play, so
/// renames and cover changes converge).
pub struct PlaylistPlayMeta<'a> {
    /// The Qobuz numeric id as text, or `"local:<uuid>"` for a first-class
    /// local playlist. It is whatever `PlayContext::playlist(..)` stamped, so
    /// the event and the card can never key differently.
    pub playlist_id: &'a str,
    pub title: &'a str,
    pub owner: &'a str,
    pub owner_id: &'a str,
    pub artwork_url: &'a str,
    pub track_count: u32,
    /// `"qobuz"` | `"local"` — where the playlist itself lives, NOT where its
    /// audio comes from (a Qobuz playlist can hold local rows).
    pub source: &'a str,
    /// Member covers for the mosaic (up to four). Remote urls for a Qobuz
    /// playlist, local file paths for a local one — the artwork cache resolves
    /// both, so the card does not care which it got.
    pub covers: &'a [String],
    /// True when [`Self::artwork_url`] is the playlist's OWN single graphic
    /// (or a custom cover) rather than a stand-in member cover.
    pub own_image: bool,
}

/// One playlist row for the rail / a View-all page.
#[derive(Clone, Default, Debug, PartialEq)]
pub struct PlaylistPlayRow {
    pub playlist_id: String,
    pub title: String,
    pub owner: String,
    pub owner_id: String,
    pub artwork_url: String,
    pub track_count: u32,
    pub source: String,
    pub covers: Vec<String>,
    pub own_image: bool,
    pub plays: u32,
}

/// Upsert the meta WITHOUT recording a play.
///
/// The two halves are separate on purpose. The meta is known by the view that
/// starts playback (it has the header in hand); the event is written at the
/// track-start edge, which only ever sees a `QueueTrack` and its
/// `context_id`. Splitting them keeps the edge from having to resolve a
/// playlist id back into a title on every single track change.
fn upsert_meta_on(conn: &Connection, m: &PlaylistPlayMeta, now: i64) {
    if let Err(e) = conn.execute(
        r#"
        INSERT INTO playlist_meta
            (playlist_id, title, owner, owner_id, artwork_url,
             track_count, source, covers, own_image, updated_at)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        ON CONFLICT(playlist_id) DO UPDATE SET
            title = excluded.title,
            owner = excluded.owner,
            owner_id = excluded.owner_id,
            artwork_url = excluded.artwork_url,
            track_count = excluded.track_count,
            source = excluded.source,
            covers = excluded.covers,
            own_image = excluded.own_image,
            updated_at = excluded.updated_at
        "#,
        params![
            m.playlist_id,
            m.title,
            m.owner,
            m.owner_id,
            m.artwork_url,
            m.track_count,
            m.source,
            serde_json::to_string(m.covers).unwrap_or_else(|_| "[]".to_string()),
            m.own_image as i32,
            now
        ],
    ) {
        // ERROR, not warn: this is the half of the pair whose absence makes the
        // rail render its empty state while the events pile up next to it — a
        // failure that is invisible in the UI by construction.
        log::error!("[qbz] playlist_play_history upsert meta failed: {e}");
    }
}

fn record_event_on(conn: &Connection, playlist_id: &str, now: i64) {
    if let Err(e) = conn.execute(
        "INSERT INTO playlist_play_events (playlist_id, occurred_at) VALUES (?, ?)",
        params![playlist_id, now],
    ) {
        log::warn!("[qbz] playlist_play_history insert event failed: {e}");
    }
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Remember what a playlist IS, so a later play event can render a card.
/// Called by the views that start playback from a playlist. No-op on an empty
/// id.
pub fn record_playlist_meta(m: PlaylistPlayMeta) {
    if m.playlist_id.is_empty() {
        return;
    }
    let now = now_secs();
    with_db(|conn| {
        upsert_meta_on(conn, &m, now);
        Some(())
    });
}

/// Record one play event. Called at the track-start edge when the track's
/// playback context is a playlist. No-op on an empty id.
pub fn record_playlist_play(playlist_id: &str) {
    if playlist_id.is_empty() {
        return;
    }
    let now = now_secs();
    with_db(|conn| {
        record_event_on(conn, playlist_id, now);
        Some(())
    });
}

/// `order_desc` is the ORDER BY body — the only difference between "recently
/// played" and "most played", which is why both come out of one query.
fn query_on(conn: &Connection, order: &str, limit: Option<u32>) -> Vec<PlaylistPlayRow> {
    let sql = format!(
        r#"
        SELECT m.playlist_id, m.title, m.owner, m.owner_id, m.artwork_url,
               m.track_count, m.source, m.covers, m.own_image, p.plays
        FROM playlist_meta m
        JOIN (
            SELECT playlist_id, COUNT(*) AS plays, MAX(occurred_at) AS last_at
            FROM playlist_play_events
            GROUP BY playlist_id
        ) p ON p.playlist_id = m.playlist_id
        ORDER BY {order}
        {}
        "#,
        limit.map(|n| format!("LIMIT {n}")).unwrap_or_default()
    );
    let out = (|| -> Option<Vec<PlaylistPlayRow>> {
        let mut stmt = conn.prepare(&sql).ok()?;
        let rows = stmt
            .query_map([], |row| {
                Ok(PlaylistPlayRow {
                    playlist_id: row.get(0)?,
                    title: row.get(1)?,
                    owner: row.get(2)?,
                    owner_id: row.get(3)?,
                    artwork_url: row.get(4)?,
                    track_count: row.get::<_, i64>(5)? as u32,
                    source: row.get(6)?,
                    covers: serde_json::from_str(&row.get::<_, String>(7)?)
                        .unwrap_or_default(),
                    own_image: row.get::<_, i64>(8)? != 0,
                    plays: row.get::<_, i64>(9)? as u32,
                })
            })
            .ok()?;
        Some(rows.flatten().collect())
    })();
    out.unwrap_or_default()
}

/// The `limit` most recently played playlists (the rail).
pub fn recent_playlists(limit: u32) -> Vec<PlaylistPlayRow> {
    with_db(|conn| Some(query_on(conn, "p.last_at DESC", Some(limit)))).unwrap_or_default()
}

/// Every played playlist, most recent first (a "View all" page).
pub fn all_recent_playlists() -> Vec<PlaylistPlayRow> {
    with_db(|conn| Some(query_on(conn, "p.last_at DESC", None))).unwrap_or_default()
}

/// Ranked by play count. Not wired to a rail — the owner asked for one rail,
/// and this falls out of the same table for free when a second one is wanted.
#[allow(dead_code)]
pub fn top_playlists(limit: u32) -> Vec<PlaylistPlayRow> {
    with_db(|conn| Some(query_on(conn, "p.plays DESC, p.last_at DESC", Some(limit))))
        .unwrap_or_default()
}

/// Drop every trace of the given playlists — the parity of
/// `recently_qt::prune_albums`, for a playlist the user deleted or unfollowed.
/// Returns how many meta rows went.
pub fn prune_playlists(ids: &[String]) -> usize {
    if ids.is_empty() {
        return 0;
    }
    with_db(|conn| {
        let mut gone = 0usize;
        for id in ids {
            let _ = conn.execute(
                "DELETE FROM playlist_play_events WHERE playlist_id = ?",
                params![id],
            );
            gone += conn
                .execute("DELETE FROM playlist_meta WHERE playlist_id = ?", params![id])
                .unwrap_or(0);
        }
        Some(gone)
    })
    .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta<'a>(id: &'a str, title: &'a str) -> PlaylistPlayMeta<'a> {
        PlaylistPlayMeta {
            playlist_id: id,
            title,
            owner: "Someone",
            owner_id: "42",
            artwork_url: "http://art",
            track_count: 12,
            source: "qobuz",
            covers: &[],
            own_image: false,
        }
    }

    fn mem() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        init_schema(&c).unwrap();
        c
    }

    #[test]
    fn recent_orders_by_last_play_not_by_count() {
        let c = mem();
        // A is played far more, B far more RECENTLY. "Recently played" must
        // put B first — that is the whole difference from "most played".
        upsert_meta_on(&c, &meta("A", "Playlist A"), 100);
        upsert_meta_on(&c, &meta("B", "Playlist B"), 100);
        for i in 0..30 {
            record_event_on(&c, "A", 100 + i);
        }
        record_event_on(&c, "B", 9_000);

        let recent = query_on(&c, "p.last_at DESC", Some(10));
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0].playlist_id, "B");
        assert_eq!(recent[1].playlist_id, "A");
        assert_eq!(recent[1].plays, 30);

        let top = query_on(&c, "p.plays DESC, p.last_at DESC", Some(10));
        assert_eq!(top[0].playlist_id, "A");
    }

    #[test]
    fn one_card_per_playlist_however_many_tracks() {
        let c = mem();
        upsert_meta_on(&c, &meta("A", "Playlist A"), 100);
        for i in 0..40 {
            record_event_on(&c, "A", 100 + i);
        }
        let recent = query_on(&c, "p.last_at DESC", None);
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].plays, 40);
    }

    #[test]
    fn meta_without_a_play_does_not_show() {
        // The JOIN is what enforces it: opening a playlist upserts nothing on
        // its own, and even if it did, a card with no play is not "recently
        // played".
        let c = mem();
        upsert_meta_on(&c, &meta("A", "Playlist A"), 100);
        assert!(query_on(&c, "p.last_at DESC", None).is_empty());
    }

    /// THE REGRESSION TEST for the bug the migration above exists to fix: a
    /// database created by the FIRST build, whose `playlist_meta` has neither
    /// `covers` nor `own_image`. `CREATE TABLE IF NOT EXISTS` leaves it alone,
    /// so without the ALTERs the upsert fails on a missing column, the meta
    /// table stays empty, and the rail renders its empty state forever while
    /// the events pile up beside it.
    #[test]
    fn init_schema_migrates_a_pre_covers_database() {
        let c = Connection::open_in_memory().unwrap();
        // The exact shape the first build shipped.
        c.execute_batch(
            r#"
            CREATE TABLE playlist_play_events (
                playlist_id TEXT NOT NULL,
                occurred_at INTEGER NOT NULL
            );
            CREATE TABLE playlist_meta (
                playlist_id TEXT PRIMARY KEY,
                title       TEXT NOT NULL,
                owner       TEXT NOT NULL DEFAULT '',
                owner_id    TEXT NOT NULL DEFAULT '',
                artwork_url TEXT NOT NULL DEFAULT '',
                track_count INTEGER NOT NULL DEFAULT 0,
                source      TEXT NOT NULL DEFAULT '',
                updated_at  INTEGER NOT NULL
            );
            "#,
        )
        .unwrap();
        // A play recorded by that build, which must survive the migration.
        record_event_on(&c, "A", 100);

        init_schema(&c).unwrap();

        // The upsert now works, and the pre-existing event still counts.
        upsert_meta_on(&c, &meta("A", "Playlist A"), 200);
        let rows = query_on(&c, "p.last_at DESC", None);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].playlist_id, "A");
        assert_eq!(rows[0].plays, 1);
        assert!(rows[0].covers.is_empty());
        assert!(!rows[0].own_image);

        // And running it AGAIN is a no-op rather than an error — every launch
        // calls it.
        init_schema(&c).unwrap();
        assert_eq!(query_on(&c, "p.last_at DESC", None).len(), 1);
    }

    #[test]
    fn prune_removes_events_and_meta() {
        let c = mem();
        upsert_meta_on(&c, &meta("A", "Playlist A"), 100);
        upsert_meta_on(&c, &meta("B", "Playlist B"), 100);
        record_event_on(&c, "A", 100);
        record_event_on(&c, "B", 100);
        let _ = c.execute("DELETE FROM playlist_play_events WHERE playlist_id = 'A'", []);
        let _ = c.execute("DELETE FROM playlist_meta WHERE playlist_id = 'A'", []);
        let left = query_on(&c, "p.last_at DESC", None);
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].playlist_id, "B");
    }
}
