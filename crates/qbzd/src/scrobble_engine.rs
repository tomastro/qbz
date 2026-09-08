// crates/qbzd/src/scrobble_engine.rs — scrobble-on-play (CONSOLE ext).
//
// A daemon background task subscribing the authority-stamped playback-event
// bus. On `TrackStarted` it sends "now playing" to every ACTIVE provider; when the
// track crosses the scrobble threshold (`qbz_app::scrobble_timing::
// scrobble_delay_secs` — Last.fm's played-half-or-4-min rule) it scrobbles
// ONCE. Credentials are re-read from the canonical `ScrobblerSettingsStore` on
// each track start, so `qbzd scrobble …` changes take effect on the next track
// with no reload signal. Best-effort + logged.
//
// Providers: Last.fm (LastFmClient::update_now_playing / scrobble) and
// ListenBrainz (submit_playing_now / submit_listen). Both backends are
// qbz-integrations (Slint-free).
//
// ListenBrainz has a persistent offline queue: a failed `submit_listen` is
// written to the SHARED `ListenBrainzCache.listen_queue` (daemon-root
// `cache/listenbrainz_v2.db`, the same schema the desktop uses) and a periodic
// drain — plus one drain at task start — retries pending listens oldest-first,
// stopping at the first failure and resuming on the next tick. The rusqlite
// Connection is never held across an await: it is opened inside a
// `spawn_blocking` for each queue/drain op (mirrors `qbz::scrobble`).
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use qbz_app::settings::scrobblers::{ScrobblerSettings, ScrobblerSettingsStore};
use qbz_integrations::lastfm::LastFmClient;
use qbz_integrations::listenbrainz::cache::ListenBrainzCache;
use qbz_integrations::listenbrainz::{AdditionalInfo, ListenBrainzClient, ListenBrainzConfig};
use qbz_models::{CoreEvent, QueueTrack};
use tokio::sync::broadcast;
use tokio::task::JoinHandle;

use crate::events_bridge::AuthorityStampedEvent;
use crate::paths::ProfileRoots;
use crate::qconnect::authority::{AuthorityCell, OwnerAuthorityToken};

/// How often the ListenBrainz offline queue is retried (plus once at task
/// start). Live now-playing/scrobble submits are unaffected — the drain only
/// clears listens that a prior submit could not deliver.
const DRAIN_INTERVAL: Duration = Duration::from_secs(120);

/// The track currently being timed for a scrobble.
struct Playing {
    track: QueueTrack,
    /// Exact owner observation that admitted this track start. A position from
    /// any later owner generation must not finish this pending scrobble.
    owner_token: OwnerAuthorityToken,
    /// Unix seconds when it started — Last.fm's scrobble timestamp.
    started_at: u64,
    /// Seconds into the track at which it becomes scrobble-eligible; `None`
    /// means "too short to scrobble" (`scrobble_delay_secs` returned None).
    threshold: Option<u64>,
    scrobbled: bool,
}

/// Spawn the scrobble-on-play task. Holds NO `Arc<AppRuntime>` (only the roots,
/// its own store, and the bus receiver), so it is outside the §8.2 audio
/// clock-release ordering — the caller aborts it for a clean shutdown.
pub fn spawn(
    roots: ProfileRoots,
    mut rx: broadcast::Receiver<AuthorityStampedEvent>,
    authority: Arc<AuthorityCell>,
) -> JoinHandle<()> {
    use broadcast::error::RecvError;
    tokio::spawn(async move {
        let store = match ScrobblerSettingsStore::new_at(&roots.data) {
            Ok(s) => s,
            Err(e) => {
                log::warn!("[scrobbler] store open failed; scrobbling disabled: {e}");
                return;
            }
        };
        let mut playing: Option<Playing> = None;
        // Fires immediately on the first tick (drains any queue left from a
        // prior offline session), then every DRAIN_INTERVAL.
        let mut drain = tokio::time::interval(DRAIN_INTERVAL);
        drain.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            // `biased`: live bus events take priority over the drain tick.
            tokio::select! {
                biased;
                received = rx.recv() => match received {
                    Ok(stamped) => {
                        let Some(owner_token) = stamped.owner_token else {
                            playing = None;
                            continue;
                        };
                        let Some(permit) = authority
                            .wait_for_exact_owner_action_permit(owner_token)
                            .await
                        else {
                            playing = None;
                            continue;
                        };
                        if playing
                            .as_ref()
                            .is_some_and(|current| current.owner_token != owner_token)
                        {
                            playing = None;
                        }

                        match stamped.event {
                            CoreEvent::TrackStarted { track, .. } => {
                                let settings = store.get_settings().unwrap_or_default();
                                if !settings.enabled {
                                    playing = None;
                                    continue;
                                }
                                now_playing(&settings, &track).await;
                                playing = Some(Playing {
                                    threshold: qbz_app::scrobble_timing::scrobble_delay_secs(track.duration_secs),
                                    started_at: now_unix(),
                                    track,
                                    owner_token,
                                    scrobbled: false,
                                });
                            }
                            CoreEvent::PositionUpdated { position_secs, .. } => {
                                if let Some(p) = playing.as_mut() {
                                    if due(position_secs, p.threshold, p.scrobbled) {
                                        let settings = store.get_settings().unwrap_or_default();
                                        p.scrobbled = scrobble(
                                            &settings,
                                            &p.track,
                                            p.started_at,
                                            &roots,
                                        )
                                        .await;
                                    }
                                }
                            }
                            _ => {}
                        }
                        // Keep the exact admission alive through every provider
                        // await and any failed-listen persistence above.
                        drop(permit);
                    }
                    Err(RecvError::Lagged(_)) => continue,
                    Err(RecvError::Closed) => return,
                },
                _ = drain.tick() => {
                    let settings = store.get_settings().unwrap_or_default();
                    if settings.enabled && settings.listenbrainz_active() {
                        drain_listenbrainz(&settings, &roots).await;
                    }
                }
            }
        }
    })
}

// ============================ internals ============================

/// Whether the current track is due to scrobble now: it has a threshold, has
/// been played to it, and hasn't been scrobbled yet. Pure — unit-tested.
fn due(position_secs: u64, threshold: Option<u64>, scrobbled: bool) -> bool {
    !scrobbled && threshold.is_some_and(|t| position_secs >= t)
}

async fn now_playing(s: &ScrobblerSettings, t: &QueueTrack) {
    let album = album_opt(t);
    if s.lastfm_active() {
        let c = LastFmClient::with_session_key(s.lastfm_session_key.clone());
        if let Err(e) = c.update_now_playing(&t.artist, &t.title, album).await {
            log::debug!("[scrobbler] last.fm now-playing failed: {e}");
        }
    }
    if s.listenbrainz_active() {
        let c = lb_client(s);
        if let Err(e) = c.submit_playing_now(&t.artist, &t.title, album, None).await {
            log::debug!("[scrobbler] listenbrainz now-playing failed: {e}");
        }
    }
}

async fn scrobble(
    s: &ScrobblerSettings,
    t: &QueueTrack,
    started_at: u64,
    roots: &ProfileRoots,
) -> bool {
    let album = album_opt(t);
    if s.lastfm_active() {
        let c = LastFmClient::with_session_key(s.lastfm_session_key.clone());
        match c.scrobble(&t.artist, &t.title, album, started_at).await {
            Ok(()) => log::info!("[scrobbler] last.fm scrobbled: {} — {}", t.artist, t.title),
            Err(e) => log::warn!("[scrobbler] last.fm scrobble failed: {e}"),
        }
    }
    if s.listenbrainz_active() {
        let c = lb_client(s);
        match c
            .submit_listen(&t.artist, &t.title, album, started_at as i64, None)
            .await
        {
            Ok(()) => log::info!(
                "[scrobbler] listenbrainz submitted: {} — {}",
                t.artist,
                t.title
            ),
            Err(e) => {
                // Persist to the shared offline queue; the periodic drain retries it.
                log::warn!("[scrobbler] listenbrainz submit failed, queueing: {e}");
                queue_listenbrainz(roots, t, started_at as i64).await;
            }
        }
    }
    true
}

/// Persist a failed listen into the SHARED `ListenBrainzCache.listen_queue`
/// (daemon-root `cache/listenbrainz_v2.db`). Opened inside a `spawn_blocking` so
/// the rusqlite Connection never crosses an await.
async fn queue_listenbrainz(roots: &ProfileRoots, t: &QueueTrack, timestamp: i64) {
    let Some(path) = lb_cache_path(roots) else {
        return;
    };
    let artist = t.artist.clone();
    let track = t.title.clone();
    let album = album_opt(t).map(str::to_string);
    let duration_ms = (t.duration_secs > 0).then_some(t.duration_secs * 1000);
    let _ = tokio::task::spawn_blocking(move || match ListenBrainzCache::new(&path) {
        Ok(cache) => {
            if let Err(e) = cache.queue_listen(
                timestamp,
                &artist,
                &track,
                album.as_deref(),
                None,
                None,
                None,
                None,
                duration_ms,
            ) {
                log::warn!("[scrobbler] queue listenbrainz listen failed: {e}");
            }
        }
        Err(e) => log::warn!("[scrobbler] open listenbrainz cache failed: {e}"),
    })
    .await;
}

/// Drain pending ListenBrainz listens oldest-first, stopping at the first
/// failure (still offline / flaky — retry on the next tick). Mirrors
/// `qbz::scrobble::flush_listenbrainz_queue`.
async fn drain_listenbrainz(s: &ScrobblerSettings, roots: &ProfileRoots) {
    let Some(path) = lb_cache_path(roots) else {
        return;
    };
    let pending = match tokio::task::spawn_blocking({
        let path = path.clone();
        move || ListenBrainzCache::new(&path).and_then(|c| c.get_pending_listens(500))
    })
    .await
    {
        Ok(Ok(p)) => p,
        _ => return,
    };
    if pending.is_empty() {
        return;
    }

    let client = lb_client(s);
    let mut sent_ids: Vec<i64> = Vec::new();
    for item in pending {
        let info = AdditionalInfo {
            recording_mbid: item.recording_mbid.clone(),
            release_mbid: item.release_mbid.clone(),
            artist_mbids: item.artist_mbids.clone(),
            isrc: item.isrc.clone(),
            duration_ms: item.duration_ms,
            ..Default::default()
        };
        if client
            .submit_listen(
                &item.artist_name,
                &item.track_name,
                item.release_name.as_deref(),
                item.listened_at,
                Some(info),
            )
            .await
            .is_ok()
        {
            sent_ids.push(item.id);
        } else {
            break; // still failing — retry on the next tick
        }
    }
    if !sent_ids.is_empty() {
        let count = sent_ids.len();
        let _ = tokio::task::spawn_blocking(move || {
            ListenBrainzCache::new(&path).and_then(|c| c.mark_listens_sent(&sent_ids))
        })
        .await;
        log::info!("[scrobbler] listenbrainz drain: {count} listen(s) sent");
    }
}

/// The daemon-root shared ListenBrainz cache DB — `<cache>/listenbrainz_v2.db`,
/// the same file name and schema the desktop opens (the daemon uses its own
/// `qbzd` cache root; it never touches the desktop's dirs).
fn lb_cache_path(roots: &ProfileRoots) -> Option<PathBuf> {
    std::fs::create_dir_all(&roots.cache).ok()?;
    Some(roots.cache.join("listenbrainz_v2.db"))
}

/// A ListenBrainz client bound to the stored token, with its own enabled flag
/// ON (submit_* early-returns if the client config is disabled — our gate is
/// `ScrobblerSettings::listenbrainz_active`, checked before calling).
fn lb_client(s: &ScrobblerSettings) -> ListenBrainzClient {
    ListenBrainzClient::with_config(ListenBrainzConfig {
        enabled: true,
        token: Some(s.listenbrainz_token.clone()),
        user_name: Some(s.listenbrainz_username.clone()),
    })
}

/// The album name, unless it's empty or the queue-track "Unknown Album"
/// placeholder (both scrobble better as "no album" than a fake one).
fn album_opt(t: &QueueTrack) -> Option<&str> {
    if t.album.is_empty() || t.album == "Unknown Album" {
        None
    } else {
        Some(&t.album)
    }
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use crate::qconnect::authority::AuthorityOrigin;

    use super::*;

    #[test]
    fn due_only_when_past_threshold_and_not_yet_scrobbled() {
        assert!(!due(10, Some(120), false)); // not there yet
        assert!(due(120, Some(120), false)); // exactly at threshold
        assert!(due(200, Some(120), false)); // past it
        assert!(!due(200, Some(120), true)); // already scrobbled
        assert!(!due(999, None, false)); // too short to scrobble (no threshold)
    }

    fn qt(album: &str) -> QueueTrack {
        QueueTrack {
            id: 1,
            title: "Spain".into(),
            version: None,
            artist: "Chick Corea".into(),
            album: album.into(),
            album_version: None,
            duration_secs: 300,
            artwork_url: None,
            hires: false,
            bit_depth: None,
            sample_rate: None,
            is_local: false,
            album_id: None,
            artist_id: None,
            streamable: true,
            source: None,
            parental_warning: false,
            source_item_id_hint: None,
            context_kind: None,
            context_id: None,
            isrc: None,
            recording_mbid: None,
        }
    }

    #[test]
    fn album_opt_drops_empty_and_unknown() {
        assert_eq!(
            album_opt(&qt("Light as a Feather")),
            Some("Light as a Feather")
        );
        assert_eq!(album_opt(&qt("")), None);
        assert_eq!(album_opt(&qt("Unknown Album")), None);
    }

    #[test]
    fn stale_owner_generation_cannot_finish_pending_scrobble() {
        let authority = Arc::new(AuthorityCell::new());
        let owner = authority.reserve(AuthorityOrigin::Owner);
        assert!(authority.install(owner));
        let (old_token, permit) = authority
            .try_owner_action_permit_observed()
            .expect("owner observation");
        drop(permit);

        let guest = authority.reserve(AuthorityOrigin::Delegated { generation: 4 });
        assert!(authority.install(guest));
        let restored_owner = authority.reserve(AuthorityOrigin::Owner);
        assert!(authority.install(restored_owner));

        assert!(authority
            .try_owner_action_permit_exact(old_token)
            .is_none());
    }
}
