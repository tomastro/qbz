//! Remembering a disc after it has been ejected.
//!
//! A disc is the one medium QBZ can play that leaves nothing behind: no path,
//! no row, no file. Everything the app learns about it — the album, the track
//! titles, WHICH pressing it is, where its cover landed — is thrown away the
//! moment the tray opens, and re-derived from the network the next time.
//!
//! That is fine for names that came from MusicBrainz. It is NOT fine for names
//! that came from the user. A single DiscID can name several pressings (the
//! owner's *Fear Inoculum* answers with four), so the moment there is a button
//! to pick the right one, or a rip wizard that lets you fix a title before it
//! is written, that choice has to outlive the eject. Otherwise correcting a
//! disc is a toy: you fix it, you take it out, you put it back, it is wrong
//! again.
//!
//! So this is a CACHE, not a second source of truth, with one rule that makes
//! it safe: **a row the user edited is never overwritten by an automatic
//! lookup.** `put_auto` refuses to touch an edited row; only `put_user` can
//! replace one.
//!
//! IDENTITY is the TOC fingerprint ([`crate::cdda::Toc::fingerprint`], and the
//! SACD equivalent): pure disc geometry, no network, no privileges, available
//! before anything has been looked up. It names the GEOMETRY, not the
//! pressing — which is exactly the point, because "for this geometry the user
//! chose pressing #2" is the fact worth remembering.
//!
//! Frontend-agnostic (ADR-006) and GLOBAL, not per-user: which record is in
//! the drive does not depend on who is logged in.

use std::path::PathBuf;
use std::sync::OnceLock;

use rusqlite::Connection;

/// One remembered track. Only the fields a human can be wrong about — the
/// geometry is read from the disc every time and is never stored, because a
/// stored start_lsn that disagrees with the disc in the drive is a way to play
/// the wrong audio.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TrackMemory {
    pub number: u32,
    pub title: String,
    pub artist: String,
}

/// What is remembered about one disc.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DiscMemory {
    pub album: String,
    pub album_artist: String,
    pub year: Option<u32>,
    pub tracks: Vec<TrackMemory>,
    /// The pressing this naming came from. Kept so the cover re-resolves to
    /// the SAME image rather than to whatever the group answers today, and so
    /// a user's choice among several pressings survives.
    pub release_id: Option<String>,
    pub release_group_id: Option<String>,
    /// Where the cover already sits on disk. The artwork cache owns the file;
    /// this only remembers which one belongs to this disc, so a re-insert
    /// paints immediately instead of waiting on the Cover Art Archive again.
    pub cover_path: Option<String>,
    /// A human corrected this. Automatic lookups must leave it alone.
    pub edited: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("no data directory")]
    NoDataDir,
    #[error("disc store: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("disc store: {0}")]
    Io(#[from] std::io::Error),
    #[error("disc store: {0}")]
    Json(#[from] serde_json::Error),
}

static DB_OVERRIDE: OnceLock<PathBuf> = OnceLock::new();

/// Point the store somewhere else. For tests, and for a future portable mode.
/// Only the FIRST call takes effect — a store that moved mid-run would leave
/// half the session's writes in the old file.
pub fn set_db_path(path: PathBuf) {
    let _ = DB_OVERRIDE.set(path);
}

fn db_path() -> Result<PathBuf, StoreError> {
    if let Some(p) = DB_OVERRIDE.get() {
        return Ok(p.clone());
    }
    let dir = dirs::data_local_dir().ok_or(StoreError::NoDataDir)?.join("qbz");
    Ok(dir.join("discs.db"))
}

fn open() -> Result<Connection, StoreError> {
    let path = db_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let conn = Connection::open(&path)?;
    migrate(&conn)?;
    Ok(conn)
}

fn migrate(conn: &Connection) -> Result<(), StoreError> {
    // WAL for the same reason ADR-002 gives the library: the app opens several
    // connections and a reader must never block on the writer.
    let _ = conn.pragma_update(None, "journal_mode", "WAL");
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS discs (
             fingerprint       TEXT PRIMARY KEY,
             disc_id           TEXT,
             album             TEXT NOT NULL DEFAULT '',
             album_artist      TEXT NOT NULL DEFAULT '',
             year              INTEGER,
             release_id        TEXT,
             release_group_id  TEXT,
             cover_path        TEXT,
             edited            INTEGER NOT NULL DEFAULT 0,
             tracks_json       TEXT NOT NULL DEFAULT '[]',
             last_seen         INTEGER NOT NULL DEFAULT 0
         );",
    )?;
    Ok(())
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// What this disc is called, if we have met it before.
///
/// Every failure — no data directory, an unreadable file, a row that will not
/// parse — answers `None`. A disc cache that can refuse to open a session is
/// worse than no disc cache: the names are a convenience and the audio is not.
pub fn get(fingerprint: &str) -> Option<DiscMemory> {
    match read(fingerprint) {
        Ok(m) => m,
        Err(e) => {
            log::warn!("[disc-store] read failed: {e}");
            None
        }
    }
}

fn read(fingerprint: &str) -> Result<Option<DiscMemory>, StoreError> {
    let conn = open()?;
    let mut stmt = conn.prepare(
        "SELECT album, album_artist, year, release_id, release_group_id,
                cover_path, edited, tracks_json
           FROM discs WHERE fingerprint = ?1",
    )?;
    let mut rows = stmt.query([fingerprint])?;
    let Some(row) = rows.next()? else {
        return Ok(None);
    };
    let tracks_json: String = row.get(7)?;
    let tracks: Vec<TrackMemory> = serde_json::from_str(&tracks_json).unwrap_or_default();
    Ok(Some(DiscMemory {
        album: row.get(0)?,
        album_artist: row.get(1)?,
        year: row.get::<_, Option<i64>>(2)?.and_then(|y| u32::try_from(y).ok()),
        release_id: row.get(3)?,
        release_group_id: row.get(4)?,
        cover_path: row.get(5)?,
        edited: row.get::<_, i64>(6)? != 0,
        tracks,
    }))
}

/// Remember what a LOOKUP found. Refuses to touch a row a human has corrected
/// — that is the whole contract, and it is enforced here rather than at the
/// call sites so a future third lookup path cannot forget it.
///
/// Returns whether anything was written.
pub fn put_auto(fingerprint: &str, disc_id: Option<&str>, memory: &DiscMemory) -> bool {
    match write(fingerprint, disc_id, memory, false) {
        Ok(written) => written,
        Err(e) => {
            log::warn!("[disc-store] auto write failed: {e}");
            false
        }
    }
}

/// Remember what the USER decided. Always wins, and stamps the row as edited
/// so no later lookup can undo it.
pub fn put_user(fingerprint: &str, disc_id: Option<&str>, memory: &DiscMemory) -> bool {
    match write(fingerprint, disc_id, memory, true) {
        Ok(written) => written,
        Err(e) => {
            log::warn!("[disc-store] user write failed: {e}");
            false
        }
    }
}

fn write(
    fingerprint: &str,
    disc_id: Option<&str>,
    memory: &DiscMemory,
    by_user: bool,
) -> Result<bool, StoreError> {
    if fingerprint.is_empty() {
        return Ok(false);
    }
    let conn = open()?;
    if !by_user {
        let edited: Option<i64> = conn
            .query_row(
                "SELECT edited FROM discs WHERE fingerprint = ?1",
                [fingerprint],
                |r| r.get(0),
            )
            .ok();
        if edited == Some(1) {
            return Ok(false);
        }
    }
    let tracks_json = serde_json::to_string(&memory.tracks)?;
    conn.execute(
        "INSERT INTO discs
             (fingerprint, disc_id, album, album_artist, year, release_id,
              release_group_id, cover_path, edited, tracks_json, last_seen)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
         ON CONFLICT(fingerprint) DO UPDATE SET
             disc_id          = excluded.disc_id,
             album            = excluded.album,
             album_artist     = excluded.album_artist,
             year             = excluded.year,
             release_id       = excluded.release_id,
             release_group_id = excluded.release_group_id,
             -- A write that carries no cover must not ERASE the one already
             -- remembered: the naming and the artwork arrive on different
             -- clocks, and the metadata answer is usually first.
             cover_path       = COALESCE(excluded.cover_path, discs.cover_path),
             edited           = excluded.edited,
             tracks_json      = excluded.tracks_json,
             last_seen        = excluded.last_seen",
        rusqlite::params![
            fingerprint,
            disc_id,
            memory.album,
            memory.album_artist,
            memory.year.map(|y| y as i64),
            memory.release_id,
            memory.release_group_id,
            memory.cover_path,
            i64::from(by_user || memory.edited),
            tracks_json,
            now_secs(),
        ],
    )?;
    Ok(true)
}

/// Attach a resolved cover to a remembered disc, creating the row if the
/// naming has not landed yet. Separate from [`put_auto`] because the cover
/// resolves on its own clock — seconds after the titles, from a different
/// service — and making the caller re-send the whole naming to record one path
/// is how the naming gets clobbered by a stale copy.
pub fn set_cover(fingerprint: &str, cover_path: &str) {
    if fingerprint.is_empty() || cover_path.is_empty() {
        return;
    }
    if let Err(e) = write_cover(fingerprint, cover_path) {
        log::warn!("[disc-store] cover write failed: {e}");
    }
}

fn write_cover(fingerprint: &str, cover_path: &str) -> Result<(), StoreError> {
    let conn = open()?;
    conn.execute(
        "INSERT INTO discs (fingerprint, cover_path, last_seen)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(fingerprint) DO UPDATE SET
             cover_path = excluded.cover_path,
             last_seen  = excluded.last_seen",
        rusqlite::params![fingerprint, cover_path, now_secs()],
    )?;
    Ok(())
}

/// Forget one disc. The escape hatch for a row whose correction turned out
/// wrong — without it, a bad `put_user` is permanent.
pub fn forget(fingerprint: &str) {
    let Ok(conn) = open() else { return };
    let _ = conn.execute("DELETE FROM discs WHERE fingerprint = ?1", [fingerprint]);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One process, one `OnceLock`, so every test in this module shares the
    /// same temp database. They use DIFFERENT fingerprints instead of trying
    /// to isolate the file.
    fn init() {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| {
            let dir = std::env::temp_dir().join(format!("qbz-disc-store-test-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            set_db_path(dir.join("discs.db"));
        });
    }

    fn memory(album: &str) -> DiscMemory {
        DiscMemory {
            album: album.to_string(),
            album_artist: "Tool".to_string(),
            year: Some(2019),
            tracks: vec![TrackMemory {
                number: 1,
                title: "Fear Inoculum".to_string(),
                artist: "Tool".to_string(),
            }],
            release_id: Some("997490e6".to_string()),
            release_group_id: Some("76203bd0".to_string()),
            cover_path: None,
            edited: false,
        }
    }

    #[test]
    fn a_disc_that_was_never_seen_is_none() {
        init();
        assert!(get("never-seen-fingerprint").is_none());
    }

    #[test]
    fn what_a_lookup_found_comes_back() {
        init();
        assert!(put_auto("fp-roundtrip", Some("disc-1"), &memory("Fear Inoculum")));
        let got = get("fp-roundtrip").expect("remembered");
        assert_eq!(got.album, "Fear Inoculum");
        assert_eq!(got.year, Some(2019));
        assert_eq!(got.tracks.len(), 1);
        assert_eq!(got.tracks[0].title, "Fear Inoculum");
        assert!(!got.edited);
    }

    /// THE rule this store exists for.
    #[test]
    fn an_automatic_lookup_never_overwrites_a_human() {
        init();
        let mut mine = memory("The Right Pressing");
        mine.release_id = Some("5c027aef".to_string());
        assert!(put_user("fp-edited", Some("disc-2"), &mine));

        // MusicBrainz answers again tomorrow with its own first pick.
        let written = put_auto("fp-edited", Some("disc-2"), &memory("The Wrong Pressing"));
        assert!(!written, "an auto lookup must refuse to touch an edited row");

        let got = get("fp-edited").expect("remembered");
        assert_eq!(got.album, "The Right Pressing");
        assert_eq!(got.release_id.as_deref(), Some("5c027aef"));
        assert!(got.edited);
    }

    #[test]
    fn a_human_can_correct_an_automatic_row() {
        init();
        assert!(put_auto("fp-upgrade", None, &memory("Auto Name")));
        assert!(put_user("fp-upgrade", None, &memory("Human Name")));
        let got = get("fp-upgrade").expect("remembered");
        assert_eq!(got.album, "Human Name");
        assert!(got.edited);
    }

    /// The naming and the cover arrive on different clocks; neither may erase
    /// the other.
    #[test]
    fn the_cover_and_the_naming_do_not_clobber_each_other() {
        init();
        set_cover("fp-cover", "/cache/art/cd-x.jpg");
        assert_eq!(
            get("fp-cover").unwrap().cover_path.as_deref(),
            Some("/cache/art/cd-x.jpg")
        );

        // A naming write with no cover in hand must leave the cover alone.
        assert!(put_auto("fp-cover", None, &memory("Late Naming")));
        let got = get("fp-cover").expect("remembered");
        assert_eq!(got.album, "Late Naming");
        assert_eq!(got.cover_path.as_deref(), Some("/cache/art/cd-x.jpg"));

        // And a cover landing after the naming must leave the naming alone.
        set_cover("fp-cover", "/cache/art/cd-y.jpg");
        let got = get("fp-cover").expect("remembered");
        assert_eq!(got.album, "Late Naming");
        assert_eq!(got.cover_path.as_deref(), Some("/cache/art/cd-y.jpg"));
    }

    #[test]
    fn forgetting_a_bad_correction_is_possible() {
        init();
        assert!(put_user("fp-forget", None, &memory("Wrong")));
        forget("fp-forget");
        assert!(get("fp-forget").is_none());
        // And the row is genuinely gone, not just flagged: an auto lookup can
        // write again.
        assert!(put_auto("fp-forget", None, &memory("Auto")));
        assert_eq!(get("fp-forget").unwrap().album, "Auto");
    }

    #[test]
    fn an_empty_fingerprint_is_refused_rather_than_stored_under_one_key() {
        init();
        assert!(!put_auto("", None, &memory("Nameless")));
        assert!(get("").is_none());
    }
}
