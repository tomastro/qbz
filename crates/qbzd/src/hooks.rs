// crates/qbzd/src/hooks.rs — run a user script on daemon events (CONSOLE ext).
//
// Headless integrators (moOde, Volumio, home-grown setups) need PUSH
// notifications to coordinate the daemon with the rest of an audio box — stop
// the local player when a Qobuz Connect session starts playing, restore the
// volume when it ends — without keeping a poller or an SSE client alive.
// `hooks.script` names an executable the daemon forks once per bus event with
// the event described in `QBZ_*` environment variables, the same integration
// contract pleezer (`--hook`) and shairport-sync (`run_this_...`) established.
//
// Enablement: the `QBZD_HOOK` env var wins when set (empty disables);
// otherwise the persisted `daemon_prefs.hook_script` path, written by `qbzd
// settings set hooks.script`. Changing it needs a daemon restart (the
// dispatcher spawns at boot, like MPRIS).
//
// The dispatcher never waits for one script before receiving another event.
// It does, however, cap concurrent children so a stuck integration cannot
// turn a burst of events into an unbounded process storm. Scripts may overlap;
// an integrator needing strict ordering can flock inside the script.
use std::path::PathBuf;

use qbz_models::{CoreEvent, PlaybackState};
use tokio::sync::broadcast;
use tokio::task::{JoinHandle, JoinSet};

use crate::paths::ProfileRoots;

/// Hooks are an integration escape hatch, not a job runner. Four concurrent
/// children leave room for short event bursts while putting a hard ceiling on
/// a broken or hung script.
const MAX_CONCURRENT_HOOKS: usize = 4;

fn parse_script(raw: &str) -> Option<PathBuf> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let path = PathBuf::from(trimmed);
    if !path.is_absolute() {
        log::warn!(
            "[hooks] ignoring non-absolute script path '{}'; use an absolute path",
            path.display()
        );
        return None;
    }
    Some(path)
}

/// Resolve the configured hook script: `QBZD_HOOK` when set (empty string
/// disables), else the persisted `hooks.script` setting; None = hooks off.
pub fn script(roots: &ProfileRoots) -> Option<PathBuf> {
    let raw = match std::env::var("QBZD_HOOK") {
        Ok(v) => v,
        Err(_) => qbz_app::settings::daemon_prefs::load_at(&roots.data).hook_script,
    };
    parse_script(&raw)
}

/// Spawn the hook dispatcher. Holds no `Arc<AppRuntime>` (only the script path
/// and the bus receiver), so it sits outside the §8.2 audio-release ordering —
/// the caller aborts it for a clean shutdown, like the scrobbler.
pub fn spawn(script: PathBuf, mut rx: broadcast::Receiver<CoreEvent>) -> JoinHandle<()> {
    use broadcast::error::RecvError;
    tokio::spawn(async move {
        let mut children = JoinSet::new();
        loop {
            let ev = tokio::select! {
                received = rx.recv() => match received {
                    Ok(ev) => ev,
                    Err(RecvError::Lagged(skipped)) => {
                        log::warn!("[hooks] event receiver lagged; skipped {skipped} event(s)");
                        continue;
                    }
                    Err(RecvError::Closed) => return,
                },
                finished = children.join_next(), if !children.is_empty() => {
                    if let Some(Err(e)) = finished {
                        log::debug!("[hooks] reaper task ended unexpectedly: {e}");
                    }
                    continue;
                }
            };
            let Some(vars) = hook_env(&ev) else { continue };
            if children.len() >= MAX_CONCURRENT_HOOKS {
                log::warn!(
                    "[hooks] dropping event while {MAX_CONCURRENT_HOOKS} hook scripts are still running"
                );
                continue;
            }
            let mut cmd = tokio::process::Command::new(&script);
            cmd.envs(vars)
                .stdin(std::process::Stdio::null())
                .kill_on_drop(true);
            match cmd.spawn() {
                Ok(mut child) => {
                    // JoinSet owns every reaper. Dropping the dispatcher aborts
                    // them, and kill_on_drop then terminates their children.
                    children.spawn(async move {
                        let _ = child.wait().await;
                    });
                }
                Err(e) => log::warn!("[hooks] could not run {}: {e}", script.display()),
            }
        }
    })
}

fn state_label(state: PlaybackState) -> &'static str {
    match state {
        PlaybackState::Playing => "playing",
        PlaybackState::Paused => "paused",
        PlaybackState::Stopped => "stopped",
        PlaybackState::Loading => "loading",
    }
}

/// Map one bus event onto the script's `QBZ_*` environment, or None for
/// events the hook does not forward. Deliberately excluded: `PositionUpdated`
/// (a fork every ~2 s), `QueueUpdated` (bulky, and queue state is one
/// `GET /api/status` away), and everything on the SSE suppress list. The
/// `LoggedIn` session payload (auth token) is NEVER forwarded — the event
/// name alone is.
fn hook_env(ev: &CoreEvent) -> Option<Vec<(String, String)>> {
    use CoreEvent::*;
    let mut vars: Vec<(String, String)> = Vec::new();
    let mut push = |k: &str, v: String| vars.push((format!("QBZ_{k}"), v));
    match ev {
        TrackStarted {
            track,
            position_secs,
        } => {
            push("EVENT", "TrackStarted".into());
            push("TRACK_ID", track.id.to_string());
            push("TITLE", track.title.clone());
            push("ARTIST", track.artist.clone());
            push("ALBUM", track.album.clone());
            push("DURATION", track.duration_secs.to_string());
            push("POSITION", position_secs.to_string());
            if let Some(url) = &track.artwork_url {
                push("COVER_URL", url.clone());
            }
            if let Some(bits) = track.bit_depth {
                push("BIT_DEPTH", bits.to_string());
            }
            if let Some(rate) = track.sample_rate {
                push("SAMPLE_RATE", rate.to_string());
            }
        }
        PlaybackStateChanged { state } => {
            push("EVENT", "PlaybackStateChanged".into());
            push("STATE", state_label(*state).into());
        }
        TrackEnded { track_id } => {
            push("EVENT", "TrackEnded".into());
            push("TRACK_ID", track_id.to_string());
        }
        VolumeChanged { volume } => {
            push("EVENT", "VolumeChanged".into());
            // 0-100, the same scale the `qbzd volume` verb speaks.
            push(
                "VOLUME",
                (((*volume).clamp(0.0, 1.0) * 100.0).round() as u32).to_string(),
            );
        }
        QconnectSessionChanged {
            state,
            device_name,
            session_active,
        } => {
            push("EVENT", "QconnectSessionChanged".into());
            push("STATE", state.clone());
            push("SESSION_ACTIVE", session_active.to_string());
            if let Some(name) = device_name {
                push("DEVICE_NAME", name.clone());
            }
        }
        LoggedIn { .. } => push("EVENT", "LoggedIn".into()),
        LoggedOut => push("EVENT", "LoggedOut".into()),
        SessionExpired => push("EVENT", "SessionExpired".into()),
        Error {
            code,
            message,
            recoverable,
        } => {
            push("EVENT", "Error".into());
            push("CODE", code.clone());
            push("MESSAGE", message.clone());
            push("RECOVERABLE", recoverable.to_string());
        }
        PlaybackError { track_id, message } => {
            push("EVENT", "PlaybackError".into());
            push("TRACK_ID", track_id.to_string());
            push("MESSAGE", message.clone());
        }
        AudioDeviceChanged { device_name } => {
            push("EVENT", "AudioDeviceChanged".into());
            push("DEVICE_NAME", device_name.clone());
        }
        _ => return None,
    }
    // The full tagged JSON for scripts that prefer jq over positional vars —
    // except LoggedIn, whose session payload stays out of child environments.
    if !matches!(ev, LoggedIn { .. }) {
        if let Ok(json) = serde_json::to_string(ev) {
            vars.push(("QBZ_EVENT_JSON".to_string(), json));
        }
    }
    Some(vars)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get<'a>(vars: &'a [(String, String)], key: &str) -> Option<&'a str> {
        vars.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }

    #[test]
    fn hook_path_must_be_absolute() {
        assert_eq!(parse_script("  "), None);
        assert_eq!(parse_script("relative/hook.sh"), None);
        assert_eq!(
            parse_script(" /opt/qbz/hook.sh "),
            Some(PathBuf::from("/opt/qbz/hook.sh"))
        );
    }

    /// A minimal QueueTrack via its serde defaults (the struct has no Default
    /// impl, and adding one to the shared models is not this feature's call).
    fn test_track() -> qbz_models::QueueTrack {
        serde_json::from_value(serde_json::json!({
            "id": 42,
            "title": "Red Beans",
            "artist": "Jon Batiste",
            "album": "Monk Movements",
            "duration_secs": 163,
            "artwork_url": "https://example.com/cover.jpg",
            "bit_depth": 24,
            "sample_rate": 48.0,
            "album_id": null,
            "artist_id": null,
        }))
        .expect("QueueTrack from JSON")
    }

    #[test]
    fn track_started_carries_metadata_and_json() {
        let track = test_track();
        let vars = hook_env(&CoreEvent::TrackStarted {
            track,
            position_secs: 7,
        })
        .expect("TrackStarted is forwarded");
        assert_eq!(get(&vars, "QBZ_EVENT"), Some("TrackStarted"));
        assert_eq!(get(&vars, "QBZ_TITLE"), Some("Red Beans"));
        assert_eq!(get(&vars, "QBZ_ARTIST"), Some("Jon Batiste"));
        assert_eq!(get(&vars, "QBZ_DURATION"), Some("163"));
        assert_eq!(get(&vars, "QBZ_POSITION"), Some("7"));
        assert_eq!(get(&vars, "QBZ_BIT_DEPTH"), Some("24"));
        let json = get(&vars, "QBZ_EVENT_JSON").expect("JSON payload present");
        assert!(json.contains("\"type\":\"TrackStarted\""));
    }

    #[test]
    fn playback_state_uses_lowercase_labels() {
        for (state, label) in [
            (PlaybackState::Playing, "playing"),
            (PlaybackState::Paused, "paused"),
            (PlaybackState::Stopped, "stopped"),
            (PlaybackState::Loading, "loading"),
        ] {
            let vars = hook_env(&CoreEvent::PlaybackStateChanged { state }).unwrap();
            assert_eq!(get(&vars, "QBZ_STATE"), Some(label));
        }
    }

    #[test]
    fn qconnect_session_changed_is_forwarded() {
        let vars = hook_env(&CoreEvent::QconnectSessionChanged {
            state: "connected".into(),
            device_name: Some("Moode Qobuz".into()),
            session_active: true,
        })
        .unwrap();
        assert_eq!(get(&vars, "QBZ_EVENT"), Some("QconnectSessionChanged"));
        assert_eq!(get(&vars, "QBZ_STATE"), Some("connected"));
        assert_eq!(get(&vars, "QBZ_SESSION_ACTIVE"), Some("true"));
        assert_eq!(get(&vars, "QBZ_DEVICE_NAME"), Some("Moode Qobuz"));
    }

    #[test]
    fn logged_in_forwards_the_event_name_but_never_the_session() {
        let session: qbz_models::UserSession = serde_json::from_value(serde_json::json!({
            "user_auth_token": "SECRET-TOKEN",
            "user_id": 1,
            "email": "user@example.com",
            "display_name": "User",
            "subscription_label": "Studio",
        }))
        .expect("UserSession from JSON");
        let vars = hook_env(&CoreEvent::LoggedIn { session }).unwrap();
        assert_eq!(get(&vars, "QBZ_EVENT"), Some("LoggedIn"));
        // Redaction: no JSON payload, no token, exactly one variable.
        assert_eq!(vars.len(), 1);
    }

    #[test]
    fn chatty_and_bulky_events_are_not_forwarded() {
        assert!(
            hook_env(&CoreEvent::PositionUpdated {
                position_secs: 1,
                duration_secs: 2
            })
            .is_none()
        );
        assert!(hook_env(&CoreEvent::ShuffleChanged { enabled: true }).is_none());
        assert!(
            hook_env(&CoreEvent::LoadingStarted {
                operation: "x".into()
            })
            .is_none()
        );
    }

    #[test]
    fn volume_is_forwarded_on_the_cli_percent_scale() {
        let vars = hook_env(&CoreEvent::VolumeChanged { volume: 0.45 }).unwrap();
        assert_eq!(get(&vars, "QBZ_VOLUME"), Some("45"));
    }
}
