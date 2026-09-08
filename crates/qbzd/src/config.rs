// crates/qbzd/src/config.rs — qbzd.toml: PROCESS concerns only (D14 single-source rule).
// Engine settings (audio/playback/qconnect content) live in the stores — never here.
// QConnect startup_mode/device_name/volume_mode live SOLELY in the daemon-root
// qconnect_settings.db KV (03 §3.4/§6; helpers land in T9) — no [qconnect] table.
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize)] // Serialize: `qbzd config show --json` (T11)
#[serde(default)]
pub struct QbzdConfig {
    pub config_version: u32,
    pub data_root: Option<String>, // container override; cache root derived
    pub server: ServerCfg,
    pub log: LogCfg,
    pub mpris: MprisCfg, // documented now, inert in P0 (01 §11)
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct ServerCfg {
    pub bind: String,
    pub port: u16,
    /// Opt-in shared secret (02 §3.1.2). Default `None` = the control plane is
    /// UNAUTHENTICATED (loopback and LAN alike). When set, every route except
    /// `GET /api/ping` requires `Authorization: Bearer <token>`; a mismatch is
    /// `401 invalid_token`. A plain config value the user writes — there is no
    /// generated file and no rotation verb (rotate = edit this + restart).
    pub token: Option<String>,
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct LogCfg {
    pub level: String,
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct MprisCfg {
    pub enabled: bool,
}

impl Default for ServerCfg {
    fn default() -> Self {
        Self {
            bind: "0.0.0.0".into(), // LAN-first posture (FB6): Sonos/Chromecast-style
            // open renderer; the Origin shield still guards browsers and
            // `[server] token` remains the opt-in restriction for powerusers.
            port: 8182,
            token: None, // open by default (02 §3.1.2)
        }
    }
}
impl Default for LogCfg {
    fn default() -> Self {
        Self {
            level: "info".into(),
        }
    }
}
impl Default for MprisCfg {
    fn default() -> Self {
        Self { enabled: true }
    }
}
impl Default for QbzdConfig {
    fn default() -> Self {
        Self {
            config_version: 1,
            data_root: None,
            server: Default::default(),
            log: Default::default(),
            mpris: Default::default(),
        }
    }
}

/// Known keys, one entry per (table, key). Kept literal so the sweep and the
/// spec table diff cleanly. Released keys are never renamed without an alias.
const KNOWN: &[(&str, &str)] = &[
    ("", "config_version"),
    ("", "data_root"),
    ("server", "bind"),
    ("server", "port"),
    ("server", "token"),
    ("log", "level"),
    ("mpris", "enabled"),
];

impl QbzdConfig {
    pub fn from_str(text: &str) -> Result<(Self, Vec<String>), String> {
        let value: toml::Value = toml::from_str(text).map_err(|e| e.to_string())?;
        let mut warns = Vec::new();
        sweep(&value, "", &mut warns);
        let cfg: QbzdConfig = value.try_into().map_err(|e: toml::de::Error| e.to_string())?;
        Ok((cfg, warns))
    }
    pub fn load(path: &std::path::Path) -> Result<(Self, Vec<String>), String> {
        match std::fs::read_to_string(path) {
            Ok(t) => Self::from_str(&t),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                Ok((Self::default(), Vec::new()))
            }
            Err(e) => Err(format!("cannot read {}: {e}", path.display())),
        }
    }
}

fn sweep(v: &toml::Value, table: &str, warns: &mut Vec<String>) {
    if let toml::Value::Table(map) = v {
        for (k, inner) in map {
            match inner {
                toml::Value::Table(_) if table.is_empty() => sweep(inner, k, warns),
                _ if !KNOWN.contains(&(table, k.as_str())) => {
                    warns.push(if table.is_empty() {
                        k.clone()
                    } else {
                        format!("[{table}].{k}")
                    });
                }
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn defaults_match_spec() {
        // 01-architecture.md §10.1 (FB6: default bind widened to 0.0.0.0 for
        // LAN-first control — restrict via [server] bind or token to opt back
        // into loopback-only).
        let (c, warns) = QbzdConfig::from_str("").unwrap();
        assert_eq!(c.config_version, 1);
        assert_eq!(c.server.bind, "0.0.0.0");
        assert_eq!(c.server.port, 8182);
        assert_eq!(c.log.level, "info");
        assert!(c.mpris.enabled);
        assert!(warns.is_empty());
    }
    #[test]
    fn unknown_keys_warn_never_error() {
        // D14 / operator §5.4 (J5 silent-revert guard)
        let (_c, warns) = QbzdConfig::from_str("[server]\nbindd = \"0.0.0.0\"\n").unwrap();
        assert_eq!(warns, vec!["[server].bindd".to_string()]);
    }
    #[test]
    fn server_token_defaults_none_and_parses_when_set() {
        // 02-cli-and-api.md §3.1.2: `[server] token` is opt-in — absent = None
        // (open control plane); present = the shared secret, no warning.
        let (open, warns) = QbzdConfig::from_str("").unwrap();
        assert_eq!(open.server.token, None);
        assert!(warns.is_empty());

        let (secured, warns) =
            QbzdConfig::from_str("[server]\ntoken = \"s3cret\"\n").unwrap();
        assert_eq!(secured.server.token.as_deref(), Some("s3cret"));
        assert!(warns.is_empty(), "known key must not warn: {warns:?}");
    }
    #[test]
    fn server_token_empty_string_parses_as_present_but_filtering_gates_it() {
        // Empty or whitespace-only tokens in the config file parse successfully,
        // but are filtered to None by daemon.rs and client.rs to prevent
        // enabling auth with an empty secret.
        let (cfg, warns) = QbzdConfig::from_str("[server]\ntoken = \"\"\n").unwrap();
        assert_eq!(cfg.server.token, Some("".to_string()));
        assert!(warns.is_empty());

        let (cfg_ws, warns) =
            QbzdConfig::from_str("[server]\ntoken = \"   \"\n").unwrap();
        assert_eq!(cfg_ws.server.token, Some("   ".to_string()));
        assert!(warns.is_empty());
    }
}
