//! Local snapshot of the user's QOBUZ playlists (offline-mode port, B7/B8;
//! membership index substrate, 2.1.1 Add-to-Playlist redesign).
//!
//! Spec D11 left an HONEST LIMIT: playlist names and membership live only in
//! the Qobuz API, so offline a mixed playlist falls back to a synthesized
//! "Playlist (N local)" name and shows zero Qobuz rows. This module stores a
//! point-in-time snapshot captured from data the app fetches while online:
//!
//! - HEADERS: every authoritative user-playlist list load (sidebar / playlist
//!   manager) upserts id + name (+ owner, track_count, ownership) for ALL
//!   listed playlists and advances the AUTHORITY GENERATION. A header absent
//!   from two consecutive authoritative lists is marked `inactive` — never
//!   deleted on a single miss, tolerating Qobuz post-write lag.
//! - MEMBERSHIP: opening a playlist DETAIL online full-replaces its snapshot
//!   track ids, and the background hydrator does the same from the
//!   ids-only `playlist/get?extra=track_ids` fetch. Membership is recorded
//!   ONLY for playlists already captured by a header producer (the user's own
//!   list) — a merely-viewed public playlist never lands in the snapshot.
//! - MUTATIONS: successful add/remove/create through QBZ update the snapshot
//!   incrementally and synchronously, so the picker's containment answer
//!   moves without waiting for a re-sync.
//!
//! Freshness is deliberately NOT one timestamp (that overload is exactly what
//! made an empty membership answer unreadable): `header_updated_at` says when
//! the list last named this playlist, `membership_synced_at` says when the
//! track ids were last authoritative, `membership_count` +
//! `membership_remote_updated_at` are the evidence a later list compares
//! against to decide staleness. `snapped_at` remains for the older offline
//! consumers and is stamped wherever it always was.
//!
//! All functions take `&Connection` (the local_playlists idiom): no async
//! runtime — testable with in-memory SQLite.

use std::collections::HashMap;

use rusqlite::{params, Connection, OptionalExtension, Result};

/// One snapshot header row.
#[derive(Debug, Clone)]
pub struct SnapshotHeader {
    pub qobuz_playlist_id: u64,
    pub name: String,
    pub owner: Option<String>,
    /// The playlist's TOTAL Qobuz track count at header time (not the
    /// offline-playable subset).
    pub track_count: Option<u32>,
    /// Unix ms when this header was last written (legacy stamp; kept for the
    /// offline consumers that predate the split freshness columns).
    pub snapped_at: i64,
    /// Unix ms when an authoritative list last named this playlist.
    pub header_updated_at: Option<i64>,
    /// Unix ms when the membership rows were last authoritative. NULL means
    /// membership was never captured — which is NOT "empty playlist".
    pub membership_synced_at: Option<i64>,
    /// Number of membership rows written at the last sync.
    pub membership_count: Option<u32>,
    /// The playlist's API `updated_at` as reported by the authoritative list.
    pub remote_updated_at: Option<i64>,
    /// The API `updated_at` in force when membership was last synced.
    pub membership_remote_updated_at: Option<i64>,
    /// Last authority generation that listed this playlist.
    pub seen_generation: i64,
    /// The session user owns (can write to) this playlist. Persisted
    /// explicitly instead of being re-inferred from whichever response
    /// happened to be loaded.
    pub is_owned: bool,
    /// Absent from two consecutive authoritative lists. Kept, not deleted, so
    /// a Qobuz-side hiccup cannot erase local knowledge.
    pub inactive: bool,
}

/// Authoritative-list producer input (one listed playlist).
#[derive(Debug, Clone)]
pub struct AuthoritativeEntry {
    pub qobuz_playlist_id: u64,
    pub name: String,
    pub owner: Option<String>,
    pub track_count: Option<u32>,
    pub remote_updated_at: Option<i64>,
    pub is_owned: bool,
}

/// Why the hydrator wants to refresh a playlist's membership.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StaleReason {
    NeverSynced,
    CountMismatch,
    RemoteRevision,
}

#[derive(Debug, Clone)]
pub struct HydrationCandidate {
    pub qobuz_playlist_id: u64,
    pub reason: StaleReason,
}

/// The picker's honest tri-state for the containment section.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MembershipIndexState {
    /// Every owned active playlist has authoritative membership.
    Complete,
    /// Some owned playlists still await a membership sync.
    Updating { pending: u32 },
    /// No authoritative list was ever recorded (fresh profile, or the app has
    /// never been online) — an empty containment answer means nothing.
    Unavailable,
}

fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

/// Create the snapshot tables. Idempotent (`IF NOT EXISTS` + pragma-guarded
/// additive ALTERs, the database.rs idiom), run by `LibraryDatabase::open`
/// next to the rest of the schema.
pub fn init_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS qobuz_playlist_snapshot (
            qobuz_playlist_id INTEGER PRIMARY KEY,
            name TEXT NOT NULL,
            owner TEXT,
            track_count INTEGER,
            snapped_at INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS qobuz_playlist_snapshot_tracks (
            qobuz_playlist_id INTEGER NOT NULL,
            position INTEGER NOT NULL,
            track_id INTEGER NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_qobuz_playlist_snapshot_tracks
            ON qobuz_playlist_snapshot_tracks(qobuz_playlist_id, position);

        -- The picker's containment question runs in the opposite direction:
        -- "which playlists hold THIS track".
        CREATE INDEX IF NOT EXISTS idx_qobuz_playlist_snapshot_tracks_track
            ON qobuz_playlist_snapshot_tracks(track_id, qobuz_playlist_id);

        CREATE TABLE IF NOT EXISTS qobuz_playlist_index_meta (
            key TEXT PRIMARY KEY,
            value INTEGER NOT NULL
        );
        "#,
    )?;
    // Additive migration (2.1.1 membership index): the split freshness
    // columns. One probe guards the whole batch — they ship together.
    let has_freshness: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('qobuz_playlist_snapshot')
              WHERE name = 'membership_synced_at'",
            [],
            |r| r.get::<_, i32>(0),
        )
        .map(|count| count > 0)
        .unwrap_or(false);
    if !has_freshness {
        conn.execute_batch(
            "ALTER TABLE qobuz_playlist_snapshot ADD COLUMN header_updated_at INTEGER;
             ALTER TABLE qobuz_playlist_snapshot ADD COLUMN membership_synced_at INTEGER;
             ALTER TABLE qobuz_playlist_snapshot ADD COLUMN membership_count INTEGER;
             ALTER TABLE qobuz_playlist_snapshot ADD COLUMN remote_updated_at INTEGER;
             ALTER TABLE qobuz_playlist_snapshot ADD COLUMN membership_remote_updated_at INTEGER;
             ALTER TABLE qobuz_playlist_snapshot ADD COLUMN seen_generation INTEGER NOT NULL DEFAULT 0;
             ALTER TABLE qobuz_playlist_snapshot ADD COLUMN is_owned INTEGER NOT NULL DEFAULT 0;
             ALTER TABLE qobuz_playlist_snapshot ADD COLUMN inactive INTEGER NOT NULL DEFAULT 0;
             -- Backfill: the old single stamp was both header and membership
             -- evidence; keep it as the header stamp everywhere, and as the
             -- membership stamp only where membership rows actually exist.
             UPDATE qobuz_playlist_snapshot
                SET header_updated_at = snapped_at
              WHERE header_updated_at IS NULL;
             UPDATE qobuz_playlist_snapshot
                SET membership_synced_at = snapped_at,
                    membership_count = (
                        SELECT COUNT(*) FROM qobuz_playlist_snapshot_tracks t
                         WHERE t.qobuz_playlist_id
                             = qobuz_playlist_snapshot.qobuz_playlist_id)
              WHERE membership_synced_at IS NULL
                AND EXISTS (
                    SELECT 1 FROM qobuz_playlist_snapshot_tracks t
                     WHERE t.qobuz_playlist_id
                         = qobuz_playlist_snapshot.qobuz_playlist_id);",
        )?;
    }
    Ok(())
}

// ───────────────────────── Authority generations ─────────────────────────

const GENERATION_KEY: &str = "authority_generation";

/// Last recorded authority generation; 0 when no authoritative list was ever
/// recorded.
pub fn current_generation(conn: &Connection) -> Result<i64> {
    conn.query_row(
        "SELECT value FROM qobuz_playlist_index_meta WHERE key = ?1",
        params![GENERATION_KEY],
        |r| r.get(0),
    )
    .optional()
    .map(|v| v.unwrap_or(0))
}

/// HEADERS producer: record one successful authoritative user-playlist list.
/// Upserts every entry, advances the authority generation, and marks headers
/// unseen for TWO consecutive generations `inactive` (the grace that
/// tolerates Qobuz post-write lag). Returns the new generation.
///
/// An EMPTY list is still recorded (a user can genuinely delete every
/// playlist), but the caller must only invoke this for a fetch that
/// succeeded — a network failure must never masquerade as an empty library.
pub fn record_authoritative_list(conn: &Connection, entries: &[AuthoritativeEntry]) -> Result<i64> {
    let ts = now_ms();
    conn.execute_batch("BEGIN IMMEDIATE")?;
    let result = (|| -> Result<i64> {
        let generation = current_generation(conn)? + 1;
        {
            let mut stmt = conn.prepare(
                "INSERT INTO qobuz_playlist_snapshot
                     (qobuz_playlist_id, name, owner, track_count, snapped_at,
                      header_updated_at, remote_updated_at, seen_generation,
                      is_owned, inactive)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?5, ?6, ?7, ?8, 0)
                 ON CONFLICT(qobuz_playlist_id) DO UPDATE SET
                     name = excluded.name,
                     owner = excluded.owner,
                     track_count = excluded.track_count,
                     snapped_at = excluded.snapped_at,
                     header_updated_at = excluded.header_updated_at,
                     remote_updated_at = excluded.remote_updated_at,
                     seen_generation = excluded.seen_generation,
                     is_owned = excluded.is_owned,
                     inactive = 0",
            )?;
            for e in entries {
                stmt.execute(params![
                    e.qobuz_playlist_id as i64,
                    e.name,
                    e.owner,
                    e.track_count,
                    ts,
                    e.remote_updated_at,
                    generation,
                    e.is_owned,
                ])?;
            }
        }
        conn.execute(
            "UPDATE qobuz_playlist_snapshot
                SET inactive = 1
              WHERE inactive = 0 AND seen_generation <= ?1 - 2",
            params![generation],
        )?;
        conn.execute(
            "INSERT INTO qobuz_playlist_index_meta (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![GENERATION_KEY, generation],
        )?;
        Ok(generation)
    })();
    match result {
        Ok(generation) => {
            conn.execute_batch("COMMIT")?;
            Ok(generation)
        }
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(e)
        }
    }
}

// ───────────────────────── Membership producers ─────────────────────────

/// MEMBERSHIP producer: full-replace the snapshot track ids of ONE playlist
/// (detail load or hydrator fetch) and refresh its header. Stamps
/// `membership_synced_at` / `membership_count`, and copies the HEADER's
/// current `remote_updated_at` into the membership evidence — never a stamp
/// from the fetch itself: `playlist/get` and `getUserPlaylists` do not report
/// comparable `updated_at` values, and trusting the fetch's made the queue
/// re-fetch the same playlist forever (smoke 2026-08-30). Returns `false`
/// (writing NOTHING) when the playlist has no header row — i.e. it was never
/// captured by the headers producer, so it is not one of the user's listed
/// playlists.
pub fn replace_tracks(
    conn: &Connection,
    qobuz_playlist_id: u64,
    name: &str,
    owner: Option<&str>,
    track_ids: &[u64],
) -> Result<bool> {
    let ts = now_ms();
    conn.execute_batch("BEGIN IMMEDIATE")?;
    let result = (|| -> Result<bool> {
        let updated = conn.execute(
            "UPDATE qobuz_playlist_snapshot
                SET name = ?2, owner = ?3, track_count = ?4, snapped_at = ?5,
                    membership_synced_at = ?5, membership_count = ?4,
                    membership_remote_updated_at = remote_updated_at
              WHERE qobuz_playlist_id = ?1",
            params![
                qobuz_playlist_id as i64,
                name,
                owner,
                track_ids.len() as u32,
                ts,
            ],
        )?;
        if updated == 0 {
            return Ok(false);
        }
        conn.execute(
            "DELETE FROM qobuz_playlist_snapshot_tracks WHERE qobuz_playlist_id = ?1",
            params![qobuz_playlist_id as i64],
        )?;
        let mut stmt = conn.prepare(
            "INSERT INTO qobuz_playlist_snapshot_tracks (qobuz_playlist_id, position, track_id)
             VALUES (?1, ?2, ?3)",
        )?;
        for (pos, tid) in track_ids.iter().enumerate() {
            stmt.execute(params![qobuz_playlist_id as i64, pos as i64, *tid as i64])?;
        }
        Ok(true)
    })();
    match result {
        Ok(wrote) => {
            conn.execute_batch("COMMIT")?;
            Ok(wrote)
        }
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(e)
        }
    }
}

/// Incremental MUTATION producer: a successful `playlist/addTracks` appends
/// its ids to the snapshot so the containment answer moves immediately.
/// Applies only when membership was already captured (`membership_synced_at`
/// set) — an uncaptured playlist stays queued for the hydrator instead of
/// pretending an append equals a snapshot. Returns whether it applied.
pub fn apply_added_tracks(
    conn: &Connection,
    qobuz_playlist_id: u64,
    track_ids: &[u64],
) -> Result<bool> {
    if track_ids.is_empty() {
        return Ok(false);
    }
    let ts = now_ms();
    conn.execute_batch("BEGIN IMMEDIATE")?;
    let result = (|| -> Result<bool> {
        let captured: Option<i64> = conn
            .query_row(
                "SELECT membership_synced_at FROM qobuz_playlist_snapshot
                  WHERE qobuz_playlist_id = ?1",
                params![qobuz_playlist_id as i64],
                |r| r.get(0),
            )
            .optional()?
            .flatten();
        if captured.is_none() {
            return Ok(false);
        }
        let next: i64 = conn.query_row(
            "SELECT COALESCE(MAX(position) + 1, 0)
               FROM qobuz_playlist_snapshot_tracks
              WHERE qobuz_playlist_id = ?1",
            params![qobuz_playlist_id as i64],
            |r| r.get(0),
        )?;
        let mut stmt = conn.prepare(
            "INSERT INTO qobuz_playlist_snapshot_tracks (qobuz_playlist_id, position, track_id)
             VALUES (?1, ?2, ?3)",
        )?;
        for (offset, tid) in track_ids.iter().enumerate() {
            stmt.execute(params![
                qobuz_playlist_id as i64,
                next + offset as i64,
                *tid as i64
            ])?;
        }
        conn.execute(
            "UPDATE qobuz_playlist_snapshot
                SET membership_count = (
                        SELECT COUNT(*) FROM qobuz_playlist_snapshot_tracks t
                         WHERE t.qobuz_playlist_id = ?1),
                    track_count = (
                        SELECT COUNT(*) FROM qobuz_playlist_snapshot_tracks t
                         WHERE t.qobuz_playlist_id = ?1),
                    membership_synced_at = ?2, snapped_at = ?2
              WHERE qobuz_playlist_id = ?1",
            params![qobuz_playlist_id as i64, ts],
        )?;
        Ok(true)
    })();
    match result {
        Ok(applied) => {
            conn.execute_batch("COMMIT")?;
            Ok(applied)
        }
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(e)
        }
    }
}

/// Incremental MUTATION producer: a successful track removal drops every
/// membership row bearing the removed TRACK ids. Same capture precondition
/// as `apply_added_tracks`.
pub fn apply_removed_tracks(
    conn: &Connection,
    qobuz_playlist_id: u64,
    track_ids: &[u64],
) -> Result<bool> {
    if track_ids.is_empty() {
        return Ok(false);
    }
    let ts = now_ms();
    conn.execute_batch("BEGIN IMMEDIATE")?;
    let result = (|| -> Result<bool> {
        let captured: Option<i64> = conn
            .query_row(
                "SELECT membership_synced_at FROM qobuz_playlist_snapshot
                  WHERE qobuz_playlist_id = ?1",
                params![qobuz_playlist_id as i64],
                |r| r.get(0),
            )
            .optional()?
            .flatten();
        if captured.is_none() {
            return Ok(false);
        }
        {
            let mut stmt = conn.prepare(
                "DELETE FROM qobuz_playlist_snapshot_tracks
                  WHERE qobuz_playlist_id = ?1 AND track_id = ?2",
            )?;
            for tid in track_ids {
                stmt.execute(params![qobuz_playlist_id as i64, *tid as i64])?;
            }
        }
        conn.execute(
            "UPDATE qobuz_playlist_snapshot
                SET membership_count = (
                        SELECT COUNT(*) FROM qobuz_playlist_snapshot_tracks t
                         WHERE t.qobuz_playlist_id = ?1),
                    track_count = (
                        SELECT COUNT(*) FROM qobuz_playlist_snapshot_tracks t
                         WHERE t.qobuz_playlist_id = ?1),
                    membership_synced_at = ?2, snapped_at = ?2
              WHERE qobuz_playlist_id = ?1",
            params![qobuz_playlist_id as i64, ts],
        )?;
        Ok(true)
    })();
    match result {
        Ok(applied) => {
            conn.execute_batch("COMMIT")?;
            Ok(applied)
        }
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(e)
        }
    }
}

/// MUTATION producer: a playlist just created through QBZ. Inserts its header
/// at the CURRENT generation (it cannot have been in the last list yet — the
/// grace rule, not a fresh fetch, is what keeps it alive) with an empty,
/// captured membership, so an immediate follow-up add applies incrementally.
pub fn record_created_playlist(
    conn: &Connection,
    qobuz_playlist_id: u64,
    name: &str,
    owner: Option<&str>,
) -> Result<()> {
    let ts = now_ms();
    let generation = current_generation(conn)?;
    conn.execute(
        "INSERT INTO qobuz_playlist_snapshot
             (qobuz_playlist_id, name, owner, track_count, snapped_at,
              header_updated_at, membership_synced_at, membership_count,
              seen_generation, is_owned, inactive)
         VALUES (?1, ?2, ?3, 0, ?4, ?4, ?4, 0, ?5, 1, 0)
         ON CONFLICT(qobuz_playlist_id) DO UPDATE SET
             name = excluded.name,
             owner = excluded.owner,
             seen_generation = excluded.seen_generation,
             is_owned = 1,
             inactive = 0",
        params![qobuz_playlist_id as i64, name, owner, ts, generation],
    )?;
    Ok(())
}

/// MUTATION producer: a playlist deleted through QBZ leaves the target set
/// immediately instead of waiting out the two-generation grace.
pub fn mark_inactive(conn: &Connection, qobuz_playlist_id: u64) -> Result<()> {
    conn.execute(
        "UPDATE qobuz_playlist_snapshot SET inactive = 1 WHERE qobuz_playlist_id = ?1",
        params![qobuz_playlist_id as i64],
    )?;
    Ok(())
}

// ───────────────────────── Hydration planning ─────────────────────────

/// The membership refreshes the hydrator still owes, oldest debt first.
/// Only owned, active playlists — followed/read-only playlists are
/// informational and must not spend the rate budget.
///
/// Staleness is deliberately NOT "any evidence disagrees" (the 2026-08-30
/// smoke found that fetching forever): a playlist is stale when
///
/// - membership was never captured; or
/// - BOTH sides carry a remote revision and they differ — the authoritative
///   list said the playlist changed since the membership sync; or
/// - a revision comparison is impossible on either side, and the counts
///   disagree (the coarse fallback).
///
/// A successful `replace_tracks` copies the header's current revision into
/// the membership evidence and equalises the counts, so every sync strictly
/// converges: only a NEW authoritative list can make a playlist stale again,
/// and then only the playlists it actually reports changed.
pub fn hydration_queue(conn: &Connection) -> Result<Vec<HydrationCandidate>> {
    let mut stmt = conn.prepare(
        "SELECT qobuz_playlist_id,
                membership_synced_at IS NULL AS never_synced,
                (membership_remote_updated_at IS NOT NULL
                 AND remote_updated_at IS NOT NULL
                 AND membership_remote_updated_at != remote_updated_at)
                    AS revision_mismatch
           FROM qobuz_playlist_snapshot
          WHERE is_owned = 1 AND inactive = 0
            AND (membership_synced_at IS NULL
                 OR (membership_remote_updated_at IS NOT NULL
                     AND remote_updated_at IS NOT NULL
                     AND membership_remote_updated_at != remote_updated_at)
                 OR ((membership_remote_updated_at IS NULL
                      OR remote_updated_at IS NULL)
                     AND membership_count IS NOT track_count))
          ORDER BY never_synced DESC, revision_mismatch DESC, qobuz_playlist_id",
    )?;
    let mut out = Vec::new();
    for r in stmt.query_map([], |r| {
        Ok((
            r.get::<_, i64>(0)? as u64,
            r.get::<_, bool>(1)?,
            r.get::<_, bool>(2)?,
        ))
    })? {
        let (pid, never, revision) = r?;
        out.push(HydrationCandidate {
            qobuz_playlist_id: pid,
            reason: if never {
                StaleReason::NeverSynced
            } else if revision {
                StaleReason::RemoteRevision
            } else {
                StaleReason::CountMismatch
            },
        });
    }
    Ok(out)
}

/// The containment section's honest tri-state.
pub fn index_state(conn: &Connection) -> Result<MembershipIndexState> {
    if current_generation(conn)? == 0 {
        return Ok(MembershipIndexState::Unavailable);
    }
    let pending = hydration_queue(conn)?.len() as u32;
    Ok(if pending == 0 {
        MembershipIndexState::Complete
    } else {
        MembershipIndexState::Updating { pending }
    })
}

// ───────────────────────────── Readers ─────────────────────────────

fn row_to_header(r: &rusqlite::Row) -> Result<SnapshotHeader> {
    Ok(SnapshotHeader {
        qobuz_playlist_id: r.get::<_, i64>("qobuz_playlist_id")? as u64,
        name: r.get("name")?,
        owner: r.get("owner")?,
        track_count: r.get("track_count")?,
        snapped_at: r.get("snapped_at")?,
        header_updated_at: r.get("header_updated_at")?,
        membership_synced_at: r.get("membership_synced_at")?,
        membership_count: r.get("membership_count")?,
        remote_updated_at: r.get("remote_updated_at")?,
        membership_remote_updated_at: r.get("membership_remote_updated_at")?,
        seen_generation: r.get("seen_generation")?,
        is_owned: r.get("is_owned")?,
        inactive: r.get("inactive")?,
    })
}

const HEADER_COLUMNS: &str = "qobuz_playlist_id, name, owner, track_count, snapped_at, \
     header_updated_at, membership_synced_at, membership_count, \
     remote_updated_at, membership_remote_updated_at, seen_generation, \
     is_owned, inactive";

/// One snapshot header, or None.
pub fn get_header(conn: &Connection, qobuz_playlist_id: u64) -> Result<Option<SnapshotHeader>> {
    conn.query_row(
        &format!(
            "SELECT {HEADER_COLUMNS} FROM qobuz_playlist_snapshot
              WHERE qobuz_playlist_id = ?1"
        ),
        params![qobuz_playlist_id as i64],
        row_to_header,
    )
    .optional()
}

/// All snapshot headers, inactive ones included (offline consumers decide).
pub fn all_headers(conn: &Connection) -> Result<Vec<SnapshotHeader>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {HEADER_COLUMNS} FROM qobuz_playlist_snapshot"
    ))?;
    let mut out = Vec::new();
    for r in stmt.query_map([], row_to_header)? {
        out.push(r?);
    }
    Ok(out)
}

/// One playlist's snapshot track ids in snapshot (position) order.
pub fn track_ids(conn: &Connection, qobuz_playlist_id: u64) -> Result<Vec<u64>> {
    let mut stmt = conn.prepare(
        "SELECT track_id FROM qobuz_playlist_snapshot_tracks
          WHERE qobuz_playlist_id = ?1 ORDER BY position",
    )?;
    let mut out = Vec::new();
    for r in stmt.query_map(params![qobuz_playlist_id as i64], |r| r.get::<_, i64>(0))? {
        out.push(r? as u64);
    }
    Ok(out)
}

/// playlist id -> snapshot track ids in position order, for every playlist
/// with membership rows (availability intersection, B8).
pub fn all_track_ids(conn: &Connection) -> Result<HashMap<u64, Vec<u64>>> {
    let mut stmt = conn.prepare(
        "SELECT qobuz_playlist_id, track_id FROM qobuz_playlist_snapshot_tracks
          ORDER BY qobuz_playlist_id, position",
    )?;
    let mut out: HashMap<u64, Vec<u64>> = HashMap::new();
    for r in stmt.query_map([], |r| {
        Ok((r.get::<_, i64>(0)? as u64, r.get::<_, i64>(1)? as u64))
    })? {
        let (pid, tid) = r?;
        out.entry(pid).or_default().push(tid);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        init_schema(&c).unwrap();
        c
    }

    fn entry(id: u64, name: &str, count: u32) -> AuthoritativeEntry {
        AuthoritativeEntry {
            qobuz_playlist_id: id,
            name: name.to_string(),
            owner: Some("me".to_string()),
            track_count: Some(count),
            remote_updated_at: None,
            is_owned: true,
        }
    }

    fn record(c: &Connection, entries: &[AuthoritativeEntry]) -> i64 {
        record_authoritative_list(c, entries).unwrap()
    }

    #[test]
    fn roundtrip_header_and_tracks() {
        let c = conn();
        record(&c, &[entry(42, "Road Trip", 3)]);
        let wrote = replace_tracks(&c, 42, "Road Trip", Some("me"), &[30, 10, 20]).unwrap();
        assert!(wrote);

        let h = get_header(&c, 42).unwrap().unwrap();
        assert_eq!(h.name, "Road Trip");
        assert_eq!(h.owner.as_deref(), Some("me"));
        assert_eq!(h.track_count, Some(3));
        assert!(h.snapped_at > 0);
        assert!(h.is_owned);
        assert!(!h.inactive);
        assert_eq!(h.membership_count, Some(3));
        assert!(h.membership_synced_at.is_some());

        // Snapshot order preserved, not sorted by id.
        assert_eq!(track_ids(&c, 42).unwrap(), vec![30, 10, 20]);
        let all = all_track_ids(&c).unwrap();
        assert_eq!(all.get(&42).unwrap(), &vec![30, 10, 20]);
    }

    #[test]
    fn replace_is_full_replace() {
        let c = conn();
        record(&c, &[entry(7, "Mix", 3)]);
        replace_tracks(&c, 7, "Mix", None, &[1, 2, 3]).unwrap();
        replace_tracks(&c, 7, "Mix renamed", None, &[9]).unwrap();

        assert_eq!(track_ids(&c, 7).unwrap(), vec![9]);
        let h = get_header(&c, 7).unwrap().unwrap();
        assert_eq!(h.name, "Mix renamed");
        assert_eq!(h.track_count, Some(1));
        // No leftover rows from the first write.
        let total: i64 = c
            .query_row(
                "SELECT COUNT(*) FROM qobuz_playlist_snapshot_tracks",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(total, 1);
    }

    #[test]
    fn names_only_rows_without_tracks() {
        let c = conn();
        record(&c, &[entry(1, "A", 10), entry(2, "B", 0)]);

        assert_eq!(all_headers(&c).unwrap().len(), 2);
        assert!(track_ids(&c, 1).unwrap().is_empty());
        assert!(all_track_ids(&c).unwrap().is_empty());

        // Re-recording updates the name in place without creating track rows,
        // and never touches the membership stamp.
        record(&c, &[entry(1, "A renamed", 11), entry(2, "B", 0)]);
        let h = get_header(&c, 1).unwrap().unwrap();
        assert_eq!(h.name, "A renamed");
        assert_eq!(h.track_count, Some(11));
        assert!(h.membership_synced_at.is_none());
        assert!(track_ids(&c, 1).unwrap().is_empty());
    }

    #[test]
    fn replace_refuses_unknown_playlist() {
        let c = conn();
        // No header row -> the detail producer writes NOTHING (a merely
        // viewed public playlist must not land in the snapshot).
        let wrote = replace_tracks(&c, 99, "Someone's list", None, &[1, 2]).unwrap();
        assert!(!wrote);
        assert!(get_header(&c, 99).unwrap().is_none());
        assert!(track_ids(&c, 99).unwrap().is_empty());
    }

    #[test]
    fn header_upsert_is_not_membership_evidence() {
        // The 2.1.0 research finding: header refreshes used to renew the one
        // timestamp that also vouched for membership. Now they must not.
        let c = conn();
        record(&c, &[entry(5, "Mix", 2)]);
        replace_tracks(&c, 5, "Mix", None, &[1, 2]).unwrap();
        let synced = get_header(&c, 5).unwrap().unwrap().membership_synced_at;

        record(&c, &[entry(5, "Mix", 2)]);
        let h = get_header(&c, 5).unwrap().unwrap();
        assert_eq!(h.membership_synced_at, synced);
        assert_eq!(h.membership_count, Some(2));
    }

    #[test]
    fn generations_and_two_generation_grace() {
        let c = conn();
        assert_eq!(current_generation(&c).unwrap(), 0);
        let g1 = record(&c, &[entry(1, "A", 0), entry(2, "B", 0)]);
        assert_eq!(g1, 1);

        // B missing once: still active (post-write lag tolerance).
        record(&c, &[entry(1, "A", 0)]);
        assert!(!get_header(&c, 2).unwrap().unwrap().inactive);

        // B missing twice: inactive, but never deleted.
        record(&c, &[entry(1, "A", 0)]);
        let h = get_header(&c, 2).unwrap().unwrap();
        assert!(h.inactive);

        // B reappears: active again.
        record(&c, &[entry(1, "A", 0), entry(2, "B", 0)]);
        assert!(!get_header(&c, 2).unwrap().unwrap().inactive);
    }

    #[test]
    fn hydration_queue_reasons_and_order() {
        let c = conn();
        let mut never = entry(1, "Never", 4);
        never.remote_updated_at = Some(100);
        // Revision unavailable on this one: the coarse count fallback governs.
        let mut counted = entry(2, "Counted", 5);
        counted.remote_updated_at = None;
        let mut revised = entry(3, "Revised", 1);
        revised.remote_updated_at = Some(100);
        let mut fresh = entry(4, "Fresh", 1);
        fresh.remote_updated_at = Some(100);
        let followed = AuthoritativeEntry {
            is_owned: false,
            ..entry(5, "Followed", 9)
        };
        record(
            &c,
            &[
                never.clone(),
                counted.clone(),
                revised.clone(),
                fresh.clone(),
                followed,
            ],
        );
        replace_tracks(&c, 2, "Counted", None, &[7]).unwrap();
        replace_tracks(&c, 3, "Revised", None, &[7]).unwrap();
        replace_tracks(&c, 4, "Fresh", None, &[8]).unwrap();
        // Next authoritative list: 2's declared count (5) disagrees with its
        // membership (1) and it has no revision signal; 3 changed remotely.
        revised.remote_updated_at = Some(200);
        record(&c, &[never, counted, revised, fresh]);

        let queue = hydration_queue(&c).unwrap();
        let ids: Vec<u64> = queue.iter().map(|q| q.qobuz_playlist_id).collect();
        assert_eq!(ids, vec![1, 3, 2]);
        assert_eq!(queue[0].reason, StaleReason::NeverSynced);
        assert_eq!(queue[1].reason, StaleReason::RemoteRevision);
        assert_eq!(queue[2].reason, StaleReason::CountMismatch);

        match index_state(&c).unwrap() {
            MembershipIndexState::Updating { pending } => assert_eq!(pending, 3),
            other => panic!("expected Updating, got {other:?}"),
        }
    }

    #[test]
    fn successful_sync_settles_the_queue_even_when_the_list_count_lags() {
        // The 2026-08-30 smoke: one playlist re-fetched forever. A sync must
        // CONVERGE — with revision evidence on both sides equal, a lagging
        // list count is the list's problem, not a reason to fetch again.
        let c = conn();
        let mut e = entry(1, "Lagging", 10);
        e.remote_updated_at = Some(100);
        record(&c, &[e.clone()]);
        replace_tracks(&c, 1, "Lagging", None, &[5, 6, 7]).unwrap();
        // The list still reports the stale count 10 and the same revision.
        record(&c, &[e]);
        assert!(hydration_queue(&c).unwrap().is_empty());
        assert_eq!(index_state(&c).unwrap(), MembershipIndexState::Complete);
    }

    #[test]
    fn only_the_playlists_the_list_reports_changed_requeue() {
        let c = conn();
        let mut a = entry(1, "A", 2);
        a.remote_updated_at = Some(100);
        let mut b = entry(2, "B", 2);
        b.remote_updated_at = Some(100);
        record(&c, &[a.clone(), b.clone()]);
        replace_tracks(&c, 1, "A", None, &[1, 2]).unwrap();
        replace_tracks(&c, 2, "B", None, &[3, 4]).unwrap();
        assert!(hydration_queue(&c).unwrap().is_empty());

        // Only B changed remotely.
        b.remote_updated_at = Some(200);
        record(&c, &[a, b]);
        let queue = hydration_queue(&c).unwrap();
        assert_eq!(queue.len(), 1);
        assert_eq!(queue[0].qobuz_playlist_id, 2);
        assert_eq!(queue[0].reason, StaleReason::RemoteRevision);
    }

    #[test]
    fn index_state_lifecycle() {
        let c = conn();
        assert_eq!(index_state(&c).unwrap(), MembershipIndexState::Unavailable);
        record(&c, &[entry(1, "A", 1)]);
        assert!(matches!(
            index_state(&c).unwrap(),
            MembershipIndexState::Updating { pending: 1 }
        ));
        replace_tracks(&c, 1, "A", None, &[10]).unwrap();
        assert_eq!(index_state(&c).unwrap(), MembershipIndexState::Complete);
    }

    #[test]
    fn incremental_add_and_remove() {
        let c = conn();
        record(&c, &[entry(1, "A", 2), entry(2, "Uncaptured", 3)]);
        replace_tracks(&c, 1, "A", None, &[10, 20]).unwrap();

        assert!(apply_added_tracks(&c, 1, &[30, 40]).unwrap());
        assert_eq!(track_ids(&c, 1).unwrap(), vec![10, 20, 30, 40]);
        let h = get_header(&c, 1).unwrap().unwrap();
        assert_eq!(h.membership_count, Some(4));
        assert_eq!(h.track_count, Some(4));

        assert!(apply_removed_tracks(&c, 1, &[20, 30]).unwrap());
        assert_eq!(track_ids(&c, 1).unwrap(), vec![10, 40]);
        assert_eq!(
            get_header(&c, 1).unwrap().unwrap().membership_count,
            Some(2)
        );

        // A playlist whose membership was never captured refuses the
        // increment — the hydrator owns it (its queue entry stays).
        assert!(!apply_added_tracks(&c, 2, &[1]).unwrap());
        assert!(track_ids(&c, 2).unwrap().is_empty());
        assert!(hydration_queue(&c)
            .unwrap()
            .iter()
            .any(|q| q.qobuz_playlist_id == 2));
    }

    #[test]
    fn created_playlist_is_captured_and_survives_one_list_miss() {
        let c = conn();
        record(&c, &[entry(1, "A", 0)]);
        record_created_playlist(&c, 9, "Brand new", Some("me")).unwrap();

        let h = get_header(&c, 9).unwrap().unwrap();
        assert!(h.is_owned);
        assert_eq!(h.membership_count, Some(0));
        // Immediate add applies incrementally — no fetch needed.
        assert!(apply_added_tracks(&c, 9, &[5]).unwrap());
        assert_eq!(track_ids(&c, 9).unwrap(), vec![5]);

        // One authoritative list that does not include it yet (Qobuz
        // post-write lag): still active.
        record(&c, &[entry(1, "A", 0)]);
        assert!(!get_header(&c, 9).unwrap().unwrap().inactive);
    }

    #[test]
    fn legacy_schema_backfill() {
        // A DB created by the pre-2.1.1 module: single-stamp schema.
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(
            "CREATE TABLE qobuz_playlist_snapshot (
                 qobuz_playlist_id INTEGER PRIMARY KEY,
                 name TEXT NOT NULL, owner TEXT, track_count INTEGER,
                 snapped_at INTEGER NOT NULL);
             CREATE TABLE qobuz_playlist_snapshot_tracks (
                 qobuz_playlist_id INTEGER NOT NULL,
                 position INTEGER NOT NULL,
                 track_id INTEGER NOT NULL);
             INSERT INTO qobuz_playlist_snapshot VALUES (1, 'With tracks', NULL, 2, 111);
             INSERT INTO qobuz_playlist_snapshot VALUES (2, 'Names only', NULL, 7, 222);
             INSERT INTO qobuz_playlist_snapshot_tracks VALUES (1, 0, 10), (1, 1, 20);",
        )
        .unwrap();
        init_schema(&c).unwrap();

        let with_tracks = get_header(&c, 1).unwrap().unwrap();
        assert_eq!(with_tracks.header_updated_at, Some(111));
        assert_eq!(with_tracks.membership_synced_at, Some(111));
        assert_eq!(with_tracks.membership_count, Some(2));

        let names_only = get_header(&c, 2).unwrap().unwrap();
        assert_eq!(names_only.header_updated_at, Some(222));
        assert!(names_only.membership_synced_at.is_none());

        // Idempotent re-run.
        init_schema(&c).unwrap();
    }
}
