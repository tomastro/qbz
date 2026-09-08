//! CRUD repository for mixtape_collections and mixtape_collection_items.
//!
//! All functions take `&Connection` (or `&mut Connection` when a transaction
//! is needed). No Tauri state, no async runtime — testable with in-memory
//! SQLite. The application layer wraps these with its per-user library handle.

use rusqlite::{params, Connection, OptionalExtension, Result};
use uuid::Uuid;

use qbz_models::mixtape::{
    AlbumSource, CollectionKind, CollectionPlayMode, CollectionSourceType, ItemType,
    MixtapeCollection, MixtapeCollectionItem,
};

fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

// ──────────────────────────── Collection CRUD ────────────────────────────

pub fn create_collection(
    conn: &Connection,
    kind: CollectionKind,
    name: &str,
    description: Option<&str>,
    source_type: CollectionSourceType,
    source_ref: Option<&str>,
) -> Result<MixtapeCollection> {
    let id = Uuid::new_v4().to_string();
    let ts = now_ms();

    // New collections go to the top of their kind's navigation (position = 0;
    // shift others down). Manual drag-reorder can rearrange later.
    conn.execute(
        "UPDATE mixtape_collections SET position = position + 1 WHERE kind = ?1",
        params![serialize_kind(kind)],
    )?;

    let last_synced_at = match source_type {
        CollectionSourceType::ArtistDiscography => Some(ts),
        _ => None,
    };

    conn.execute(
        "INSERT INTO mixtape_collections (
            id, kind, name, description,
            source_type, source_ref,
            play_mode, custom_artwork_path,
            position, hidden, last_played_at, play_count,
            last_synced_at, created_at, updated_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL, 0, 0, NULL, 0, ?8, ?9, ?10)",
        params![
            id,
            serialize_kind(kind),
            name,
            description,
            serialize_source_type(source_type),
            source_ref,
            serialize_play_mode(CollectionPlayMode::InOrder),
            last_synced_at,
            ts,
            ts,
        ],
    )?;

    get_collection(conn, &id).map(|o| o.expect("just inserted"))
}

pub fn list_collections(
    conn: &Connection,
    kind: Option<CollectionKind>,
) -> Result<Vec<MixtapeCollection>> {
    let mut out = Vec::new();
    match kind {
        Some(k) => {
            let mut stmt = conn.prepare(
                "SELECT id, kind, name, description, source_type, source_ref,
                        play_mode, custom_artwork_path, position, hidden,
                        last_played_at, play_count, last_synced_at,
                        created_at, updated_at
                   FROM mixtape_collections
                   WHERE kind = ?1
                   ORDER BY position ASC",
            )?;
            let rows = stmt.query_map(params![serialize_kind(k)], row_to_collection)?;
            for r in rows {
                out.push(r?);
            }
        }
        None => {
            let mut stmt = conn.prepare(
                "SELECT id, kind, name, description, source_type, source_ref,
                        play_mode, custom_artwork_path, position, hidden,
                        last_played_at, play_count, last_synced_at,
                        created_at, updated_at
                   FROM mixtape_collections
                   ORDER BY kind, position ASC",
            )?;
            let rows = stmt.query_map([], row_to_collection)?;
            for r in rows {
                out.push(r?);
            }
        }
    }
    // Hydrate items for every listed collection so the sidebar / AddToMixtape
    // modal can show accurate item counts + mosaic artwork. Without this the
    // listing returns `items: []` and every count reads as zero.
    for c in out.iter_mut() {
        c.items = list_items(conn, &c.id)?;
    }
    Ok(out)
}

pub fn get_collection(conn: &Connection, id: &str) -> Result<Option<MixtapeCollection>> {
    let maybe = conn
        .query_row(
            "SELECT id, kind, name, description, source_type, source_ref,
                    play_mode, custom_artwork_path, position, hidden,
                    last_played_at, play_count, last_synced_at,
                    created_at, updated_at
               FROM mixtape_collections
               WHERE id = ?1",
            params![id],
            row_to_collection,
        )
        .optional()?;
    if let Some(mut c) = maybe {
        c.items = list_items(conn, id)?;
        Ok(Some(c))
    } else {
        Ok(None)
    }
}

pub fn rename_collection(conn: &Connection, id: &str, new_name: &str) -> Result<()> {
    conn.execute(
        "UPDATE mixtape_collections SET name = ?1, updated_at = ?2 WHERE id = ?3",
        params![new_name, now_ms(), id],
    )?;
    Ok(())
}

pub fn set_description(conn: &Connection, id: &str, desc: Option<&str>) -> Result<()> {
    conn.execute(
        "UPDATE mixtape_collections SET description = ?1, updated_at = ?2 WHERE id = ?3",
        params![desc, now_ms(), id],
    )?;
    Ok(())
}

pub fn set_play_mode(conn: &Connection, id: &str, mode: CollectionPlayMode) -> Result<()> {
    conn.execute(
        "UPDATE mixtape_collections SET play_mode = ?1, updated_at = ?2 WHERE id = ?3",
        params![serialize_play_mode(mode), now_ms(), id],
    )?;
    Ok(())
}

/// Convert between Mixtape and Collection. Rejects any involvement of
/// ArtistCollection — that kind is anchored by `source_ref` (an artist id)
/// and cannot be freely renamed into something else.
pub fn set_kind(conn: &Connection, id: &str, new_kind: CollectionKind) -> Result<()> {
    let existing = get_collection(conn, id)?.ok_or(rusqlite::Error::QueryReturnedNoRows)?;
    if matches!(existing.kind, CollectionKind::ArtistCollection)
        || matches!(new_kind, CollectionKind::ArtistCollection)
    {
        return Err(rusqlite::Error::InvalidParameterName(
            "cannot convert to/from artist_collection".into(),
        ));
    }
    conn.execute(
        "UPDATE mixtape_collections SET kind = ?1, updated_at = ?2 WHERE id = ?3",
        params![serialize_kind(new_kind), now_ms(), id],
    )?;
    Ok(())
}

pub fn set_custom_artwork(conn: &Connection, id: &str, path: Option<&str>) -> Result<()> {
    conn.execute(
        "UPDATE mixtape_collections SET custom_artwork_path = ?1, updated_at = ?2 WHERE id = ?3",
        params![path, now_ms(), id],
    )?;
    Ok(())
}

/// Returns the currently stored `custom_artwork_path` for a collection, if
/// any. Used before overwriting / clearing to find the previous file that
/// needs deleting from disk.
pub fn get_custom_artwork(conn: &Connection, id: &str) -> Result<Option<String>> {
    let path = conn
        .query_row(
            "SELECT custom_artwork_path FROM mixtape_collections WHERE id = ?1",
            params![id],
            |r| r.get::<_, Option<String>>(0),
        )
        .ok()
        .flatten();
    Ok(path)
}

pub fn delete_collection(conn: &Connection, id: &str) -> Result<()> {
    // CASCADE removes items.
    conn.execute("DELETE FROM mixtape_collections WHERE id = ?1", params![id])?;
    Ok(())
}

pub fn touch_play(conn: &Connection, id: &str) -> Result<()> {
    let ts = now_ms();
    conn.execute(
        "UPDATE mixtape_collections
            SET last_played_at = ?1, play_count = play_count + 1, updated_at = ?2
            WHERE id = ?3",
        params![ts, ts, id],
    )?;
    Ok(())
}

// ──────────────────────────── Item CRUD ────────────────────────────

pub fn list_items(conn: &Connection, collection_id: &str) -> Result<Vec<MixtapeCollectionItem>> {
    let mut stmt = conn.prepare(
        "SELECT collection_id, position, item_type, source, source_item_id,
                title, subtitle, artwork_url, year, track_count, added_at
           FROM mixtape_collection_items
           WHERE collection_id = ?1
           ORDER BY position ASC",
    )?;
    let mut out = Vec::new();
    for r in stmt.query_map(params![collection_id], row_to_item)? {
        out.push(r?);
    }
    Ok(out)
}

/// Insert a new item at the end of the collection. Returns `true` if inserted,
/// `false` if an exact (source, source_item_id) duplicate already exists in
/// this collection. Different variants of the same album — e.g. Qobuz vs a
/// Local copy — are NOT deduped (they may differ in mastering or quality).
pub fn add_item(
    conn: &Connection,
    collection_id: &str,
    item_type: ItemType,
    source: AlbumSource,
    source_item_id: &str,
    title: &str,
    subtitle: Option<&str>,
    artwork_url: Option<&str>,
    year: Option<i32>,
    track_count: Option<i32>,
) -> Result<bool> {
    add_item_with(
        conn, collection_id, item_type, source, source_item_id,
        title, subtitle, artwork_url, year, track_count,
        false,
    )
}

/// Same as `add_item` but when `allow_duplicate` is true the
/// `(collection_id, source, source_item_id)` dedup check is skipped and the
/// row is always inserted. Used by the confirmation-backed add flow so a
/// user that explicitly says "yes, add it again" gets their duplicate.
#[allow(clippy::too_many_arguments)]
pub fn add_item_with(
    conn: &Connection,
    collection_id: &str,
    item_type: ItemType,
    source: AlbumSource,
    source_item_id: &str,
    title: &str,
    subtitle: Option<&str>,
    artwork_url: Option<&str>,
    year: Option<i32>,
    track_count: Option<i32>,
    allow_duplicate: bool,
) -> Result<bool> {
    if !allow_duplicate {
        let exists: bool = conn
            .prepare(
                "SELECT 1 FROM mixtape_collection_items
                   WHERE collection_id = ?1 AND source = ?2 AND source_item_id = ?3
                   LIMIT 1",
            )?
            .exists(params![collection_id, serialize_source(source), source_item_id])?;
        if exists {
            return Ok(false);
        }
    }

    let next_pos: i32 = conn.query_row(
        "SELECT COALESCE(MAX(position), -1) + 1
           FROM mixtape_collection_items WHERE collection_id = ?1",
        params![collection_id],
        |r| r.get(0),
    )?;

    let ts = now_ms();
    conn.execute(
        "INSERT INTO mixtape_collection_items (
            collection_id, position, item_type, source, source_item_id,
            title, subtitle, artwork_url, year, track_count, added_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            collection_id,
            next_pos,
            serialize_item_type(item_type),
            serialize_source(source),
            source_item_id,
            title,
            subtitle,
            artwork_url,
            year,
            track_count,
            ts,
        ],
    )?;
    conn.execute(
        "UPDATE mixtape_collections SET updated_at = ?1 WHERE id = ?2",
        params![ts, collection_id],
    )?;
    Ok(true)
}

/// Returns true if this `(collection_id, source, source_item_id)` tuple
/// already has at least one row in mixtape_collection_items. Used by the
/// bulk-add confirmation flow so the UI can ask before inserting duplicates.
pub fn item_exists(
    conn: &Connection,
    collection_id: &str,
    source: AlbumSource,
    source_item_id: &str,
) -> Result<bool> {
    let exists: bool = conn
        .prepare(
            "SELECT 1 FROM mixtape_collection_items
               WHERE collection_id = ?1 AND source = ?2 AND source_item_id = ?3
               LIMIT 1",
        )?
        .exists(params![collection_id, serialize_source(source), source_item_id])?;
    Ok(exists)
}

/// Fill an artwork snapshot that was absent when an item was inserted.
/// Existing non-empty artwork is authoritative (including a user-selected
/// snapshot) and is never overwritten. Every duplicate of the same
/// source-owned item in the collection is repaired together.
pub fn backfill_item_artwork(
    conn: &Connection,
    collection_id: &str,
    source: AlbumSource,
    source_item_id: &str,
    artwork_url: &str,
) -> Result<usize> {
    if artwork_url.trim().is_empty() {
        return Ok(0);
    }
    conn.execute(
        "UPDATE mixtape_collection_items
            SET artwork_url = ?1
          WHERE collection_id = ?2
            AND source = ?3
            AND source_item_id = ?4
            AND (artwork_url IS NULL OR TRIM(artwork_url) = '')",
        params![
            artwork_url,
            collection_id,
            serialize_source(source),
            source_item_id,
        ],
    )
}

pub fn remove_item(conn: &Connection, collection_id: &str, position: i32) -> Result<()> {
    conn.execute(
        "DELETE FROM mixtape_collection_items
           WHERE collection_id = ?1 AND position = ?2",
        params![collection_id, position],
    )?;
    // Compact positions above the removed index.
    conn.execute(
        "UPDATE mixtape_collection_items
           SET position = position - 1
           WHERE collection_id = ?1 AND position > ?2",
        params![collection_id, position],
    )?;
    conn.execute(
        "UPDATE mixtape_collections SET updated_at = ?1 WHERE id = ?2",
        params![now_ms(), collection_id],
    )?;
    Ok(())
}

/// Rewrite an entire collection's item order in a single transaction.
/// `new_order_positions` is a permutation of current positions (0..N).
pub fn reorder_items(
    conn: &mut Connection,
    collection_id: &str,
    new_order_positions: &[i32],
) -> Result<()> {
    let tx = conn.transaction()?;
    let current = list_items_tx(&tx, collection_id)?;
    if current.len() != new_order_positions.len() {
        return Err(rusqlite::Error::InvalidParameterName(
            "reorder length mismatch".into(),
        ));
    }

    tx.execute(
        "DELETE FROM mixtape_collection_items WHERE collection_id = ?1",
        params![collection_id],
    )?;

    for (new_pos, old_pos) in new_order_positions.iter().enumerate() {
        let item = current
            .iter()
            .find(|i| i.position == *old_pos)
            .ok_or_else(|| {
                rusqlite::Error::InvalidParameterName(format!(
                    "unknown position {} in reorder",
                    old_pos
                ))
            })?;
        tx.execute(
            "INSERT INTO mixtape_collection_items (
                collection_id, position, item_type, source, source_item_id,
                title, subtitle, artwork_url, year, track_count, added_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                &item.collection_id,
                new_pos as i32,
                serialize_item_type(item.item_type),
                serialize_source(item.source),
                &item.source_item_id,
                &item.title,
                &item.subtitle,
                &item.artwork_url,
                &item.year,
                &item.track_count,
                &item.added_at,
            ],
        )?;
    }
    tx.execute(
        "UPDATE mixtape_collections SET updated_at = ?1 WHERE id = ?2",
        params![now_ms(), collection_id],
    )?;
    tx.commit()
}

fn list_items_tx(
    tx: &rusqlite::Transaction,
    collection_id: &str,
) -> Result<Vec<MixtapeCollectionItem>> {
    let mut stmt = tx.prepare(
        "SELECT collection_id, position, item_type, source, source_item_id,
                title, subtitle, artwork_url, year, track_count, added_at
           FROM mixtape_collection_items
           WHERE collection_id = ?1
           ORDER BY position ASC",
    )?;
    let mut out = Vec::new();
    for r in stmt.query_map(params![collection_id], row_to_item)? {
        out.push(r?);
    }
    Ok(out)
}

// ──────────────────────────── Serde helpers ────────────────────────────

fn serialize_kind(k: CollectionKind) -> &'static str {
    match k {
        CollectionKind::Mixtape => "mixtape",
        CollectionKind::Collection => "collection",
        CollectionKind::ArtistCollection => "artist_collection",
    }
}
fn parse_kind(s: &str) -> CollectionKind {
    match s {
        "mixtape" => CollectionKind::Mixtape,
        "artist_collection" => CollectionKind::ArtistCollection,
        _ => CollectionKind::Collection,
    }
}
fn serialize_source_type(t: CollectionSourceType) -> &'static str {
    match t {
        CollectionSourceType::Manual => "manual",
        CollectionSourceType::ArtistDiscography => "artist_discography",
    }
}
fn parse_source_type(s: &str) -> CollectionSourceType {
    match s {
        "artist_discography" => CollectionSourceType::ArtistDiscography,
        _ => CollectionSourceType::Manual,
    }
}
fn serialize_play_mode(m: CollectionPlayMode) -> &'static str {
    match m {
        CollectionPlayMode::InOrder => "in_order",
        CollectionPlayMode::AlbumShuffle => "album_shuffle",
    }
}
fn parse_play_mode(s: &str) -> CollectionPlayMode {
    match s {
        "album_shuffle" => CollectionPlayMode::AlbumShuffle,
        _ => CollectionPlayMode::InOrder,
    }
}
fn serialize_item_type(t: ItemType) -> &'static str {
    match t {
        ItemType::Album => "album",
        ItemType::Track => "track",
        ItemType::Playlist => "playlist",
    }
}
fn parse_item_type(s: &str) -> ItemType {
    match s {
        "track" => ItemType::Track,
        "playlist" => ItemType::Playlist,
        _ => ItemType::Album,
    }
}
fn serialize_source(s: AlbumSource) -> &'static str {
    match s {
        AlbumSource::Qobuz => "qobuz",
        AlbumSource::Local => "local",
    }
}
fn parse_source(s: &str) -> AlbumSource {
    match s {
        "local" => AlbumSource::Local,
        _ => AlbumSource::Qobuz,
    }
}

// ──────────────────────────── Row mappers ────────────────────────────

fn row_to_collection(r: &rusqlite::Row) -> Result<MixtapeCollection> {
    Ok(MixtapeCollection {
        id: r.get("id")?,
        kind: parse_kind(&r.get::<_, String>("kind")?),
        name: r.get("name")?,
        description: r.get("description")?,
        source_type: parse_source_type(&r.get::<_, String>("source_type")?),
        source_ref: r.get("source_ref")?,
        play_mode: parse_play_mode(&r.get::<_, String>("play_mode")?),
        custom_artwork_path: r.get("custom_artwork_path")?,
        position: r.get("position")?,
        hidden: r.get::<_, i64>("hidden")? != 0,
        last_played_at: r.get("last_played_at")?,
        play_count: r.get("play_count")?,
        last_synced_at: r.get("last_synced_at")?,
        created_at: r.get("created_at")?,
        updated_at: r.get("updated_at")?,
        items: Vec::new(),
    })
}

fn row_to_item(r: &rusqlite::Row) -> Result<MixtapeCollectionItem> {
    Ok(MixtapeCollectionItem {
        collection_id: r.get("collection_id")?,
        position: r.get("position")?,
        item_type: parse_item_type(&r.get::<_, String>("item_type")?),
        source: parse_source(&r.get::<_, String>("source")?),
        source_item_id: r.get("source_item_id")?,
        title: r.get("title")?,
        subtitle: r.get("subtitle")?,
        artwork_url: r.get("artwork_url")?,
        year: r.get("year")?,
        track_count: r.get("track_count")?,
        added_at: r.get("added_at")?,
    })
}

// ──────────────────────────── Tests ────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn fresh_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::schema::run_mixtape_migrations(&conn).unwrap();
        conn
    }

    #[test]
    fn create_then_get_roundtrips() {
        let conn = fresh_db();
        let c = create_collection(
            &conn,
            CollectionKind::Mixtape,
            "90s Cassettes",
            Some("cassette-style"),
            CollectionSourceType::Manual,
            None,
        )
        .unwrap();
        let loaded = get_collection(&conn, &c.id).unwrap().unwrap();
        assert_eq!(loaded.name, "90s Cassettes");
        assert!(matches!(loaded.kind, CollectionKind::Mixtape));
        assert!(matches!(loaded.play_mode, CollectionPlayMode::InOrder));
        assert!(loaded.items.is_empty());
    }

    #[test]
    fn artist_collection_stores_source_ref() {
        let conn = fresh_db();
        let c = create_collection(
            &conn,
            CollectionKind::ArtistCollection,
            "George Harrison",
            None,
            CollectionSourceType::ArtistDiscography,
            Some("qobuz-artist-123"),
        )
        .unwrap();
        assert_eq!(c.source_ref.as_deref(), Some("qobuz-artist-123"));
        assert!(matches!(c.source_type, CollectionSourceType::ArtistDiscography));
        assert!(c.last_synced_at.is_some(), "artist collection stamps last_synced_at on create");
    }

    #[test]
    fn list_sorts_by_position_within_kind() {
        let conn = fresh_db();
        let a = create_collection(&conn, CollectionKind::Mixtape, "A", None, CollectionSourceType::Manual, None).unwrap();
        let b = create_collection(&conn, CollectionKind::Mixtape, "B", None, CollectionSourceType::Manual, None).unwrap();
        let c = create_collection(&conn, CollectionKind::Mixtape, "C", None, CollectionSourceType::Manual, None).unwrap();
        // New collections go to position=0; older ones shift. So C is first.
        let list = list_collections(&conn, Some(CollectionKind::Mixtape)).unwrap();
        assert_eq!(list[0].id, c.id);
        assert_eq!(list[1].id, b.id);
        assert_eq!(list[2].id, a.id);
    }

    #[test]
    fn add_item_dedupes_on_source_plus_id_exact() {
        let conn = fresh_db();
        let c = create_collection(&conn, CollectionKind::Mixtape, "x", None, CollectionSourceType::Manual, None).unwrap();
        let ok1 = add_item(
            &conn, &c.id, ItemType::Album, AlbumSource::Qobuz,
            "album-123", "Dookie", Some("Green Day"), None, Some(1994), Some(15),
        ).unwrap();
        let ok2 = add_item(
            &conn, &c.id, ItemType::Album, AlbumSource::Qobuz,
            "album-123", "Dookie", Some("Green Day"), None, Some(1994), Some(15),
        ).unwrap();
        assert!(ok1, "first add succeeds");
        assert!(!ok2, "exact duplicate rejected");

        // Different source — allowed; same item id in a different source is a different item.
        let ok3 = add_item(
            &conn, &c.id, ItemType::Album, AlbumSource::Local,
            "album-123", "Dookie", Some("Green Day"), None, Some(1994), Some(15),
        ).unwrap();
        assert!(ok3, "different source passes dedup");

        // Different item_type but same source+id — allowed (conceptually different beast:
        // a track dropped next to an album of the same id would still be a distinct item).
        let ok4 = add_item(
            &conn, &c.id, ItemType::Track, AlbumSource::Local,
            "album-123", "Dookie - track", Some("Green Day"), None, Some(1994), Some(1),
        ).unwrap();
        assert!(!ok4, "same source+id even across item_type is still dedup");
        // (NOTE: spec says dedup is exact (source, source_item_id). If your read of
        // the spec differs, adjust this test AND the add_item SQL accordingly.)
    }

    #[test]
    fn add_track_and_playlist_item_types() {
        let conn = fresh_db();
        let c = create_collection(&conn, CollectionKind::Mixtape, "mixed", None, CollectionSourceType::Manual, None).unwrap();
        add_item(&conn, &c.id, ItemType::Album,    AlbumSource::Qobuz, "al-1",  "Alb",  None, None, None, None).unwrap();
        add_item(&conn, &c.id, ItemType::Track,    AlbumSource::Qobuz, "tk-99", "Trk",  None, None, None, Some(1)).unwrap();
        add_item(&conn, &c.id, ItemType::Playlist, AlbumSource::Qobuz, "pl-7",  "Plst", None, None, None, Some(24)).unwrap();
        let items = list_items(&conn, &c.id).unwrap();
        assert_eq!(items.len(), 3);
        assert!(matches!(items[0].item_type, ItemType::Album));
        assert!(matches!(items[1].item_type, ItemType::Track));
        assert!(matches!(items[2].item_type, ItemType::Playlist));
    }

    #[test]
    fn artwork_backfill_only_repairs_empty_snapshots() {
        let conn = fresh_db();
        let c = create_collection(
            &conn,
            CollectionKind::Collection,
            "remote",
            None,
            CollectionSourceType::Manual,
            None,
        )
        .unwrap();
        add_item(
            &conn,
            &c.id,
            ItemType::Album,
            AlbumSource::Local,
            "jellyfin:album-1",
            "Album",
            None,
            None,
            None,
            None,
        )
        .unwrap();

        assert_eq!(
            backfill_item_artwork(
                &conn,
                &c.id,
                AlbumSource::Local,
                "jellyfin:album-1",
                "jellyfin:album-1/tag",
            )
            .unwrap(),
            1
        );
        assert_eq!(
            backfill_item_artwork(
                &conn,
                &c.id,
                AlbumSource::Local,
                "jellyfin:album-1",
                "jellyfin:new-tag",
            )
            .unwrap(),
            0,
            "a resolved refresh must not replace an existing snapshot"
        );
        assert_eq!(
            list_items(&conn, &c.id).unwrap()[0].artwork_url.as_deref(),
            Some("jellyfin:album-1/tag")
        );
    }

    #[test]
    fn remove_item_compacts_positions() {
        let conn = fresh_db();
        let c = create_collection(&conn, CollectionKind::Collection, "x", None, CollectionSourceType::Manual, None).unwrap();
        for i in 0..3 {
            add_item(
                &conn, &c.id, ItemType::Album, AlbumSource::Qobuz,
                &format!("id-{}", i), &format!("t-{}", i), None, None, None, None,
            ).unwrap();
        }
        remove_item(&conn, &c.id, 1).unwrap();
        let items = list_items(&conn, &c.id).unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].position, 0);
        assert_eq!(items[1].position, 1);
        assert_eq!(items[1].source_item_id, "id-2");
    }

    #[test]
    fn reorder_items_round_trips() {
        let mut conn = fresh_db();
        let c = create_collection(&conn, CollectionKind::Mixtape, "x", None, CollectionSourceType::Manual, None).unwrap();
        for i in 0..3 {
            add_item(
                &conn, &c.id, ItemType::Album, AlbumSource::Qobuz,
                &format!("id-{}", i), &format!("t-{}", i), None, None, None, None,
            ).unwrap();
        }
        // Reverse the order: old [0,1,2] -> new [2,1,0]
        reorder_items(&mut conn, &c.id, &[2, 1, 0]).unwrap();
        let items = list_items(&conn, &c.id).unwrap();
        assert_eq!(items[0].source_item_id, "id-2");
        assert_eq!(items[1].source_item_id, "id-1");
        assert_eq!(items[2].source_item_id, "id-0");
        for (i, it) in items.iter().enumerate() {
            assert_eq!(it.position, i as i32, "positions dense after reorder");
        }
    }

    #[test]
    fn convert_kind_rejects_artist_collection() {
        let conn = fresh_db();
        let art = create_collection(
            &conn,
            CollectionKind::ArtistCollection,
            "GH",
            None,
            CollectionSourceType::ArtistDiscography,
            Some("artist-42"),
        )
        .unwrap();
        let err = set_kind(&conn, &art.id, CollectionKind::Mixtape);
        assert!(err.is_err(), "converting from artist_collection must be rejected");

        let m = create_collection(&conn, CollectionKind::Mixtape, "m", None, CollectionSourceType::Manual, None).unwrap();
        let err2 = set_kind(&conn, &m.id, CollectionKind::ArtistCollection);
        assert!(err2.is_err(), "converting to artist_collection must be rejected");
    }

    #[test]
    fn delete_collection_cascades_items() {
        let conn = fresh_db();
        conn.execute("PRAGMA foreign_keys = ON", []).unwrap(); // ensure cascade fires
        let c = create_collection(&conn, CollectionKind::Mixtape, "x", None, CollectionSourceType::Manual, None).unwrap();
        add_item(&conn, &c.id, ItemType::Album, AlbumSource::Qobuz, "a", "t", None, None, None, None).unwrap();
        delete_collection(&conn, &c.id).unwrap();
        let items = list_items(&conn, &c.id).unwrap();
        assert!(items.is_empty());
    }

    #[test]
    fn touch_play_bumps_count_and_timestamp() {
        let conn = fresh_db();
        let c = create_collection(&conn, CollectionKind::Mixtape, "x", None, CollectionSourceType::Manual, None).unwrap();
        assert_eq!(c.play_count, 0);
        touch_play(&conn, &c.id).unwrap();
        let loaded = get_collection(&conn, &c.id).unwrap().unwrap();
        assert_eq!(loaded.play_count, 1);
        assert!(loaded.last_played_at.is_some());
    }
}
