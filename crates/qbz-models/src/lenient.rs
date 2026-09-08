//! Lenient per-item parsing for JSON arrays coming from the Qobuz API.
//!
//! The bug class this kills: `from_value::<Vec<T>>(...).unwrap_or_default()`
//! is all-or-nothing — ONE delisted/odd-shaped entry among thousands blanks
//! the entire user-visible list while any separately parsed `total` badge
//! stays correct ("Tracks 6125" over "No favorite tracks yet", #556).
//! Instead, deserialize each element individually, skip (and log) the ones
//! that don't fit the model, and keep the rest.
//!
//! This is for list endpoints whose failure mode was ALREADY a silent empty
//! list. Endpoints that propagate an honest `Err` on malformed payloads keep
//! doing so — do not funnel those through here.

use serde::de::DeserializeOwned;
use serde_json::Value;

/// Deserialize each array element individually, skipping (and warning about)
/// the ones that don't match `T` instead of nuking the whole list.
///
/// `what` names the item kind for the logs (e.g. `"track"`, `"album"`).
pub fn parse_items_lenient<T: DeserializeOwned>(items: Vec<Value>, what: &str) -> Vec<T> {
    let total = items.len();
    let mut out = Vec::with_capacity(total);
    let mut dropped = 0usize;
    for item in items {
        // Grab the id for the log before the value is consumed.
        let id_hint = item
            .get("id")
            .map(|v| v.to_string())
            .unwrap_or_else(|| "?".into());
        match serde_json::from_value::<T>(item) {
            Ok(t) => out.push(t),
            Err(e) => {
                dropped += 1;
                log::warn!(
                    "[qbz] lenient parse: skipping malformed {what} item (id {id_hint}): {e}"
                );
            }
        }
    }
    if dropped > 0 {
        log::warn!(
            "[qbz] lenient parse: skipped {dropped}/{total} {what} items (model mismatch — see warnings above)"
        );
    }
    out
}

/// Pull `value[key]["items"]` as an array (the standard Qobuz list envelope)
/// and parse it leniently via [`parse_items_lenient`]. Missing/odd-shaped
/// envelope yields an empty vec, matching the old `unwrap_or_default()`
/// behavior for that case.
pub fn parse_items_array<T: DeserializeOwned>(value: &Value, key: &str, what: &str) -> Vec<T> {
    let items = value
        .get(key)
        .and_then(|b| b.get("items"))
        .and_then(|i| i.as_array())
        .cloned()
        .unwrap_or_default();
    parse_items_lenient(items, what)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Debug, Deserialize, PartialEq)]
    struct Item {
        id: u64,
        name: String,
    }

    #[test]
    fn lenient_keeps_good_items_and_drops_bad_ones() {
        let items = vec![
            serde_json::json!({ "id": 1, "name": "a" }),
            serde_json::json!({ "id": "not-a-number", "name": "poisoned" }),
            serde_json::json!({ "id": 2, "name": "b" }),
        ];
        let out: Vec<Item> = parse_items_lenient(items, "test");
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].id, 1);
        assert_eq!(out[1].id, 2);
    }

    #[test]
    fn array_helper_walks_the_envelope() {
        let value = serde_json::json!({
            "tracks": {
                "items": [
                    { "id": 7, "name": "keep" },
                    { "id": null, "name": "drop" }
                ],
                "total": 2
            }
        });
        let out: Vec<Item> = parse_items_array(&value, "tracks", "test");
        assert_eq!(out, vec![Item { id: 7, name: "keep".into() }]);
    }

    #[test]
    fn array_helper_handles_missing_envelope() {
        let out: Vec<Item> = parse_items_array(&serde_json::json!({}), "tracks", "test");
        assert!(out.is_empty());
        let out: Vec<Item> =
            parse_items_array(&serde_json::json!({ "tracks": null }), "tracks", "test");
        assert!(out.is_empty());
    }
}
