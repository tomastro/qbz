//! Resolves a MixtapeCollection's items into a flat Vec<QueueTrack>, then the
//! caller applies it to the queue per the enqueue mode.
//!
//! The resolver is split into a trait so tests can use a mock without real
//! API / DB calls.
//!
//! Frontend-agnostic notes (ADR-006):
//! - The Qobuz resolvers are async free fns over `qbz_qobuz::QobuzClient`.
//! - Provider-specific local and LAN resolution belongs to `qbz-source`.
//!   This crate only defines collection ordering/flattening and the generic
//!   resolver seam, so it does not pull provider databases into its graph.

use qbz_models::mixtape::{AlbumSource, CollectionPlayMode, ItemType, MixtapeCollectionItem};
use qbz_models::QueueTrack as CoreQueueTrack;

// The real shared Qobuz model types (re-exported by qbz-qobuz from qbz-models).
use qbz_models::Track as ApiTrack;

/// Trait for expanding a single Mixtape item into its tracks. Implementations:
/// - `ProdItemResolver` — uses the Qobuz client plus a caller-supplied resolver
///   for non-Qobuz items
/// - mocks in `#[cfg(test)]`
#[async_trait::async_trait]
pub trait ItemResolver: Send + Sync {
    async fn resolve(&self, item: &MixtapeCollectionItem) -> Result<Vec<CoreQueueTrack>, String>;
}

/// Apply play_mode to item ordering, then resolve each item and flatten.
/// Failed items are logged and skipped (partial playback > total failure).
/// Every track produced by a single item has its `source_item_id_hint`
/// stamped with the owning item's `source_item_id` for skip-to-item boundary
/// detection downstream.
pub async fn resolve_collection_tracks(
    items: Vec<MixtapeCollectionItem>,
    play_mode: CollectionPlayMode,
    resolver: &dyn ItemResolver,
) -> Vec<CoreQueueTrack> {
    let items = if matches!(play_mode, CollectionPlayMode::AlbumShuffle) {
        shuffle_items(items)
    } else {
        items
    };

    let mut out = Vec::new();
    for item in items {
        match resolver.resolve(&item).await {
            Ok(mut tracks) => {
                let hint = item.source_item_id.clone();
                for track in &mut tracks {
                    track.source_item_id_hint = Some(hint.clone());
                }
                out.extend(tracks);
            }
            Err(e) => {
                log::warn!(
                    "[Mixtape/enqueue] skipping item {:?}/{}: {}",
                    item.source,
                    item.source_item_id,
                    e
                );
            }
        }
    }
    out
}

/// Shuffle the ITEM order for `album_shuffle` play mode. Time-seeded ⇒ a
/// different order every play. Each item later expands to its tracks IN ORDER,
/// so albums stay contiguous and internally ordered.
pub fn shuffle_items(mut items: Vec<MixtapeCollectionItem>) -> Vec<MixtapeCollectionItem> {
    use rand::seq::SliceRandom;
    use rand::SeedableRng;
    use std::time::{SystemTime, UNIX_EPOCH};

    let seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;
    let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
    items.shuffle(&mut rng);
    items
}

// ──────────────────────────── Boundary detection ────────────────────────────

/// Given a queue and a current index, find the next index whose item boundary
/// differs from the current. Boundary := source_item_id_hint, or album_id as
/// fallback when hint is absent.
pub fn next_item_index(queue: &[CoreQueueTrack], current: usize) -> Option<usize> {
    let boundary_of = |i: usize| -> Option<&str> {
        queue.get(i).and_then(|track| {
            track
                .source_item_id_hint
                .as_deref()
                .or(track.album_id.as_deref())
        })
    };
    let current_boundary = boundary_of(current)?;
    for i in (current + 1)..queue.len() {
        if boundary_of(i) != Some(current_boundary) {
            return Some(i);
        }
    }
    None
}

/// Mirror: depending on elapsed ms and whether we're at item-start, either
/// restart the current item or jump to start of the previous item.
pub fn previous_item_index(
    queue: &[CoreQueueTrack],
    current: usize,
    current_elapsed_ms: u64,
) -> Option<usize> {
    let boundary_of = |i: usize| -> Option<String> {
        queue.get(i).and_then(|track| {
            track
                .source_item_id_hint
                .as_deref()
                .or(track.album_id.as_deref())
                .map(str::to_string)
        })
    };
    if current >= queue.len() {
        return None;
    }
    let current_boundary = boundary_of(current)?;

    let mut item_start = current;
    while item_start > 0 && boundary_of(item_start - 1) == Some(current_boundary.clone()) {
        item_start -= 1;
    }

    // If elapsed > 3s OR we are mid-item, seek to item_start.
    if current_elapsed_ms > 3_000 || current > item_start {
        return Some(item_start);
    }

    // Otherwise jump to start of previous item.
    if item_start == 0 {
        return Some(0);
    }
    let prev_boundary = boundary_of(item_start - 1)?;
    let mut prev_start = item_start - 1;
    while prev_start > 0 && boundary_of(prev_start - 1) == Some(prev_boundary.clone()) {
        prev_start -= 1;
    }
    Some(prev_start)
}

// ──────────────────────────── ProdItemResolver ────────────────────────────

/// Resolver over the shared Qobuz client plus a caller-supplied synchronous
/// closure for non-Qobuz items. The closure keeps provider state and database
/// handles outside this generic crate.
pub struct ProdItemResolver<'a, L>
where
    L: Fn(&MixtapeCollectionItem) -> Result<Vec<CoreQueueTrack>, String> + Send + Sync,
{
    pub client: &'a qbz_qobuz::QobuzClient,
    pub local: L,
}

impl<'a, L> ProdItemResolver<'a, L>
where
    L: Fn(&MixtapeCollectionItem) -> Result<Vec<CoreQueueTrack>, String> + Send + Sync,
{
    /// Build a resolver from the shared Qobuz client and a non-Qobuz resolver
    /// closure. The closure is invoked only for `AlbumSource::Local` items.
    pub fn new(client: &'a qbz_qobuz::QobuzClient, local: L) -> Self {
        Self { client, local }
    }
}

#[async_trait::async_trait]
impl<'a, L> ItemResolver for ProdItemResolver<'a, L>
where
    L: Fn(&MixtapeCollectionItem) -> Result<Vec<CoreQueueTrack>, String> + Send + Sync,
{
    async fn resolve(&self, item: &MixtapeCollectionItem) -> Result<Vec<CoreQueueTrack>, String> {
        match (item.item_type, item.source) {
            (ItemType::Album, AlbumSource::Qobuz) => {
                resolve_qobuz_album(self.client, &item.source_item_id).await
            }
            (ItemType::Track, AlbumSource::Qobuz) => {
                let track_id: u64 = item
                    .source_item_id
                    .parse()
                    .map_err(|_| format!("invalid qobuz track id: {}", item.source_item_id))?;
                resolve_qobuz_track(self.client, track_id).await
            }
            (ItemType::Playlist, AlbumSource::Qobuz) => {
                let playlist_id: u64 = item
                    .source_item_id
                    .parse()
                    .map_err(|_| format!("invalid qobuz playlist id: {}", item.source_item_id))?;
                resolve_qobuz_playlist(self.client, playlist_id).await
            }
            // Provider resolution is delegated to the caller-supplied closure.
            (_, AlbumSource::Local) => (self.local)(item),
        }
    }
}

// ── Qobuz album ──

pub async fn resolve_qobuz_album(
    client: &qbz_qobuz::QobuzClient,
    album_id: &str,
) -> Result<Vec<CoreQueueTrack>, String> {
    let album = client
        .get_album(album_id)
        .await
        .map_err(|e| format!("Qobuz get_album({}) failed: {}", album_id, e))?;

    let tracks = match album.tracks {
        Some(container) => container.items,
        None => return Err(format!("album {} returned no tracks container", album_id)),
    };

    if tracks.is_empty() {
        return Err(format!("album {} has 0 tracks", album_id));
    }

    // Build QueueTrack from each track. We have the parent Album in scope so
    // we can fill artwork / album title / album artist even when the track's
    // own `album` field is absent (shallow responses inside albums/get).
    let album_artwork = album
        .image
        .large
        .clone()
        .or_else(|| album.image.extralarge.clone())
        .or_else(|| album.image.thumbnail.clone());
    let album_title = album.title.clone();
    let album_artist_name = album.artist.name.clone();
    let album_id_str = album.id.clone();

    let result = tracks
        .into_iter()
        .map(|track| {
            // Prefer the track's own performer; fall back to album artist.
            let artist = track
                .performer
                .as_ref()
                .map(|p| p.name.clone())
                .unwrap_or_else(|| album_artist_name.clone());
            let artist_id = track.performer.as_ref().map(|p| p.id);
            // Prefer the track's nested album image when present.
            let artwork = track
                .album
                .as_ref()
                .and_then(|a| a.image.large.clone().or_else(|| a.image.thumbnail.clone()))
                .or_else(|| album_artwork.clone());

            CoreQueueTrack {
                id: track.id,
                title: track.title.clone(),
                version: track.version.clone(),
                artist,
                album: album_title.clone(),
                album_version: None,
                duration_secs: track.duration as u64,
                artwork_url: artwork,
                hires: track.hires,
                bit_depth: track.maximum_bit_depth,
                sample_rate: track.maximum_sampling_rate,
                is_local: false,
                album_id: Some(album_id_str.clone()),
                artist_id,
                // Resolved once here: `CoreQueueTrack.streamable` is a plain
                // `bool`, and absence on the catalog `Track` means "the
                // endpoint was terse", not "pulled" (`Track::is_streamable`).
                streamable: track.is_streamable(),
                source: Some("qobuz".to_string()),
                parental_warning: track.parental_warning,
                // Stamped centrally by resolve_collection_tracks; left None here.
                source_item_id_hint: None,
                context_kind: None,
                context_id: None,
                isrc: None,
                recording_mbid: None,
            }
        })
        .collect();

    Ok(result)
}

// ── Qobuz track ──

pub async fn resolve_qobuz_track(
    client: &qbz_qobuz::QobuzClient,
    track_id: u64,
) -> Result<Vec<CoreQueueTrack>, String> {
    let track = client
        .get_track(track_id)
        .await
        .map_err(|e| format!("Qobuz get_track({}) failed: {}", track_id, e))?;

    Ok(vec![track_to_queue_track_from_api(&track)])
}

// ── Qobuz playlist ──

pub async fn resolve_qobuz_playlist(
    client: &qbz_qobuz::QobuzClient,
    playlist_id: u64,
) -> Result<Vec<CoreQueueTrack>, String> {
    let playlist = client
        .get_playlist(playlist_id)
        .await
        .map_err(|e| format!("Qobuz get_playlist({}) failed: {}", playlist_id, e))?;

    let tracks = match playlist.tracks {
        Some(container) => container.items,
        None => return Err(format!("playlist {} returned no tracks", playlist_id)),
    };

    if tracks.is_empty() {
        return Err(format!("playlist {} has 0 tracks", playlist_id));
    }

    Ok(tracks.iter().map(track_to_queue_track_from_api).collect())
}

// ── Qobuz mapping helper ──

/// Map a Qobuz API `Track` to a `CoreQueueTrack`.
pub fn track_to_queue_track_from_api(track: &ApiTrack) -> CoreQueueTrack {
    let artwork_url = track
        .album
        .as_ref()
        .and_then(|a| a.image.large.clone())
        .or_else(|| track.album.as_ref().and_then(|a| a.image.thumbnail.clone()))
        .or_else(|| {
            track
                .album
                .as_ref()
                .and_then(|a| a.image.extralarge.clone())
        });
    let artist = track
        .performer
        .as_ref()
        .map(|p| p.name.clone())
        .unwrap_or_else(|| "Unknown Artist".to_string());
    let album = track
        .album
        .as_ref()
        .map(|a| a.title.clone())
        .unwrap_or_else(|| "Unknown Album".to_string());
    let album_id = track.album.as_ref().map(|a| a.id.clone());
    let artist_id = track.performer.as_ref().map(|p| p.id);

    CoreQueueTrack {
        id: track.id,
        title: track.title.clone(),
        version: track.version.clone(),
        artist,
        album,
        album_version: None,
        duration_secs: track.duration as u64,
        artwork_url,
        hires: track.hires,
        bit_depth: track.maximum_bit_depth,
        sample_rate: track.maximum_sampling_rate,
        is_local: false,
        album_id,
        artist_id,
        // Same resolution as the album path above, single-track flavour.
        streamable: track.is_streamable(),
        source: Some("qobuz".to_string()),
        parental_warning: track.parental_warning,
        source_item_id_hint: None,
        context_kind: None,
        context_id: None,
        isrc: None,
        recording_mbid: None,
    }
}

// ──────────────────────────── Tests ────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use qbz_models::mixtape::{AlbumSource, CollectionPlayMode, ItemType, MixtapeCollectionItem};

    // ── Mock resolver ──

    struct MockResolver;

    #[async_trait::async_trait]
    impl ItemResolver for MockResolver {
        async fn resolve(
            &self,
            item: &MixtapeCollectionItem,
        ) -> Result<Vec<CoreQueueTrack>, String> {
            let n = item.track_count.unwrap_or(1).max(1) as usize;
            Ok((0..n)
                .map(|i| CoreQueueTrack {
                    id: i as u64,
                    title: format!("{}-t{}", item.title, i),
                    version: None,
                    artist: item.subtitle.clone().unwrap_or_default(),
                    album: item.title.clone(),
                    album_version: None,
                    duration_secs: 180,
                    artwork_url: None,
                    hires: false,
                    bit_depth: Some(16),
                    sample_rate: Some(44.1),
                    is_local: matches!(item.source, AlbumSource::Local),
                    album_id: Some(item.source_item_id.clone()),
                    artist_id: None,
                    streamable: true,
                    source: Some(match item.source {
                        AlbumSource::Qobuz => "qobuz".into(),
                        AlbumSource::Local => "local".into(),
                    }),
                    parental_warning: false,
                    source_item_id_hint: None, // stamped by resolve_collection_tracks
                    context_kind: None,
                    context_id: None,
                    isrc: None,
                    recording_mbid: None,
                })
                .collect())
        }
    }

    fn item(
        idx: i32,
        kind: ItemType,
        src: AlbumSource,
        id: &str,
        tracks: i32,
    ) -> MixtapeCollectionItem {
        MixtapeCollectionItem {
            collection_id: "c".into(),
            position: idx,
            item_type: kind,
            source: src,
            source_item_id: id.into(),
            title: format!("title-{}", idx),
            subtitle: None,
            artwork_url: None,
            year: None,
            track_count: Some(tracks),
            added_at: 0,
        }
    }

    #[tokio::test]
    async fn resolver_stamps_hint_and_flattens_in_order() {
        let items = vec![
            item(0, ItemType::Album, AlbumSource::Qobuz, "a-1", 3),
            item(1, ItemType::Track, AlbumSource::Qobuz, "t-99", 1),
            item(2, ItemType::Album, AlbumSource::Local, "al-local-xyz", 2),
        ];
        let tracks =
            resolve_collection_tracks(items, CollectionPlayMode::InOrder, &MockResolver).await;
        assert_eq!(tracks.len(), 6);
        assert_eq!(tracks[0].source_item_id_hint.as_deref(), Some("a-1"));
        assert_eq!(tracks[2].source_item_id_hint.as_deref(), Some("a-1"));
        assert_eq!(tracks[3].source_item_id_hint.as_deref(), Some("t-99"));
        assert_eq!(
            tracks[4].source_item_id_hint.as_deref(),
            Some("al-local-xyz")
        );
    }

    #[tokio::test]
    async fn album_shuffle_changes_order_but_each_album_stays_together() {
        let items = vec![
            item(0, ItemType::Album, AlbumSource::Qobuz, "a-1", 3),
            item(1, ItemType::Album, AlbumSource::Qobuz, "a-2", 3),
            item(2, ItemType::Album, AlbumSource::Qobuz, "a-3", 3),
        ];
        let tracks =
            resolve_collection_tracks(items, CollectionPlayMode::AlbumShuffle, &MockResolver).await;
        assert_eq!(tracks.len(), 9);
        // Each album's tracks must be contiguous (no interleaving).
        let mut i = 0;
        let mut seen = std::collections::HashSet::new();
        while i < tracks.len() {
            let h = tracks[i].source_item_id_hint.clone().unwrap();
            assert!(
                !seen.contains(&h),
                "album {} must not reappear after a gap",
                h
            );
            seen.insert(h.clone());
            while i < tracks.len() && tracks[i].source_item_id_hint.as_deref() == Some(&h) {
                i += 1;
            }
        }
        assert_eq!(seen.len(), 3, "all 3 albums must be represented");
    }

    fn qt(album: &str, item: Option<&str>) -> CoreQueueTrack {
        CoreQueueTrack {
            id: 0,
            title: "t".into(),
            version: None,
            artist: "a".into(),
            album: "alb".into(),
            album_version: None,
            duration_secs: 100,
            artwork_url: None,
            hires: false,
            bit_depth: Some(16),
            sample_rate: Some(44.1),
            is_local: false,
            album_id: Some(album.into()),
            artist_id: None,
            streamable: true,
            source: Some("qobuz".into()),
            parental_warning: false,
            source_item_id_hint: item.map(String::from),
            context_kind: None,
            context_id: None,
            isrc: None,
            recording_mbid: None,
        }
    }

    #[test]
    fn next_item_jumps_past_same_hint() {
        let q = vec![
            qt("a1", Some("hint-a1")),
            qt("a1", Some("hint-a1")),
            qt("a2", Some("hint-a2")),
        ];
        assert_eq!(next_item_index(&q, 0), Some(2));
        assert_eq!(next_item_index(&q, 1), Some(2));
        assert_eq!(next_item_index(&q, 2), None);
    }

    #[test]
    fn next_item_falls_back_to_album_id() {
        let q = vec![qt("a1", None), qt("a2", None)];
        assert_eq!(next_item_index(&q, 0), Some(1));
    }

    #[test]
    fn previous_item_restarts_when_mid_item() {
        let q = vec![
            qt("a1", Some("h1")),
            qt("a1", Some("h1")),
            qt("a2", Some("h2")),
        ];
        // current=1 (mid-item of h1), elapsed=500ms → restart at item start (0)
        assert_eq!(previous_item_index(&q, 1, 500), Some(0));
        // current=0 (at item start of h1), elapsed=500ms → same item start, go to prev (0)
        assert_eq!(previous_item_index(&q, 0, 500), Some(0));
        // current=2 (start of h2), elapsed=500ms → go to previous item start (0)
        assert_eq!(previous_item_index(&q, 2, 500), Some(0));
        // current=2 (start of h2), elapsed=5000ms → restart current item (2)
        assert_eq!(previous_item_index(&q, 2, 5_000), Some(2));
    }
}
