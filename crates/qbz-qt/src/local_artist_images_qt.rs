//! Visible-window artist portrait enrichment for Local Library.
//!
//! The Artists rail can contain hundreds or thousands of tag spellings.  A
//! request-per-row loader is both slow and hostile to provider rate limits, so
//! this module is deliberately tied to the existing artwork window:
//!
//! - mapping a row only registers its `(art key, display name)` and performs
//!   one profile-wide cache seed;
//! - only keys reported as visible are queued;
//! - one sequential worker deduplicates normalized names and resolves at most
//!   50 artists per process;
//! - positive and negative results are persisted.  Negative results have a
//!   seven-day TTL, preventing every revisit from repeating misses;
//! - Qobuz, Last.fm and Discogs are image providers. MusicBrainz is used only
//!   for conservative canonical-name resolution; it does not host portraits.
//!
//! A track/album cover already indexed for the artist remains the immediate
//! fallback and does not spend a network request. Custom/cached portraits win
//! over that fallback on every later mapping pass.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{LazyLock, Mutex, OnceLock};
use std::time::Duration;

use qbz_integrations::musicbrainz::MatchConfidence;
use qbz_library::LibraryDatabase;
use qbz_source::SourceId;
use tokio::sync::mpsc;

const MAX_REMOTE_ARTISTS_PER_PROCESS: usize = 50;
const NEGATIVE_TTL_SECS: i64 = 7 * 24 * 60 * 60;
const BETWEEN_ARTISTS: Duration = Duration::from_millis(1_200);
const LASTFM_PLACEHOLDER_HASH: &str = "2a96cbd8b46e442fc41c2b86b821562f";

#[derive(Clone)]
struct Candidate {
    key: String,
    name: String,
    normalized: String,
    profile: PathBuf,
    needs_remote: bool,
}

#[derive(Default)]
struct Registry {
    profile: Option<PathBuf>,
    images: HashMap<String, String>,
    candidates: HashMap<String, Candidate>,
}

#[derive(Clone)]
struct Resolution {
    url: String,
    source: &'static str,
    canonical_name: Option<String>,
}

#[derive(Default)]
struct StoredResolution {
    preferred_path: Option<String>,
    fresh: bool,
}

static REGISTRY: LazyLock<Mutex<Registry>> = LazyLock::new(|| Mutex::new(Registry::default()));
static ATTEMPTED: LazyLock<Mutex<HashSet<(PathBuf, String)>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));
static QUEUE: OnceLock<mpsc::UnboundedSender<Candidate>> = OnceLock::new();
static REMOTE_ATTEMPTS: AtomicUsize = AtomicUsize::new(0);

fn normalize(name: &str) -> String {
    crate::local_artist_match::normalize_artist(name)
}

fn load_cached_images(profile: &Path) -> HashMap<String, String> {
    if !profile.is_file() {
        return HashMap::new();
    }
    let Ok(db) = LibraryDatabase::open(profile) else {
        return HashMap::new();
    };
    db.get_all_artist_image_urls()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(name, path)| {
            let key = normalize(&name);
            (!key.is_empty() && !path.trim().is_empty()).then_some((key, path))
        })
        .collect()
}

/// Register one artist row and return a previously cached/custom portrait.
/// `has_fallback` means the catalog already carries an album/server image, so
/// the row is not a remote-fetch candidate even though a cached portrait may
/// still override that fallback.
pub(crate) fn register(key: &str, name: &str, has_fallback: bool) -> Option<String> {
    let profile = crate::local_state::db_path()?;
    let normalized = normalize(name);
    if key.is_empty() || normalized.is_empty() {
        return None;
    }

    let mut registry = REGISTRY.lock().unwrap_or_else(|error| error.into_inner());
    if registry.profile.as_ref() != Some(&profile) {
        registry.images = load_cached_images(&profile);
        registry.candidates.clear();
        registry.profile = Some(profile.clone());
    }
    let cached = registry.images.get(&normalized).cloned();
    registry.candidates.insert(
        key.to_string(),
        Candidate {
            key: key.to_string(),
            name: name.to_string(),
            normalized,
            profile,
            needs_remote: cached.is_none() && !has_fallback,
        },
    );
    cached
}

/// Queue only the missing artist keys in the mounted artwork window.
pub(crate) fn request_visible(keys: &[String]) {
    if crate::offline_fwd::exclude_network_folders_now() {
        return;
    }
    ensure_worker();
    let Some(queue) = QUEUE.get() else {
        return;
    };
    let candidates = {
        let registry = REGISTRY.lock().unwrap_or_else(|error| error.into_inner());
        keys.iter()
            .filter_map(|key| registry.candidates.get(key))
            .filter(|candidate| candidate.needs_remote)
            .filter(|candidate| candidate.normalized != "various artists")
            .cloned()
            .collect::<Vec<_>>()
    };
    for candidate in candidates {
        let identity = (candidate.profile.clone(), candidate.normalized.clone());
        let inserted = ATTEMPTED
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(identity);
        if inserted {
            let _ = queue.send(candidate);
        }
    }
}

fn ensure_worker() {
    if QUEUE.get().is_some() {
        return;
    }
    let (tx, rx) = mpsc::unbounded_channel();
    if QUEUE.set(tx).is_ok() {
        crate::spawn(worker(rx));
    }
}

async fn worker(mut rx: mpsc::UnboundedReceiver<Candidate>) {
    let mut first_remote = true;
    while let Some(candidate) = rx.recv().await {
        // A user/profile switch makes the queued row stale.
        if crate::local_state::db_path().as_ref() != Some(&candidate.profile) {
            continue;
        }
        if crate::offline_fwd::exclude_network_folders_now() {
            ATTEMPTED
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .remove(&(candidate.profile.clone(), candidate.normalized.clone()));
            continue;
        }

        let stored_candidate = candidate.clone();
        let stored = tokio::task::spawn_blocking(move || read_stored(&stored_candidate))
            .await
            .unwrap_or_default();
        if let Some(path) = stored.preferred_path {
            publish(&candidate, path).await;
            continue;
        }
        if stored.fresh {
            continue;
        }
        if REMOTE_ATTEMPTS.fetch_add(1, Ordering::AcqRel) >= MAX_REMOTE_ARTISTS_PER_PROCESS {
            continue;
        }
        if !first_remote {
            tokio::time::sleep(BETWEEN_ARTISTS).await;
        }
        first_remote = false;

        let resolution = resolve_remote(&candidate.name).await;
        let persisted_candidate = candidate.clone();
        let persisted_resolution = resolution.clone();
        let preferred = tokio::task::spawn_blocking(move || {
            persist(&persisted_candidate, persisted_resolution.as_ref())
        })
        .await
        .ok()
        .flatten();
        if let Some(path) = preferred.or_else(|| resolution.map(|value| value.url)) {
            publish(&candidate, path).await;
        }
    }
}

fn open_profile(profile: &Path) -> Option<LibraryDatabase> {
    if let Some(parent) = profile.parent() {
        std::fs::create_dir_all(parent).ok()?;
    }
    LibraryDatabase::open(profile).ok()
}

fn read_stored(candidate: &Candidate) -> StoredResolution {
    let Some(db) = open_profile(&candidate.profile) else {
        return StoredResolution::default();
    };
    let info = db.get_artist_image(&candidate.name).ok().flatten();
    let preferred_path = info.as_ref().and_then(|value| {
        value
            .custom_image_path
            .clone()
            .or_else(|| value.image_url.clone())
            .filter(|path| !path.trim().is_empty())
    });
    let fresh = preferred_path.is_some()
        || db
            .artist_image_resolution_is_fresh(&candidate.name, NEGATIVE_TTL_SECS)
            .unwrap_or(false);
    StoredResolution {
        preferred_path,
        fresh,
    }
}

fn persist(candidate: &Candidate, resolution: Option<&Resolution>) -> Option<String> {
    let db = open_profile(&candidate.profile)?;
    match resolution {
        Some(value) => db
            .cache_artist_image_with_canonical(
                &candidate.name,
                Some(&value.url),
                value.source,
                None,
                value.canonical_name.as_deref(),
            )
            .ok()?,
        None => db
            .cache_artist_image_with_canonical(&candidate.name, None, "miss", None, None)
            .ok()?,
    }
    // Re-read because a custom image may have been selected while the remote
    // lookup was in flight; the upsert preserves it and it must still win.
    let info = db.get_artist_image(&candidate.name).ok().flatten()?;
    info.custom_image_path
        .or(info.image_url)
        .filter(|path| !path.trim().is_empty())
}

async fn publish(candidate: &Candidate, path: String) {
    if crate::local_state::db_path().as_ref() != Some(&candidate.profile) {
        return;
    }
    let keys = {
        let mut registry = REGISTRY.lock().unwrap_or_else(|error| error.into_inner());
        if registry.profile.as_ref() != Some(&candidate.profile) {
            return;
        }
        registry
            .images
            .insert(candidate.normalized.clone(), path.clone());
        let keys = registry
            .candidates
            .values_mut()
            .filter(|entry| entry.normalized == candidate.normalized)
            .map(|entry| {
                entry.needs_remote = false;
                entry.key.clone()
            })
            .collect::<Vec<_>>();
        keys
    };
    if keys.is_empty() {
        return;
    }
    crate::local_state::with_art(|art| {
        for key in &keys {
            art.insert(key.clone(), (SourceId::LOCAL, path.clone()));
        }
    });

    let resolve_keys = keys.clone();
    let Some(window) = tokio::task::spawn_blocking(move || {
        crate::local_artwork::resolve_window_blocking(resolve_keys)
    })
    .await
    .ok() else {
        return;
    };
    crate::local_bridge_ops::emit_artwork(window.hits);
    let cold =
        crate::local_artwork::stream_cold(window.cold, crate::local_bridge_ops::emit_artwork_one);
    let remote = async {
        let fetched = crate::local_artwork::fetch_plex_misses(window.plex_misses).await;
        crate::local_bridge_ops::emit_artwork(fetched);
    };
    tokio::join!(cold, remote);
}

async fn resolve_remote(name: &str) -> Option<Resolution> {
    let original_key = normalize(name);
    let app = crate::app();

    if let Ok(page) = app.core().search_artists(name, 5, 0, None).await {
        if let Some(value) = page
            .items
            .into_iter()
            .find(|artist| normalize(&artist.name) == original_key)
        {
            if let Some(url) = value.image.as_ref().and_then(|image| image.for_px(150)) {
                if useful_url(url) {
                    return Some(Resolution {
                        url: url.clone(),
                        source: "qobuz",
                        canonical_name: Some(value.name),
                    });
                }
            }
        }
    }

    let mut canonical_name = name.to_string();
    let mut canonical_key = original_key.clone();
    let mut mbid = None;
    if let Ok(Ok(lastfm)) = tokio::time::timeout(
        Duration::from_secs(10),
        qbz_integrations::LastFmClient::new().get_artist_info(name),
    )
    .await
    {
        let lastfm_key = normalize(&lastfm.name);
        if lastfm_key == original_key {
            canonical_name = lastfm.name.clone();
            canonical_key = lastfm_key;
            mbid = lastfm.mbid.clone();
            if let Some(url) = lastfm.image.filter(|url| useful_url(url)) {
                return Some(Resolution {
                    url,
                    source: "lastfm",
                    canonical_name: Some(lastfm.name),
                });
            }
        }
    }

    if mbid.is_none() && app.core().musicbrainz_is_enabled().await {
        if let Ok(Some(resolved)) = app.core().musicbrainz_resolve_artist(name).await {
            if matches!(
                resolved.confidence,
                MatchConfidence::Exact | MatchConfidence::High | MatchConfidence::Medium
            ) && normalize(&resolved.name) == original_key
            {
                canonical_key = normalize(&resolved.name);
                canonical_name = resolved.name;
                mbid = Some(resolved.mbid);
            }
        }
    }

    if canonical_name != name {
        if let Ok(page) = app.core().search_artists(&canonical_name, 5, 0, None).await {
            if let Some(value) = page
                .items
                .into_iter()
                .find(|artist| normalize(&artist.name) == canonical_key)
            {
                if let Some(url) = value.image.as_ref().and_then(|image| image.for_px(150)) {
                    if useful_url(url) {
                        return Some(Resolution {
                            url: url.clone(),
                            source: "qobuz",
                            canonical_name: Some(value.name),
                        });
                    }
                }
            }
        }
    }

    if let Ok(found) = qbz_integrations::DiscogsClient::new()
        .search_artist(&canonical_name)
        .await
    {
        if let Some(value) = found.results.into_iter().find(|value| {
            value.result_type == "artist" && normalize_discogs_artist(&value.title) == canonical_key
        }) {
            if let Some(url) = value
                .cover_image
                .or(value.thumb)
                .filter(|url| useful_url(url))
            {
                return Some(Resolution {
                    url,
                    source: "discogs",
                    canonical_name: Some(canonical_name),
                });
            }
        }
    }

    let _ = mbid; // retained as identity evidence; no raster portrait endpoint.
    None
}

fn useful_url(url: &str) -> bool {
    let url = url.trim();
    !url.is_empty() && !url.contains("spacer.gif") && !url.contains(LASTFM_PLACEHOLDER_HASH)
}

fn normalize_discogs_artist(value: &str) -> String {
    let value = value.trim();
    let without_suffix = value
        .strip_suffix(')')
        .and_then(|prefix| prefix.rsplit_once(" ("))
        .filter(|(_, suffix)| suffix.chars().all(|ch| ch.is_ascii_digit()))
        .map(|(name, _)| name)
        .unwrap_or(value);
    normalize(without_suffix)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discogs_numeric_disambiguation_is_identity_not_a_name_change() {
        assert_eq!(normalize_discogs_artist("Genesis (2)"), "genesis");
        assert_eq!(normalize_discogs_artist("!!!"), "");
        assert_eq!(normalize_discogs_artist("blink-182"), "blink 182");
    }

    #[test]
    fn provider_placeholders_are_never_persisted_as_portraits() {
        assert!(!useful_url(
            "https://lastfm/2a96cbd8b46e442fc41c2b86b821562f.png"
        ));
        assert!(!useful_url("https://discogs/spacer.gif"));
        assert!(useful_url("https://static.qobuz.com/portrait.jpg"));
    }
}
