use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TrackOrigin {
    QobuzOnline,
    QobuzOfflineCache,
    LocalLibrary,
    Plex,
    ExternalUnknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdmissionDecision {
    pub accepted: bool,
    pub reason: &'static str,
}

impl AdmissionDecision {
    pub const fn allow(reason: &'static str) -> Self {
        Self {
            accepted: true,
            reason,
        }
    }

    pub const fn block(reason: &'static str) -> Self {
        Self {
            accepted: false,
            reason,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HandoffIntent {
    ContinueLocally,
    SendToConnect,
}

pub fn evaluate_remote_queue_admission(origin: TrackOrigin) -> AdmissionDecision {
    match origin {
        TrackOrigin::QobuzOnline => AdmissionDecision::allow("qobuz_online_source"),
        TrackOrigin::QobuzOfflineCache => AdmissionDecision::allow("qobuz_offline_cache_source"),
        TrackOrigin::LocalLibrary => {
            AdmissionDecision::block("local_library_tracks_never_enter_remote_qconnect_queue")
        }
        TrackOrigin::Plex => {
            AdmissionDecision::block("plex_tracks_never_enter_remote_qconnect_queue")
        }
        TrackOrigin::ExternalUnknown => {
            AdmissionDecision::block("unknown_origin_blocked_for_remote_qconnect_queue")
        }
    }
}

pub fn resolve_handoff_intent(origin: TrackOrigin) -> HandoffIntent {
    match origin {
        TrackOrigin::QobuzOnline | TrackOrigin::QobuzOfflineCache => HandoffIntent::SendToConnect,
        TrackOrigin::LocalLibrary | TrackOrigin::Plex | TrackOrigin::ExternalUnknown => {
            HandoffIntent::ContinueLocally
        }
    }
}

/// Server-side backstop: re-evaluate EVERY track's origin, independent of any
/// command-level `origin`. A bare `track_id` cannot prove "Qobuz vs local/Plex"
/// on its own, so the frontend ships per-track origins and the gate re-validates
/// each one here. An empty list is blocked: we cannot prove the queue is
/// all-Qobuz, so we refuse rather than trust the command-level origin.
pub fn validate_track_origins_for_admission(origins: &[TrackOrigin]) -> AdmissionDecision {
    if origins.is_empty() {
        return AdmissionDecision::block("empty_track_origins_blocked");
    }
    for origin in origins {
        let decision = evaluate_remote_queue_admission(*origin);
        if !decision.accepted {
            return decision;
        }
    }
    AdmissionDecision::allow("all_track_origins_qobuz")
}

#[cfg(test)]
mod tests {
    use super::*;
    use qbz_models::PlaybackSource;

    #[test]
    fn blocks_command_when_any_track_origin_is_non_qobuz() {
        assert!(
            validate_track_origins_for_admission(&[
                TrackOrigin::QobuzOnline,
                TrackOrigin::QobuzOfflineCache
            ])
            .accepted
        );
        assert!(
            !validate_track_origins_for_admission(&[
                TrackOrigin::QobuzOnline,
                TrackOrigin::LocalLibrary
            ])
            .accepted
        );
        assert!(!validate_track_origins_for_admission(&[TrackOrigin::Plex]).accepted);
        assert!(!validate_track_origins_for_admission(&[TrackOrigin::ExternalUnknown]).accepted);
        assert!(!validate_track_origins_for_admission(&[]).accepted); // empty -> blocked
    }

    #[test]
    fn admission_matches_cast_predicate() {
        let pairs = [
            (TrackOrigin::QobuzOnline, PlaybackSource::Qobuz),
            (TrackOrigin::QobuzOfflineCache, PlaybackSource::OfflineCache),
            (TrackOrigin::LocalLibrary, PlaybackSource::Local),
            (TrackOrigin::Plex, PlaybackSource::Plex),
        ];
        for (origin, source) in pairs {
            assert_eq!(
                evaluate_remote_queue_admission(origin).accepted,
                source.is_castable_to_qconnect(),
                "admission/predicate disagree for {origin:?}",
            );
        }
    }

    #[test]
    fn offline_cache_is_admitted() {
        assert!(evaluate_remote_queue_admission(TrackOrigin::QobuzOfflineCache).accepted);
    }
}
