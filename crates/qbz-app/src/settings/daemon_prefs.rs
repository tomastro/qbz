// crates/qbz-app/src/settings/daemon_prefs.rs
//! The daemon's 2 player prefs (01-architecture.md §10.3). Lives in qbz-app so
//! qbzd AND settings::bundle share one struct (D2). NOT read by the desktop.
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DaemonPrefs {
    /// Same key contract as desktop ui_prefs.streaming_quality:
    /// "mp3" | "cd" | "hires" | "hires_plus" (crates/qbz/src/ui_prefs.rs:307-308)
    pub streaming_quality: String,
    /// Restored at boot; NEVER imported (04 §3 — power-amp hazard).
    pub volume: f32,
    /// Whether the daemon publishes MPRIS system media controls at boot
    /// (CONSOLE ext). Default ON; the `QBZD_MPRIS` env var overrides it when
    /// set. Toggling this needs a daemon restart (the service spawns at boot).
    pub mpris_enabled: bool,
    /// Absolute path of an executable the daemon runs on playback/session
    /// events (CONSOLE ext), with the event in `QBZ_*` environment variables.
    /// Empty = hooks off (the default). The `QBZD_HOOK` env var overrides it
    /// when set. Changing it needs a daemon restart (the dispatcher spawns at
    /// boot). Machine-local by design — NEVER part of the settings bundle.
    pub hook_script: String,
}
impl Default for DaemonPrefs {
    fn default() -> Self {
        Self {
            streaming_quality: "hires_plus".into(),
            volume: 0.5,
            mpris_enabled: true,
            hook_script: String::new(),
        }
    }
}

pub fn load_at(data_root: &Path) -> DaemonPrefs {
    let path = data_root.join("daemon_prefs.json");
    match std::fs::read_to_string(&path) {
        Ok(text) => match serde_json::from_str::<serde_json::Value>(&text) {
            Ok(v) => {
                // forward-compatible: unknown fields ignored WITH a warning (01 §10.3)
                if let Some(obj) = v.as_object() {
                    for k in obj.keys() {
                        if k != "streaming_quality"
                            && k != "volume"
                            && k != "mpris_enabled"
                            && k != "hook_script"
                        {
                            log::warn!("[daemon_prefs] unknown field ignored: {k}");
                        }
                    }
                }
                serde_json::from_value(v).unwrap_or_default()
            }
            Err(e) => {
                log::warn!("[daemon_prefs] unreadable, using defaults: {e}");
                DaemonPrefs::default()
            }
        },
        Err(_) => DaemonPrefs::default(),
    }
}

pub fn save_at(prefs: &DaemonPrefs, data_root: &Path) -> Result<(), String> {
    std::fs::create_dir_all(data_root).map_err(|e| e.to_string())?;
    std::fs::write(
        data_root.join("daemon_prefs.json"),
        serde_json::to_vec_pretty(prefs).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unique_test_dir(name: &str) -> std::path::PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "qbz-app-daemon-prefs-{name}-{}-{nonce}",
            std::process::id()
        ))
    }

    #[test]
    fn missing_file_returns_defaults() {
        let dir = unique_test_dir("missing");
        let prefs = load_at(&dir);
        assert_eq!(prefs.streaming_quality, "hires_plus");
        assert_eq!(prefs.volume, 0.5);
    }

    #[test]
    fn roundtrip_preserves_values_at_tempdir() {
        let dir = unique_test_dir("roundtrip");
        let prefs = DaemonPrefs {
            streaming_quality: "cd".into(),
            volume: 0.72,
            // Non-default (default is true) so the roundtrip actually exercises
            // the field, not just its presence.
            mpris_enabled: false,
            // Non-default (default is empty) for the same reason.
            hook_script: "/usr/local/bin/qbz-hook.sh".into(),
        };

        save_at(&prefs, &dir).expect("save daemon prefs");
        let loaded = load_at(&dir);

        assert_eq!(loaded.streaming_quality, "cd");
        assert_eq!(loaded.volume, 0.72);
        assert!(!loaded.mpris_enabled);
        assert_eq!(loaded.hook_script, "/usr/local/bin/qbz-hook.sh");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unknown_field_ignored_defaults_intact_for_known_fields() {
        let dir = unique_test_dir("unknown-field");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        std::fs::write(
            dir.join("daemon_prefs.json"),
            r#"{"streaming_quality":"mp3","volume":0.3,"bitrate_kbps":320}"#,
        )
        .expect("write raw prefs file");

        let loaded = load_at(&dir);

        assert_eq!(loaded.streaming_quality, "mp3");
        assert_eq!(loaded.volume, 0.3);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
