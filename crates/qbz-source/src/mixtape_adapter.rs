//! Adapts the registry to `qbz_mixtape::ItemResolver`.
//!
//! `qbz-mixtape` stays a provider-agnostic leaf of the graph: this crate
//! depends on it, never the reverse. It retains `ItemResolver`, generic
//! collection expansion and Qobuz mapping; this adapter supplies local and LAN
//! dispatch through the registry.

use qbz_models::mixtape::MixtapeCollectionItem;
use qbz_models::QueueTrack;

use crate::id::RawRef;
use crate::registry::SourceRegistry;

/// A `qbz_mixtape::ItemResolver` backed by the whole registry.
///
/// It replaces `ProdItemResolver::new(&client, resolve_local)` at the five
/// construction sites in `myqbz_play_qt.rs` (:148, :159, :193, :235, :533) and
/// deletes `resolve_local` (:110-115) — the function that IS bug 1's measured
/// 23–34 opens per collection.
///
/// **No lifetime parameter:** the registry is `'static`, and a
/// `RegistryResolver<'a>` would only re-import the borrow that stops
/// `ProdItemResolver` (which borrows a stack-local client clone) from being
/// moved into a spawned task.
///
/// **Behavioural note for review:** this resolver takes NO Qobuz client. A
/// Qobuz item in a collection therefore fails PER ITEM with
/// `NotConfigured { by: QOBUZ, why: "no client" }` — which
/// `resolve_collection_tracks` logs and skips (enqueue.rs:58-66) — while the
/// local and Plex items in the same collection resolve normally. Today the
/// *whole* resolve returns an empty vec before the resolver is even built
/// (myqbz_play_qt.rs:145, :156, :190, :232). That is survey IC-5, and it is the
/// direct upstream cause of bug 3's MyQBZ half.
pub struct RegistryResolver {
    registry: &'static SourceRegistry,
}

impl RegistryResolver {
    /// Wrap the process-wide registry (`crate::registry()`).
    pub fn new(registry: &'static SourceRegistry) -> Self {
        Self { registry }
    }
}

#[async_trait::async_trait]
impl qbz_mixtape::ItemResolver for RegistryResolver {
    async fn resolve(&self, item: &MixtapeCollectionItem) -> Result<Vec<QueueTrack>, String> {
        let raw = RawRef::from_mixtape_item(item);
        let media = self.registry.claim(&raw).map_err(|e| e.to_string())?;
        self.registry
            .tracks(&media)
            .await
            .map_err(|e| e.to_string())
    }
}
