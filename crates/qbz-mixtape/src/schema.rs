//! SQLite schema migrations for mixtape_collections + mixtape_collection_items,
//! plus one additive column on the existing session_queue_state table.
//!
//! Follows qbz-library's inline-ALTER pattern: additive-only, idempotent,
//! safe to run on every app start.

use rusqlite::{Connection, Result};

pub fn run_mixtape_migrations(conn: &Connection) -> Result<()> {
    log::info!("[Mixtape/schema] running migrations");

    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS mixtape_collections (
            id                      TEXT PRIMARY KEY,
            kind                    TEXT NOT NULL,
            name                    TEXT NOT NULL,
            description             TEXT,
            source_type             TEXT NOT NULL DEFAULT 'manual',
            source_ref              TEXT,
            play_mode               TEXT NOT NULL DEFAULT 'in_order',
            custom_artwork_path     TEXT,
            position                INTEGER NOT NULL DEFAULT 0,
            hidden                  INTEGER NOT NULL DEFAULT 0,
            last_played_at          INTEGER,
            play_count              INTEGER NOT NULL DEFAULT 0,
            last_synced_at          INTEGER,
            created_at              INTEGER NOT NULL,
            updated_at              INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS mixtape_collection_items (
            collection_id           TEXT NOT NULL REFERENCES mixtape_collections(id) ON DELETE CASCADE,
            position                INTEGER NOT NULL,
            item_type               TEXT NOT NULL,
            source                  TEXT NOT NULL,
            source_item_id          TEXT NOT NULL,
            title                   TEXT NOT NULL,
            subtitle                TEXT,
            artwork_url             TEXT,
            year                    INTEGER,
            track_count             INTEGER,
            added_at                INTEGER NOT NULL,
            PRIMARY KEY (collection_id, position)
        );

        CREATE INDEX IF NOT EXISTS idx_mixtape_items_collection
            ON mixtape_collection_items(collection_id);
        CREATE INDEX IF NOT EXISTS idx_mixtapes_kind_position
            ON mixtape_collections(kind, position);
        "#,
    )?;
    log::info!("[Mixtape/schema] mixtape_collections + items ensured");

    // Additive column on session_queue_state (if that table exists in this DB).
    // Some setups may not have session_queue_state yet; in that case we skip —
    // the Mixtape enqueue context tracking gets persisted once session_queue_state
    // is created by its own migration.
    let session_table_exists: bool = conn
        .prepare("SELECT 1 FROM sqlite_master WHERE type='table' AND name='session_queue_state'")?
        .exists([])?;

    if session_table_exists {
        let has_col: bool = conn
            .prepare(
                "SELECT COUNT(*) FROM pragma_table_info('session_queue_state') \
                 WHERE name = 'source_collection_id'",
            )?
            .query_row([], |r| r.get::<_, i64>(0))?
            > 0;
        if !has_col {
            conn.execute(
                "ALTER TABLE session_queue_state ADD COLUMN source_collection_id TEXT",
                [],
            )?;
            log::info!(
                "[Mixtape/schema] added session_queue_state.source_collection_id"
            );
        }
    } else {
        log::info!(
            "[Mixtape/schema] session_queue_state not present yet; skipping ALTER"
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrations_are_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        run_mixtape_migrations(&conn).unwrap();
        run_mixtape_migrations(&conn).unwrap();

        // Both tables exist
        let collections: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='mixtape_collections'",
            [], |r| r.get(0),
        ).unwrap();
        assert_eq!(collections, 1);
        let items: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='mixtape_collection_items'",
            [], |r| r.get(0),
        ).unwrap();
        assert_eq!(items, 1);
    }

    #[test]
    fn adds_column_when_session_table_exists() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute(
            "CREATE TABLE session_queue_state (user_id INTEGER, extra TEXT)",
            [],
        ).unwrap();
        run_mixtape_migrations(&conn).unwrap();

        let has_col: i64 = conn.query_row(
            "SELECT COUNT(*) FROM pragma_table_info('session_queue_state') WHERE name = 'source_collection_id'",
            [], |r| r.get(0),
        ).unwrap();
        assert_eq!(has_col, 1);
    }

    #[test]
    fn tolerates_missing_session_table() {
        let conn = Connection::open_in_memory().unwrap();
        // Don't create session_queue_state
        run_mixtape_migrations(&conn).unwrap(); // should not panic
    }
}
