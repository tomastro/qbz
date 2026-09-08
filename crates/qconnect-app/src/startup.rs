//! QConnect startup mode: persisted preference for whether QConnect
//! auto-connects when the app launches.
//!
//! Pure logic only — no SQLite, no FS. Persistence lives in the Tauri
//! adapter (src-tauri/src/qconnect/startup.rs) matching the existing
//! `device_name` persistence pattern in `transport.rs`.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum QconnectStartupMode {
    /// Never auto-connect on startup. Default for fresh installs.
    #[default]
    Off,
    /// Always auto-connect on every startup, regardless of last state.
    On,
    /// Restore the on/off state captured at last connect/disconnect.
    RememberLast,
}

impl QconnectStartupMode {
    pub fn as_str(self) -> &'static str {
        match self {
            QconnectStartupMode::Off => "off",
            QconnectStartupMode::On => "on",
            QconnectStartupMode::RememberLast => "remember_last",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "off" => Some(QconnectStartupMode::Off),
            "on" => Some(QconnectStartupMode::On),
            "remember_last" => Some(QconnectStartupMode::RememberLast),
            _ => None,
        }
    }
}

/// Decide whether QConnect should auto-connect at startup.
///
/// CLI override (when present) wins over the persisted mode.
/// `last_known` is only consulted when mode == RememberLast.
pub fn compute_effective_startup(
    mode: QconnectStartupMode,
    cli_override: Option<bool>,
    last_known: Option<bool>,
) -> bool {
    if let Some(v) = cli_override {
        return v;
    }
    match mode {
        QconnectStartupMode::Off => false,
        QconnectStartupMode::On => true,
        QconnectStartupMode::RememberLast => last_known.unwrap_or(false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_off() {
        assert_eq!(QconnectStartupMode::default(), QconnectStartupMode::Off);
    }

    #[test]
    fn round_trip_str() {
        for m in [
            QconnectStartupMode::Off,
            QconnectStartupMode::On,
            QconnectStartupMode::RememberLast,
        ] {
            assert_eq!(QconnectStartupMode::from_str(m.as_str()), Some(m));
        }
    }

    #[test]
    fn from_str_unknown_value_returns_none() {
        assert_eq!(QconnectStartupMode::from_str("auto"), None);
        assert_eq!(QconnectStartupMode::from_str(""), None);
    }

    #[test]
    fn cli_override_true_wins_over_mode() {
        assert!(compute_effective_startup(
            QconnectStartupMode::Off,
            Some(true),
            None
        ));
        assert!(compute_effective_startup(
            QconnectStartupMode::Off,
            Some(true),
            Some(false)
        ));
    }

    #[test]
    fn cli_override_false_wins_over_mode() {
        assert!(!compute_effective_startup(
            QconnectStartupMode::On,
            Some(false),
            None
        ));
        assert!(!compute_effective_startup(
            QconnectStartupMode::RememberLast,
            Some(false),
            Some(true)
        ));
    }

    #[test]
    fn mode_off_with_no_cli_returns_false() {
        assert!(!compute_effective_startup(
            QconnectStartupMode::Off,
            None,
            None
        ));
        assert!(!compute_effective_startup(
            QconnectStartupMode::Off,
            None,
            Some(true)
        ));
    }

    #[test]
    fn mode_on_with_no_cli_returns_true() {
        assert!(compute_effective_startup(
            QconnectStartupMode::On,
            None,
            None
        ));
        assert!(compute_effective_startup(
            QconnectStartupMode::On,
            None,
            Some(false)
        ));
    }

    #[test]
    fn mode_remember_last_uses_last_known() {
        assert!(compute_effective_startup(
            QconnectStartupMode::RememberLast,
            None,
            Some(true)
        ));
        assert!(!compute_effective_startup(
            QconnectStartupMode::RememberLast,
            None,
            Some(false)
        ));
    }

    #[test]
    fn mode_remember_last_defaults_off_when_no_history() {
        assert!(!compute_effective_startup(
            QconnectStartupMode::RememberLast,
            None,
            None
        ));
    }
}
