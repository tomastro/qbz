//! Listen log — the per-user record of what was actually heard.
//!
//! Contract: `qbz-nix-docs/research/2026-08-28-listening-log-offline-reco-analysis.md`
//! §12.1. Capture only: this module has NO consumer in the app. No rail,
//! mix, insight or suggestion reads it; the older stores
//! (`recently_played.json`, `album_play_history.db`,
//! `playlist_play_history.db`) keep running unchanged beside it.
//!
//! - [`store`]   — the SQLite schema (PK = event) and its migrations.
//! - [`tracker`] — the pure state machine (accumulator, end reasons).
//! - [`rules`]   — the reading thresholds (`is_play`, `is_skip`).
//! - [`logger`]  — the async facade the Qt poll loop and `qbzd` drive.

pub mod logger;
pub mod rules;
pub mod store;
pub mod tracker;

pub use logger::{now_unix, ListenLogger, Origin};
pub use rules::{is_play, is_skip, EndReason};
pub use store::{ListenRow, ListenStore};
pub use tracker::{ListenMeta, ListenTracker};

/// Ephemeral ids (CD / ad-hoc folder rows) live above 2^48 and never reach
/// the log (owner rule, mirrored from `recently_qt.rs:329`).
pub const EPHEMERAL_ID_FLOOR: u64 = 1 << 48;

/// Build the row snapshot from a queue track plus what the host knows about
/// the stream. `source_item_id` is the NATIVE id when the queue carries one
/// (media-server item ids, Plex rating keys, offline row ids travel in
/// `source_item_id_hint` for local-tier rows); for Qobuz the hint is a
/// container id and the track id IS the native id.
pub fn meta_from_queue_track(
    track: &qbz_models::QueueTrack,
    bit_depth: Option<u32>,
    sample_rate_hz: Option<u32>,
    output_backend: Option<String>,
) -> ListenMeta {
    let source = track
        .source
        .clone()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            if track.is_local {
                "local".into()
            } else {
                "qobuz".into()
            }
        });
    // "qobuz_download" is the offline cache's own source word in the local
    // library; the contract's vocabulary calls that tier `offline`.
    let source = if source == "qobuz_download" {
        "offline".to_string()
    } else {
        source
    };
    let source_item_id = match (&track.source_item_id_hint, track.is_local) {
        (Some(hint), true) if !hint.is_empty() => hint.clone(),
        _ => track.id.to_string(),
    };
    ListenMeta {
        source,
        source_item_id,
        track_id: Some(track.id as i64),
        album_id: track.album_id.clone().filter(|a| !a.is_empty()),
        artist_id: track.artist_id.map(|id| id.to_string()),
        isrc: track.isrc.clone().filter(|s| !s.is_empty()),
        recording_mbid: track.recording_mbid.clone().filter(|s| !s.is_empty()),
        title: track.title.clone(),
        artist: track.artist.clone(),
        album: Some(track.album.clone()).filter(|a| !a.is_empty()),
        album_artist: None,
        artwork_key: track.artwork_url.clone().filter(|a| !a.is_empty()),
        duration_ms: track.duration_secs * 1_000,
        context_kind: track.context_kind.clone().unwrap_or_default(),
        context_id: track.context_id.clone().unwrap_or_default(),
        bit_depth: bit_depth.or(track.bit_depth),
        sample_rate: sample_rate_hz
            .or_else(|| track.sample_rate.map(|khz| (khz * 1000.0).round() as u32)),
        output_backend,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track() -> qbz_models::QueueTrack {
        qbz_models::QueueTrack {
            id: 7,
            title: "Song".into(),
            version: None,
            artist: "Band".into(),
            album: "LP".into(),
            album_version: None,
            duration_secs: 200,
            artwork_url: Some("http://art".into()),
            hires: true,
            bit_depth: Some(24),
            sample_rate: Some(96.0),
            is_local: false,
            album_id: Some("alb".into()),
            artist_id: Some(9),
            streamable: true,
            source: Some("qobuz".into()),
            parental_warning: false,
            source_item_id_hint: Some("alb".into()),
            context_kind: Some("playlist".into()),
            context_id: Some("pl-1".into()),
            isrc: Some("USRC17607839".into()),
            recording_mbid: None,
        }
    }

    #[test]
    fn qobuz_uses_the_track_id_not_the_container_hint() {
        let m = meta_from_queue_track(&track(), None, None, None);
        assert_eq!(m.source, "qobuz");
        assert_eq!(m.source_item_id, "7");
        assert_eq!(m.track_id, Some(7));
        assert_eq!(m.duration_ms, 200_000);
        assert_eq!(m.context_kind, "playlist");
        assert_eq!(m.context_id, "pl-1");
        assert_eq!(m.isrc.as_deref(), Some("USRC17607839"));
        assert_eq!(m.sample_rate, Some(96_000));
        assert_eq!(m.bit_depth, Some(24));
    }

    #[test]
    fn local_tier_rows_use_the_native_hint_and_offline_is_renamed() {
        let mut t = track();
        t.is_local = true;
        t.source = Some("jellyfin".into());
        t.source_item_id_hint = Some("item-abc".into());
        let m = meta_from_queue_track(&t, Some(16), Some(44_100), Some("alsa".into()));
        assert_eq!(m.source, "jellyfin");
        assert_eq!(m.source_item_id, "item-abc");
        assert_eq!(m.bit_depth, Some(16));
        assert_eq!(m.sample_rate, Some(44_100));
        assert_eq!(m.output_backend.as_deref(), Some("alsa"));

        t.source = Some("qobuz_download".into());
        assert_eq!(
            meta_from_queue_track(&t, None, None, None).source,
            "offline"
        );

        t.source = None;
        t.source_item_id_hint = None;
        let m = meta_from_queue_track(&t, None, None, None);
        assert_eq!(m.source, "local");
        assert_eq!(m.source_item_id, "7");
    }
}
