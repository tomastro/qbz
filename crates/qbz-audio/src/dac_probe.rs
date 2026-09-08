//! DAC hardware-state probe (HiFi wizard Slice 8b / N6).
//!
//! Reads the ACTUAL negotiated rate the DAC is running at, independent of what
//! QBZ *requested* — this is the ground truth for the wizard's playback test.
//! On Linux, `/proc/asound/cardN/pcm*p/sub0/hw_params` reports the live hardware
//! rate while a stream is open (`closed` when idle). The ALSA card number is
//! resolved from the PipeWire `node.name` via `pw-dump` (robust; needs only
//! `pipewire-bin`, not pactl/pipewire-pulse).
//!
//! Read-only: this never opens or reconfigures a stream, so the protected
//! bit-perfect / sample-rate-passthrough path is untouched.

use serde::{Deserialize, Serialize};
// Every target but Windows still runs `pw-dump`.
#[cfg(not(windows))]
use std::process::Command;

/// The DAC's live, negotiated hardware state — what the card is REALLY clocked
/// at, not what QBZ asked for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NegotiatedRate {
    /// Hardware sample rate the DAC is actually running at, in Hz.
    pub sample_rate: u32,
    /// ALSA hardware format string as reported (e.g. "S32_LE", "S24_3LE").
    /// Note: 24-bit audio is commonly carried in an `S32_LE` container — this is
    /// the ALSA container format, so the wizard verdict keys on the RATE.
    pub format: String,
    /// Channel count (e.g. 2).
    pub channels: u32,
}

/// Parse the contents of `/proc/asound/cardN/pcm*p/sub*/hw_params`.
///
/// Returns `None` when the device is idle (`closed`), empty, or has no `rate:`
/// line. Pure (no I/O) so it is unit-testable against captured fixtures.
pub fn parse_hw_params(content: &str) -> Option<NegotiatedRate> {
    let trimmed = content.trim();
    if trimmed.is_empty() || trimmed == "closed" {
        return None;
    }
    let mut sample_rate: Option<u32> = None;
    let mut format: Option<String> = None;
    let mut channels: Option<u32> = None;
    for line in content.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("rate:") {
            // "rate: 192000 (192000/1)" -> 192000
            sample_rate = rest
                .trim()
                .split_whitespace()
                .next()
                .and_then(|s| s.parse().ok());
        } else if let Some(rest) = line.strip_prefix("format:") {
            format = Some(rest.trim().to_string());
        } else if let Some(rest) = line.strip_prefix("channels:") {
            channels = rest.trim().parse().ok();
        }
    }
    let sample_rate = sample_rate?;
    Some(NegotiatedRate {
        sample_rate,
        format: format.unwrap_or_default(),
        channels: channels.unwrap_or(0),
    })
}

/// Pure helper: find the ALSA card number backing a PipeWire sink `node.name`
/// in `pw-dump` JSON. Reads `api.alsa.pcm.card` / `alsa.card` (string or int).
pub fn parse_alsa_card_for_node(json: &str, node_name: &str) -> Option<u32> {
    let root: serde_json::Value = serde_json::from_str(json).ok()?;
    let arr = root.as_array()?;
    for obj in arr {
        if obj.get("type").and_then(|v| v.as_str()) != Some("PipeWire:Interface:Node") {
            continue;
        }
        let props = match obj.get("info").and_then(|i| i.get("props")) {
            Some(p) => p,
            None => continue,
        };
        if props.get("node.name").and_then(|v| v.as_str()) != Some(node_name) {
            continue;
        }
        for key in ["api.alsa.pcm.card", "alsa.card", "card.id"] {
            if let Some(v) = props.get(key) {
                if let Some(n) = v.as_u64() {
                    return Some(n as u32);
                }
                if let Some(n) = v.as_str().and_then(|s| s.parse::<u32>().ok()) {
                    return Some(n);
                }
            }
        }
        return None; // matched the node but it carries no card property
    }
    None
}

/// Resolve the ALSA card number for a sink node by running `pw-dump`.
///
/// W14, Windows only: `pw-dump` does not exist there, so the spawn already
/// failed into this same None. Scoped to Windows rather than `not(linux)` --
/// a macOS or BSD box can have a real `pw-dump`, and this function is not the
/// place to decide it may not speak.
#[cfg(windows)]
fn alsa_card_for_node(_node_name: &str) -> Option<u32> {
    None
}

/// Resolve the ALSA card number for a sink node by running `pw-dump`.
#[cfg(not(windows))]
fn alsa_card_for_node(node_name: &str) -> Option<u32> {
    let output = Command::new("pw-dump").output().ok()?;
    if !output.status.success() {
        return None;
    }
    let json = String::from_utf8_lossy(&output.stdout);
    parse_alsa_card_for_node(&json, node_name)
}

/// Read the live hardware params for an ALSA card's playback substream.
fn read_hw_params_for_card(card: u32) -> Option<NegotiatedRate> {
    // The DAC's playback PCM is almost always pcm0p; scan a few in case of
    // multi-PCM cards (e.g. an HDMI PCM at index 0 and analog later).
    for pcm in 0..4 {
        let path = format!("/proc/asound/card{}/pcm{}p/sub0/hw_params", card, pcm);
        if let Ok(content) = std::fs::read_to_string(&path) {
            if let Some(nr) = parse_hw_params(&content) {
                return Some(nr);
            }
        }
    }
    None
}

/// Probe the DAC's live negotiated hardware rate for the given PipeWire sink
/// `node.name`. `None` = idle/closed or unresolvable. Read-only; safe to poll.
pub fn negotiated_stream_rate(node_name: &str) -> Option<NegotiatedRate> {
    let card = alsa_card_for_node(node_name)?;
    read_hw_params_for_card(card)
}

/// The negotiated rate of whichever ALSA card is ACTIVELY playing right now.
///
/// Scans every card's playback substream and returns the first one that is
/// open (not `closed`). This is DAC-agnostic — it reports the rate of whatever
/// QBZ is currently outputting to, so it works no matter which (or how many)
/// DACs the user selected; you can only play through one output at a time.
/// `None` = nothing is playing. Read-only; safe to poll.
pub fn negotiated_active_rate() -> Option<NegotiatedRate> {
    for card in 0..16 {
        if let Some(nr) = read_hw_params_for_card(card) {
            return Some(nr);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    // Real shape captured from /proc/asound/card1/pcm0p/sub0/hw_params while a
    // 24/192 stream was open on a Cambridge USB DAC.
    const ACTIVE: &str = "access: MMAP_INTERLEAVED\n\
format: S32_LE\n\
subformat: STD\n\
channels: 2\n\
rate: 192000 (192000/1)\n\
period_size: 2048\n\
buffer_size: 32768\n";

    #[test]
    fn parses_active_hw_params() {
        let nr = parse_hw_params(ACTIVE).expect("active stream parses");
        assert_eq!(nr.sample_rate, 192000);
        assert_eq!(nr.format, "S32_LE");
        assert_eq!(nr.channels, 2);
    }

    #[test]
    fn idle_or_empty_yields_none() {
        assert!(parse_hw_params("closed").is_none());
        assert!(parse_hw_params("closed\n").is_none());
        assert!(parse_hw_params("").is_none());
        // No rate line -> None (we can't assert a negotiated rate).
        assert!(parse_hw_params("format: S16_LE\nchannels: 2\n").is_none());
    }

    #[test]
    fn resolves_card_from_node_name() {
        let json = r#"[
          { "id": 53, "type": "PipeWire:Interface:Node",
            "info": { "props": {
              "media.class": "Audio/Sink",
              "node.name": "alsa_output.usb-Cambridge_Audio-00.analog-stereo",
              "api.alsa.pcm.card": 1, "alsa.card": 1 } } }
        ]"#;
        assert_eq!(
            parse_alsa_card_for_node(json, "alsa_output.usb-Cambridge_Audio-00.analog-stereo"),
            Some(1)
        );
        assert_eq!(parse_alsa_card_for_node(json, "alsa_output.unknown"), None);
    }

    #[test]
    fn resolves_card_when_only_string_prop_present() {
        let json = r#"[
          { "id": 7, "type": "PipeWire:Interface:Node",
            "info": { "props": {
              "media.class": "Audio/Sink",
              "node.name": "alsa_output.pci-x.analog-stereo",
              "alsa.card": "0" } } }
        ]"#;
        assert_eq!(
            parse_alsa_card_for_node(json, "alsa_output.pci-x.analog-stereo"),
            Some(0)
        );
    }
}
