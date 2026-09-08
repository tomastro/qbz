//! A tiny in-memory, insertion-order LRU for FRONTEND-SHAPED payloads.
//!
//! ## Why this exists, when `search_cache` already does
//!
//! `search_cache` (CAPA A) caches `SearchAllResults` — the raw API shape. It
//! has never been read by anything: `SearchService::cached` has zero non-test
//! callers in either frontend, and `git log -S` shows the read side was never
//! wired in any commit. Reviving it would still leave the expensive half on
//! the table, because a cortinilla payload is not the API response: it is the
//! response after ranking, per-category truncation, i18n section titles, the
//! top-result pick and — the part no API cache can shortcut — the local
//! library query and its derived album/artist grouping.
//!
//! So this caches the FINISHED payload instead, and it is deliberately
//! generic: the store never inspects `T`. That matters, because the two
//! frontends' payload types have diverged (the Slint carries one raw
//! `artwork_url`, the Qt port an `art_url` / `art_path` pair) and unifying
//! them is a refactor neither needs in order to share this.
//!
//! ## Deliberately NOT persisted
//!
//! Process lifetime IS the TTL. A payload embeds local-library rows and
//! resolved artwork paths, so a persisted entry would resurrect albums the
//! user has since deleted and covers that have since moved. There is no
//! timestamp and no staleness rule, and that is the point: nothing here
//! outlives the process, so nothing can go stale across one.
//!
//! The caller is responsible for clearing on the events that invalidate
//! content within a session — a user switch, and any mutation of the local
//! library.

use std::collections::HashMap;
use std::collections::VecDeque;

use crate::settings::search_cache::normalize_query;

/// Default bound. Small on purpose: the value is "the user is refining a
/// query", and a refinement session is a handful of prefixes, not a history.
pub const DEFAULT_CAPACITY: usize = 20;

/// An insertion-order LRU keyed by the normalized query.
///
/// The key goes through [`normalize_query`] — the SAME function
/// `search_cache` and `search_ranking` use — so `"  Pink   Floyd "` and
/// `"pink floyd"` are one entry, here and in the ranking buckets alike.
pub struct PayloadCache<T> {
    capacity: usize,
    entries: HashMap<String, T>,
    order: VecDeque<String>,
}

impl<T> PayloadCache<T> {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            entries: HashMap::new(),
            order: VecDeque::new(),
        }
    }

    /// The cached payload for `query`, if any. Does NOT reorder — this is an
    /// insertion-order bound, not a true recency LRU, matching `search_cache`.
    pub fn get(&self, query: &str) -> Option<&T> {
        self.entries.get(&normalize_query(query))
    }

    /// Insert or replace. Re-inserting an existing key keeps its original
    /// position, so a repeatedly refreshed query cannot pin itself forever.
    pub fn put(&mut self, query: &str, value: T) {
        let key = normalize_query(query);
        if self.entries.insert(key.clone(), value).is_none() {
            self.order.push_back(key);
        }
        while self.order.len() > self.capacity {
            if let Some(oldest) = self.order.pop_front() {
                self.entries.remove(&oldest);
            }
        }
    }

    /// Drop everything. Called on a user switch, on teardown, and on any
    /// local-library mutation.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.order.clear();
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl<T> Default for PayloadCache<T> {
    fn default() -> Self {
        Self::new(DEFAULT_CAPACITY)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A value type with NO `PartialEq`, NO `Clone` and NO `Serialize`, to
    /// prove the store never inspects `T` — which is what lets both frontends
    /// share it despite their payload types having diverged.
    struct Opaque(#[allow(dead_code)] u32);

    #[test]
    fn payload_cache_bounds_normalizes_and_evicts() {
        let mut c: PayloadCache<Opaque> = PayloadCache::new(3);
        assert!(c.is_empty());

        c.put("alpha", Opaque(1));
        c.put("beta", Opaque(2));
        assert_eq!(c.len(), 2);
        assert!(c.get("alpha").is_some());

        // The key is normalized: whitespace collapsed, trimmed, lowercased.
        c.put("  Pink   Floyd ", Opaque(3));
        assert!(
            c.get("pink floyd").is_some(),
            "the normalized key must match"
        );
        assert_eq!(c.len(), 3);

        // Past the bound the OLDEST goes, not the newest.
        c.put("gamma", Opaque(4));
        assert_eq!(c.len(), 3, "never grows past the capacity");
        assert!(c.get("alpha").is_none(), "the oldest key was evicted");
        assert!(c.get("gamma").is_some());

        // Re-inserting an existing key must not push a second order entry, or
        // a refreshed query would evict the cache one slot at a time.
        c.put("gamma", Opaque(5));
        assert_eq!(c.len(), 3);
        assert_eq!(c.order.len(), 3, "no duplicate order entry");

        c.clear();
        assert!(c.is_empty());
        assert!(c.get("gamma").is_none());
    }

    #[test]
    fn payload_cache_capacity_is_never_zero() {
        let mut c: PayloadCache<Opaque> = PayloadCache::new(0);
        c.put("only", Opaque(1));
        assert_eq!(c.len(), 1, "a zero capacity would make put() a no-op");
    }
}
