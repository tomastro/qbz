//! Persistent QConnect device identity (frontend-agnostic).
//!
//! The QConnect device UUID must be stable across runs and shared across
//! frontends (Tauri, Slint). It is persisted in a small global SQLite settings
//! database (`<data_dir>/qbz/qconnect_settings.db`, key `device_uuid`). This was
//! relocated out of the Tauri adapter so both frontends resolve the SAME uuid
//! for the SAME install — the persisted path, key, and `QBZ_QCONNECT_DEVICE_UUID`
//! env override are byte-identical to the prior Tauri-side behavior.

use uuid::Uuid;

/// Path to the QConnect settings database (global, not per-user).
///
/// `<data_dir>/qbz/qconnect_settings.db`. Public so the Tauri adapter's
/// device-name persistence (which shares this DB file) resolves the exact same
/// path.
pub fn qconnect_settings_db_path() -> Option<std::path::PathBuf> {
    let data_dir = dirs::data_dir()?.join("qbz");
    std::fs::create_dir_all(&data_dir).ok()?;
    Some(data_dir.join("qconnect_settings.db"))
}

/// Resolve the QConnect device UUID. An explicit `QBZ_QCONNECT_DEVICE_UUID` env
/// value takes precedence; otherwise the value persisted in the settings DB is
/// used (generated + persisted on first run). Fail-open: if no settings path is
/// available, a fresh v4 is returned (same behavior as before persistence
/// existed), keeping QConnect functional on exotic systems without a data dir.
pub fn resolve_qconnect_device_uuid() -> String {
    if let Some(explicit) = std::env::var("QBZ_QCONNECT_DEVICE_UUID")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    {
        return explicit;
    }

    match qconnect_settings_db_path() {
        Some(path) => device_uuid_from_db(&path),
        None => Uuid::new_v4().to_string(),
    }
}

/// Load the persisted device_uuid from `path`, generating + persisting one on
/// first run. Split out so the persistence round-trip is unit-testable against a
/// temp path.
pub fn device_uuid_from_db(path: &std::path::Path) -> String {
    if let Some(existing) = load_persisted_device_uuid(path) {
        return existing;
    }
    let generated = Uuid::new_v4().to_string();
    persist_device_uuid(path, &generated);
    generated
}

/// Load the persisted device_uuid. Returns None if not set or on any error.
fn load_persisted_device_uuid(path: &std::path::Path) -> Option<String> {
    let conn = rusqlite::Connection::open(path).ok()?;
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")
        .ok()?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS settings (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        )",
    )
    .ok()?;
    conn.query_row(
        "SELECT value FROM settings WHERE key = 'device_uuid'",
        [],
        |row| row.get::<_, String>(0),
    )
    .ok()
    .filter(|v| !v.trim().is_empty())
}

/// Persist the device_uuid to disk (INSERT OR REPLACE).
fn persist_device_uuid(path: &std::path::Path, uuid: &str) {
    let Ok(conn) = rusqlite::Connection::open(path) else {
        return;
    };
    let _ = conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;");
    let _ = conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS settings (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        )",
    );
    let _ = conn.execute(
        "INSERT OR REPLACE INTO settings (key, value) VALUES ('device_uuid', ?1)",
        rusqlite::params![uuid],
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_uuid_env_override_takes_precedence() {
        // SAFETY: single-threaded test process for this var; restore after.
        std::env::set_var("QBZ_QCONNECT_DEVICE_UUID", "env-override-uuid-123");
        let uuid = resolve_qconnect_device_uuid();
        std::env::remove_var("QBZ_QCONNECT_DEVICE_UUID");
        assert_eq!(uuid, "env-override-uuid-123");
    }

    #[test]
    fn device_uuid_persists_and_is_reused_across_calls() {
        let tmp = std::env::temp_dir().join(format!(
            "qbz_qconnect_uuid_test_{}.db",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&tmp);

        // First call generates and persists.
        let first = device_uuid_from_db(&tmp);
        assert!(!first.trim().is_empty(), "generated uuid must be non-empty");
        // Second call must return the SAME value (read back from disk, not a fresh v4).
        let second = device_uuid_from_db(&tmp);
        assert_eq!(first, second, "device_uuid must be stable across calls");

        let _ = std::fs::remove_file(&tmp);
    }
}
