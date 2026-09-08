//! Headless DAC capability probe (HiFi wizard Slice 2).
//!
//! Mirrors the old Tauri `v2_query_dac_capabilities` so the Slint wizard can
//! query a DAC's real supported sample rates + pretty description WITHOUT a
//! Tauri command. The actual detection already lived in this crate
//! (`PipeWireBackend::get_sink_supported_rates`, `get_device_supported_rates`);
//! this module just assembles the DTO frontend-agnostically. Read-only.

use serde::{Deserialize, Serialize};

/// DAC capability descriptor surfaced to the wizard.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DacCapabilities {
    pub node_name: String,
    pub sample_rates: Vec<u32>,
    pub formats: Vec<String>,
    pub channels: Option<u32>,
    pub description: Option<String>,
    pub error: Option<String>,
    /// True when `sample_rates` came from real detection (`/proc/asound` /
    /// ALSA), false when it is the `FALLBACK_RATES` common set. Consumers that
    /// derive a quality cap from the ceiling (#638 fix 3) must disclose the
    /// fallback case — the cap still applies, but is not guaranteed to match
    /// the hardware. `serde(default)` keeps old serialized payloads readable.
    #[serde(default)]
    pub detected: bool,
}

/// The common rate set used when real detection fails (continuous-range DACs,
/// non-USB devices, missing `/proc/asound` stream files).
const FALLBACK_RATES: [u32; 6] = [44100, 48000, 88200, 96000, 176400, 192000];

/// Nominal format list. NOTE: these are NOT probed — ALSA does not expose a
/// clean per-node format capability here, so we report the common set honestly
/// as nominal (matches the prior Tauri behavior; do not present it as measured).
fn nominal_formats() -> Vec<String> {
    vec![
        "S16LE".to_string(),
        "S24LE".to_string(),
        "F32LE".to_string(),
    ]
}

/// Pure assembly (testable): combine detected rates + description into the DTO,
/// falling back to the common rate set when detection yielded nothing.
fn assemble(
    node_name: &str,
    detected_rates: Option<Vec<u32>>,
    description: Option<String>,
) -> DacCapabilities {
    // Computed BEFORE the fallback collapses into `sample_rates`, so the DTO
    // can tell a real ceiling from the assumed common set (#638 F26).
    let rates = detected_rates.filter(|r| !r.is_empty());
    let detected = rates.is_some();
    let sample_rates = rates.unwrap_or_else(|| FALLBACK_RATES.to_vec());
    DacCapabilities {
        node_name: node_name.to_string(),
        sample_rates,
        formats: nominal_formats(),
        channels: Some(2),
        description,
        error: None,
        detected,
    }
}

/// Detect a DAC's real supported sample rates + pretty description, headlessly.
/// Read-only — reads `/proc/asound` and runs `pw-dump`; never opens a stream.
pub fn query_dac_capabilities(node_name: &str) -> DacCapabilities {
    // Pretty description from robust (pw-dump-backed, Slice 0) enumeration.
    let description =
        crate::backend::BackendManager::create_backend(crate::backend::AudioBackendType::PipeWire)
            .ok()
            .and_then(|b| b.enumerate_devices().ok())
            .and_then(|devs| {
                devs.into_iter()
                    .find(|d| d.id == node_name || d.name == node_name)
            })
            .map(|d| d.description.unwrap_or(d.name));

    // Real sample rates: PipeWire sink -> ALSA card -> /proc/asound, with an
    // ALSA-direct fallback. Hardware-only; non-Linux gets the fallback set.
    #[cfg(target_os = "linux")]
    let detected = crate::pipewire_backend::PipeWireBackend::get_sink_supported_rates(node_name)
        .or_else(|| crate::alsa_backend::get_device_supported_rates(node_name));
    #[cfg(not(target_os = "linux"))]
    let detected: Option<Vec<u32>> = None;

    assemble(node_name, detected, description)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uses_detected_rates_when_present() {
        let caps = assemble(
            "alsa_output.usb-x",
            Some(vec![44100, 96000, 192000]),
            Some("My DAC".into()),
        );
        assert_eq!(caps.sample_rates, vec![44100, 96000, 192000]);
        assert_eq!(caps.description.as_deref(), Some("My DAC"));
        assert_eq!(caps.channels, Some(2));
        assert!(caps.error.is_none());
        assert!(caps.detected);
    }

    #[test]
    fn falls_back_when_detection_empty_or_missing() {
        let none = assemble("x", None, None);
        assert_eq!(none.sample_rates, FALLBACK_RATES.to_vec());
        assert!(!none.detected);
        let empty = assemble("x", Some(vec![]), None);
        assert_eq!(empty.sample_rates, FALLBACK_RATES.to_vec());
        assert!(!empty.detected);
    }

    #[test]
    fn formats_are_nominal_not_empty() {
        let caps = assemble("x", None, None);
        assert!(caps.formats.contains(&"S24LE".to_string()));
    }
}
