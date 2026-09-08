//! Artist page per-section release sort — the Qt half of
//! `crates/qbz/src/artist_prefs.rs` (78 lines, read in full for this port).
//!
//! SAME document, SAME disk key: `<data-dir>/qbz/artist_ui.json`, an object
//! whose `section_sort` member maps `release_type` -> sort key
//! (`newest` | `oldest` | `title-asc` | `title-desc`). That is deliberate and
//! is the whole point of the reference's design note — the choice is keyed by
//! release_type ONLY, never by artist, "so a chosen sort sticks across artists
//! and restarts". A user who alternates between the shipping Slint build and
//! this one must find the same sort, so the snake_case key and the five exact
//! wire strings are not ours to rename.
//!
//! Like `library_prefs.rs` (favorites_ui.json) and `local_state.rs`
//! (locallibrary_ui.json), this file follows the PORT's writer, not the
//! Slint's:
//!
//!  - the Slint `set_sort()` serializes its whole `Prefs` struct through
//!    `fs::write` (O_TRUNC — a reader that lands mid-write sees an empty file
//!    and answers `Prefs::default()`), and in doing so drops any key the file
//!    carries that its struct does not know about;
//!  - here a write is a read-modify-write of the parsed OBJECT, published
//!    through `settings_qt::write_json_object_atomic` (temp file + `rename(2)`),
//!    so a concurrent reader sees the whole old document or the whole new one,
//!    and any key `artist_ui.json` gains later survives our writes untouched.
//!
//! There is deliberately NO process-local cache here. The Slint keeps a
//! `thread_local!` one (`artist_prefs.rs:19-22`); this file is co-owned with a
//! second process, and it is read at most once per artist page load (one
//! `read_all()` for every bucket), so a stale cache is the worse trade.

use std::collections::HashMap;
use std::path::PathBuf;

use serde_json::{Map, Value};

/// The on-disk member name. SNAKE_CASE because the Slint's
/// `struct Prefs { section_sort: HashMap<String, String> }` (artist_prefs.rs:16)
/// reads this exact name — renaming it silently splits the two frontends'
/// stored sorts instead of erroring.
const SECTION_SORT_KEY: &str = "section_sort";

/// The value stored for "leave the order the server sent". Never written to
/// disk: see `apply_sort`.
pub(crate) const DEFAULT_SORT: &str = "default";

fn store_path() -> Option<PathBuf> {
    Some(dirs::data_dir()?.join("qbz").join("artist_ui.json"))
}

/// Every persisted bucket sort, `release_type -> sort`. Read ONCE per artist
/// page build (`artist_qt::map_artist`) rather than once per bucket.
///
/// Any failure — no data dir, unreadable file, a `section_sort` that is not an
/// object — yields an empty map, i.e. every bucket falls back to "default".
/// That matches the Slint's `unwrap_or_default()` on a bad parse.
pub(crate) fn read_all() -> HashMap<String, String> {
    let Some(path) = store_path() else {
        return HashMap::new();
    };
    let Some(doc) = crate::settings_qt::read_json_object(&path) else {
        return HashMap::new();
    };
    let Some(map) = doc.get(SECTION_SORT_KEY).and_then(|v| v.as_object()) else {
        return HashMap::new();
    };
    map.iter()
        .filter_map(|(rt, v)| v.as_str().map(|s| (rt.clone(), s.to_string())))
        .collect()
}

/// ONE bucket's persisted sort — the port of `qbz/src/artist_prefs.rs:45-52`
/// `get_sort`, which `artist_releases::reset` (`artist_releases.rs:25`) calls to
/// seat the discography page's picker on the choice the user already made on
/// the artist page. The two surfaces deliberately SHARE the store: the pref is
/// keyed by release_type only, so opening "See discography" on a bucket sorted
/// A–Z lands on an A–Z page.
///
/// The artist page itself does NOT go through here — `map_artist` reads the
/// whole map once per page build (`read_all`) rather than once per bucket. This
/// accessor exists for the single-bucket callers.
pub(crate) fn get_sort(release_type: &str) -> String {
    read_all()
        .get(release_type)
        .cloned()
        .unwrap_or_else(|| DEFAULT_SORT.to_string())
}

/// The `section_sort` edit, factored out of `set_sort` so it can be tested
/// without a data dir.
///
/// "default" REMOVES the entry instead of storing the string (the Slint's
/// `artist_prefs.rs:61-67`, "'default' removes the entry to keep the file
/// small"). Storing it would round-trip correctly today but would diverge from
/// the file the Slint writes, which is the one thing this module exists to
/// avoid.
fn apply_sort(map: &mut Map<String, Value>, release_type: &str, sort: &str) {
    if sort == DEFAULT_SORT {
        map.remove(release_type);
    } else {
        map.insert(release_type.to_string(), Value::String(sort.to_string()));
    }
}

/// Persist one bucket's sort. Read-modify-write of the WHOLE document so the
/// `section_sort` sub-object is merged, never rebuilt, and no sibling key is
/// lost (see the header).
pub(crate) fn set_sort(release_type: &str, sort: &str) {
    let Some(path) = store_path() else {
        return;
    };
    // `None` means the file exists but did not parse: the caller must not
    // write, or it would replace a document it never managed to read
    // (settings_qt::read_json_object).
    let Some(mut doc) = crate::settings_qt::read_json_object(&path) else {
        return;
    };
    let mut sorts = match doc.get(SECTION_SORT_KEY) {
        Some(Value::Object(m)) => m.clone(),
        _ => Map::new(),
    };
    apply_sort(&mut sorts, release_type, sort);
    doc.insert(SECTION_SORT_KEY.to_string(), Value::Object(sorts));
    crate::settings_qt::write_json_object_atomic(&path, &doc);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_disk_key_stays_snake_case_for_the_slint() {
        // qbz/src/artist_prefs.rs:16 reads this exact member name.
        assert_eq!(SECTION_SORT_KEY, "section_sort");
    }

    #[test]
    fn default_removes_the_entry_rather_than_storing_it() {
        let mut map = Map::new();
        apply_sort(&mut map, "album", "title-asc");
        assert_eq!(map.get("album").and_then(|v| v.as_str()), Some("title-asc"));
        apply_sort(&mut map, "album", "default");
        assert!(map.get("album").is_none(), "'default' must delete the key");
    }

    #[test]
    fn buckets_are_independent() {
        // The pref is keyed by release_type ONLY — one artist's choice for
        // "album" must not disturb "live" (and applies to every artist).
        let mut map = Map::new();
        apply_sort(&mut map, "album", "newest");
        apply_sort(&mut map, "live", "oldest");
        apply_sort(&mut map, "album", "default");
        assert!(map.get("album").is_none());
        assert_eq!(map.get("live").and_then(|v| v.as_str()), Some("oldest"));
    }
}
