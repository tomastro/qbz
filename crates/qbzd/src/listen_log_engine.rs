// crates/qbzd/src/listen_log_engine.rs — the daemon's listen log.
//
// A sibling of `scrobble_engine` on the same authority-stamped playback bus, WITHOUT its
// `settings.enabled` gate: the scrobbler's switch is about sending plays
// elsewhere; the log is local and on by default (its own switch lives in
// `listen_meta.paused`, shared with the desktop's Settings toggle). Until
// this existed a headless streamer contributed zero history.
//
// Edges (events_bridge.rs): `TrackStarted` opens a row and closes the
// previous one — the bus has no TrackEnded (nobody emits it), so the close
// reason is INFERRED: natural when the last position sat within 2 s of the
// duration, skip otherwise. `PositionUpdated` (only sent while playing)
// feeds the accumulator. `PlaybackStateChanged{Stopped}` closes the row with
// the same inference. The task is aborted on shutdown; `shutdown_blocking`
// closes whatever is open first.

use std::sync::Arc;

use qbz_app::listen_log::{meta_from_queue_track, ListenLogger, Origin};
use qbz_models::{CoreEvent, PlaybackState};
use tokio::sync::broadcast;
use tokio::task::JoinHandle;

use crate::events_bridge::AuthorityStampedEvent;
use crate::paths::ProfileRoots;
use crate::qconnect::authority::{AuthorityCell, OwnerAuthorityToken};

/// The bound logger, so the daemon's shutdown path can close the open row
/// after aborting the task.
pub struct ListenLogTask {
    pub handle: JoinHandle<()>,
    logger: Option<Arc<ListenLogger>>,
}

impl ListenLogTask {
    /// Abort the subscriber and close the row in flight as `shutdown`.
    pub async fn stop(self) {
        self.handle.abort();
        let _ = self.handle.await;
        if let Some(logger) = self.logger {
            logger.shutdown_blocking();
        }
    }
}

/// Open the daemon-root store (`<data>/listen/listen_log.db`, origin
/// `qbzd:<hostname>`) and subscribe. A failed open logs and yields an inert
/// task — the daemon must boot without a history file.
pub async fn spawn(
    roots: &ProfileRoots,
    rx: broadcast::Receiver<AuthorityStampedEvent>,
    authority: Arc<AuthorityCell>,
) -> ListenLogTask {
    let hostname = qbz_app::settings::bundle::hostname();
    let logger = match ListenLogger::open(roots.data.clone(), Origin::Daemon { hostname }).await {
        Ok(l) => l,
        Err(e) => {
            log::warn!("[listen-log] open failed; the daemon records no history: {e}");
            return ListenLogTask {
                handle: tokio::spawn(async {}),
                logger: None,
            };
        }
    };
    let handle = tokio::spawn(run(Arc::clone(&logger), rx, authority));
    ListenLogTask {
        handle,
        logger: Some(logger),
    }
}

async fn run(
    logger: Arc<ListenLogger>,
    mut rx: broadcast::Receiver<AuthorityStampedEvent>,
    authority: Arc<AuthorityCell>,
) {
    use broadcast::error::RecvError;
    let mut active_owner_token: Option<OwnerAuthorityToken> = None;
    loop {
        match rx.recv().await {
            Ok(event) => {
                handle_event(&logger, &authority, &mut active_owner_token, event).await
            }
            Err(RecvError::Lagged(n)) => {
                // Position ticks were dropped: the accumulator simply misses
                // them (a gap > 5 s credits nothing), which is the honest
                // outcome — never extrapolate. Also close the open row: the
                // missing span may contain an authority handoff.
                log::debug!("[listen-log] bus lagged by {n} events");
                active_owner_token = None;
                logger.handoff().await;
            }
            Err(RecvError::Closed) => return,
        }
    }
}

async fn handle_event(
    logger: &ListenLogger,
    authority: &Arc<AuthorityCell>,
    active_owner_token: &mut Option<OwnerAuthorityToken>,
    stamped: AuthorityStampedEvent,
) {
    let Some(owner_token) = stamped.owner_token else {
        *active_owner_token = None;
        logger.handoff().await;
        return;
    };
    let Some(permit) = authority
        .wait_for_exact_owner_action_permit(owner_token)
        .await
    else {
        // A stale old-owner delivery must not close a row already opened by a
        // newer owner generation. Close only the row that this rejected event
        // could have belonged to (or an untracked open row).
        if active_owner_token
            .as_ref()
            .is_none_or(|current| *current == owner_token)
        {
            *active_owner_token = None;
            logger.handoff().await;
        }
        return;
    };

    // A later owner generation is a real authority boundary even when the bus
    // lagged over the intervening guest event. Never let the restored owner
    // continue accumulating into the pre-handoff row.
    if active_owner_token.is_some_and(|current| current != owner_token) {
        logger.handoff().await;
    }
    *active_owner_token = Some(owner_token);

    match stamped.event {
        CoreEvent::TrackStarted { track, .. } => {
            let meta = meta_from_queue_track(&track, None, None, None);
            logger.track_started(meta, true).await;
        }
        CoreEvent::PositionUpdated { position_secs, .. } => {
            logger.tick(position_secs * 1_000, true).await;
        }
        CoreEvent::PlaybackStateChanged { state } if state == PlaybackState::Stopped => {
            logger.stopped(true).await;
        }
        _ => {}
    }
    // The exact admission covers all logger awaits and their SQLite writes.
    drop(permit);
}

#[cfg(test)]
mod tests {
    use qbz_app::listen_log::{EndReason, Origin};
    use qbz_models::QueueTrack;

    use crate::qconnect::authority::AuthorityOrigin;

    use super::*;

    fn track(id: u64) -> QueueTrack {
        QueueTrack {
            id,
            title: format!("Track {id}"),
            version: None,
            artist: "Artist".into(),
            album: "Album".into(),
            album_version: None,
            duration_secs: 180,
            artwork_url: None,
            hires: false,
            bit_depth: None,
            sample_rate: None,
            is_local: false,
            album_id: None,
            artist_id: None,
            streamable: true,
            source: Some("qobuz".into()),
            parental_warning: false,
            source_item_id_hint: None,
            context_kind: None,
            context_id: None,
            isrc: None,
            recording_mbid: None,
        }
    }

    #[tokio::test]
    async fn guest_handoff_closes_owner_and_ignores_guest_playback() {
        let dir = tempfile::tempdir().unwrap();
        let logger = ListenLogger::open(dir.path().to_path_buf(), Origin::Install)
            .await
            .unwrap();
        let authority = Arc::new(AuthorityCell::new());
        let owner = authority.reserve(AuthorityOrigin::Owner);
        assert!(authority.install(owner));
        let (owner_token, permit) = authority
            .try_owner_action_permit_observed()
            .expect("owner observation");
        drop(permit);
        let mut active_owner_token = None;

        handle_event(
            &logger,
            &authority,
            &mut active_owner_token,
            AuthorityStampedEvent {
                event: CoreEvent::TrackStarted {
                    track: track(1),
                    position_secs: 0,
                },
                owner_token: Some(owner_token),
            },
        )
        .await;
        handle_event(
            &logger,
            &authority,
            &mut active_owner_token,
            AuthorityStampedEvent {
                event: CoreEvent::PositionUpdated {
                    position_secs: 1,
                    duration_secs: 180,
                },
                owner_token: Some(owner_token),
            },
        )
        .await;

        let guest = authority.reserve(AuthorityOrigin::Delegated { generation: 9 });
        assert!(authority.install(guest));
        handle_event(
            &logger,
            &authority,
            &mut active_owner_token,
            AuthorityStampedEvent {
                event: CoreEvent::TrackStarted {
                    track: track(2),
                    position_secs: 0,
                },
                owner_token: None,
            },
        )
        .await;
        handle_event(
            &logger,
            &authority,
            &mut active_owner_token,
            AuthorityStampedEvent {
                event: CoreEvent::PositionUpdated {
                    position_secs: 30,
                    duration_secs: 180,
                },
                owner_token: None,
            },
        )
        .await;

        let restored_owner = authority.reserve(AuthorityOrigin::Owner);
        assert!(authority.install(restored_owner));
        handle_event(
            &logger,
            &authority,
            &mut active_owner_token,
            AuthorityStampedEvent {
                event: CoreEvent::TrackStarted {
                    track: track(3),
                    position_secs: 0,
                },
                // A late event from the pre-handoff owner must not be promoted
                // merely because owner authority is available again.
                owner_token: Some(owner_token),
            },
        )
        .await;

        let rows = logger.rows().await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].source_item_id, "1");
        assert_eq!(rows[0].end_reason, Some(EndReason::Handoff));
        assert!(!logger.has_open_row());
    }
}
