//! PipeWire audio backend
//!
//! Uses PipeWire/PulseAudio for audio output with device selection.
//! - Enumerates devices using pactl (pretty names)
//! - Sets PULSE_SINK environment variable for device routing
//! - Creates stream using CPAL "pulse" or "pipewire" device
//! - Does NOT change system default (only affects QBZ)

use super::backend::{AudioBackend, AudioBackendType, AudioDevice, BackendConfig, BackendResult};
use rodio::{
    cpal::{
        traits::{DeviceTrait, HostTrait},
        BufferSize, SampleFormat, StreamConfig, SupportedBufferSize, SupportedStreamConfig,
    },
    DeviceSinkBuilder, MixerDeviceSink,
};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};

/// Set true whenever QBZ writes a global `clock.force-rate` to the PipeWire
/// graph, so `reset_pipewire_clock` only resets a force WE applied and never
/// clobbers another app's intentional forced rate (issue #263, leak fix).
/// The force and the reset both run on the audio thread, so `Relaxed` is enough.
static CLOCK_FORCE_APPLIED: AtomicBool = AtomicBool::new(false);

/// Restores the `PIPEWIRE_NODE` env var to its previous value when dropped, so
/// locked-mode sink targeting (Tier 2a, #263) does not leak into later stream
/// opens. Edition 2021: `set_var`/`remove_var` are safe.
#[cfg(target_os = "linux")]
struct PwNodeEnvGuard(Option<String>);

#[cfg(target_os = "linux")]
impl Drop for PwNodeEnvGuard {
    fn drop(&mut self) {
        match self.0.take() {
            Some(v) => std::env::set_var("PIPEWIRE_NODE", v),
            None => std::env::remove_var("PIPEWIRE_NODE"),
        }
    }
}

pub struct PipeWireBackend {
    #[allow(dead_code)]
    host: rodio::cpal::Host,
}

impl PipeWireBackend {
    pub fn new() -> BackendResult<Self> {
        Ok(Self {
            host: rodio::cpal::default_host(),
        })
    }

    /// Reset PipeWire clock.force-rate and clock.force-quantum to 0.
    /// Call this when playback stops so other apps aren't stuck at a forced rate.
    /// Quantum reset is kept for safety even though we no longer force it.
    pub fn reset_pipewire_clock() {
        // Only reset a force WE applied — otherwise stopping QBZ would clobber
        // another app's intentional clock.force-rate. This also makes the call
        // safe to invoke unconditionally on every stop/suspend (issue #263 leak
        // fix): previously the reset was gated on `pw_force_bitperfect`, but QBZ
        // forces the clock for ANY non-locked PipeWire stream, so a plain
        // PipeWire (no-passthrough) user left the graph force-clocked after stop.
        if !CLOCK_FORCE_APPLIED.swap(false, Ordering::Relaxed) {
            return;
        }
        log::info!("[PipeWire Backend] Resetting clock.force-rate and clock.force-quantum to 0");
        let _ = Command::new("pw-metadata")
            .args(["-n", "settings", "0", "clock.force-rate", "0"])
            .output();
        let _ = Command::new("pw-metadata")
            .args(["-n", "settings", "0", "clock.force-quantum", "0"])
            .output();
    }

    /// Get the ALSA card number for a PipeWire/PulseAudio sink name.
    /// Parses `pactl list sinks` to find the `alsa.card` property.
    fn get_alsa_card_for_sink(sink_name: &str) -> Option<String> {
        let output = Command::new("pactl")
            .args(["list", "sinks"])
            .output()
            .ok()?;

        if !output.status.success() {
            return None;
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut in_target_sink = false;

        for line in stdout.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("Sink #") {
                if in_target_sink {
                    return None; // Passed target sink without finding alsa.card
                }
            } else if trimmed.starts_with("Name:") {
                let name = trimmed.trim_start_matches("Name:").trim();
                in_target_sink = name == sink_name;
            } else if in_target_sink && trimmed.starts_with("alsa.card = ") {
                let card = trimmed
                    .trim_start_matches("alsa.card = ")
                    .trim_matches('"')
                    .to_string();
                return Some(card);
            }
        }

        None
    }

    /// Query the DAC's supported sample rates from /proc/asound/cardN/stream0.
    /// Returns None if rates can't be determined (non-USB device, continuous range, etc.)
    pub fn get_sink_supported_rates(sink_name: &str) -> Option<Vec<u32>> {
        let alsa_card = Self::get_alsa_card_for_sink(sink_name)?;

        let stream_path = format!("/proc/asound/card{}/stream0", alsa_card);
        let content = std::fs::read_to_string(&stream_path).ok()?;

        // Collect all rates from Playback Rates: lines (handles multiple alt settings)
        let mut in_playback = false;
        let mut all_rates = Vec::new();

        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed == "Playback:" {
                in_playback = true;
            } else if trimmed == "Capture:" {
                in_playback = false;
            }
            if in_playback && trimmed.starts_with("Rates:") {
                let rates_str = trimmed.trim_start_matches("Rates:").trim();
                if rates_str.contains("continuous") {
                    return None; // Any rate in range is supported
                }
                for rate_str in rates_str.split(',') {
                    if let Ok(rate) = rate_str.trim().parse::<u32>() {
                        if !all_rates.contains(&rate) {
                            all_rates.push(rate);
                        }
                    }
                }
            }
        }

        if all_rates.is_empty() {
            None
        } else {
            all_rates.sort();
            Some(all_rates)
        }
    }

    /// Query the current PipeWire graph sample rate via pw-metadata.
    /// The rate the PipeWire graph is ACTUALLY running at.
    ///
    /// `clock.force-rate` is the key this backend writes and the one that
    /// drives the graph; `clock.rate` is the graph's NOMINAL default and does
    /// not move when a rate is forced. Verifying the former by reading the
    /// latter is how this check spent releases reporting "Audio may play at
    /// wrong speed" over perfectly correct output — measured on a Fosi ZH3:
    /// `clock.force-rate` = 96000 and `clock.rate` = 48000 at the same instant,
    /// with the DAC's own display reading 96 kHz. The retry did not help
    /// because the delay was never the problem.
    ///
    /// Falls back to `clock.rate` when nothing is forced (`force-rate` then
    /// reads 0 or is absent), which is the honest answer in that case.
    fn get_pipewire_current_rate() -> Option<u32> {
        Self::read_pw_setting("clock.force-rate")
            .filter(|rate| *rate > 0)
            .or_else(|| Self::read_pw_setting("clock.rate"))
    }

    /// One `pw-metadata` settings read, parsed. Split out so the two keys above
    /// cannot drift apart.
    fn read_pw_setting(key: &str) -> Option<u32> {
        let output = Command::new("pw-metadata")
            .args(["-n", "settings", "0", key])
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        // pw-metadata output: "Found "settings" metadata 0\nupdate: id:0 key:'clock.rate' value:'96000' type:''"
        // EXACT key match: `clock.rate` is a SUBSTRING of `clock.force-rate`, so
        // a `contains(key)` would happily read the forced rate while asked for
        // the nominal one. pw-metadata prints `key:'<name>'`.
        let needle = format!("key:'{key}'");
        for line in stdout.lines() {
            if line.contains(&needle) && line.contains("value:") {
                // Extract value between single quotes after "value:"
                if let Some(start) = line.find("value:'") {
                    let after = &line[start + 7..];
                    if let Some(end) = after.find('\'') {
                        return after[..end].parse::<u32>().ok();
                    }
                }
            }
        }
        None
    }

    /// Find the best fallback sample rate in the same family.
    /// 44.1kHz family: 44100, 88200, 176400, 352800
    /// 48kHz family: 48000, 96000, 192000, 384000
    fn find_best_fallback_rate(requested: u32, supported: &[u32]) -> u32 {
        let is_441_family = requested % 44100 == 0;

        // Find highest supported rate in the same family that's <= requested
        let mut candidates: Vec<u32> = supported
            .iter()
            .filter(|&&r| {
                if is_441_family {
                    r % 44100 == 0
                } else {
                    r % 48000 == 0
                }
            })
            .filter(|&&r| r <= requested)
            .copied()
            .collect();
        candidates.sort();

        if let Some(&best) = candidates.last() {
            return best;
        }

        // No rate in the same family — use highest supported rate overall
        supported.iter().copied().max().unwrap_or(48000)
    }

    /// Parse pactl output to get device list with pretty names
    fn enumerate_pipewire_sinks(&self) -> BackendResult<Vec<AudioDevice>> {
        // Get default sink
        let default_sink = Command::new("pactl")
            .args(["get-default-sink"])
            .output()
            .ok()
            .and_then(|o| {
                if o.status.success() {
                    String::from_utf8(o.stdout)
                        .ok()
                        .map(|s| s.trim().to_string())
                } else {
                    None
                }
            });

        // Get all sinks with details
        let output = Command::new("pactl")
            .args(["list", "sinks"])
            .output()
            .map_err(|e| format!("Failed to run pactl: {}", e))?;

        if !output.status.success() {
            return Err("pactl command failed".to_string());
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut devices = Vec::new();

        // Parse pactl output
        let mut current_name: Option<String> = None;
        let mut current_description: Option<String> = None;
        let mut current_max_rate: Option<u32> = None;
        let mut current_is_hardware: bool = false;
        let mut current_device_bus: Option<String> = None;

        for line in stdout.lines() {
            let line = line.trim();

            if line.starts_with("Sink #") {
                // Save previous device if complete
                if let (Some(id), Some(name)) = (current_name.take(), current_description.take()) {
                    let is_default = default_sink.as_ref().map(|d| d == &id).unwrap_or(false);
                    devices.push(AudioDevice {
                        id: id.clone(),
                        name,
                        description: None,
                        is_default,
                        max_sample_rate: current_max_rate.take(),
                        supported_sample_rates: None, // PipeWire handles sample rate conversion
                        device_bus: current_device_bus.take(),
                        is_hardware: current_is_hardware,
                    });
                }
                current_max_rate = None;
                current_is_hardware = false;
                current_device_bus = None;
            } else if line.starts_with("Name:") {
                current_name = Some(line.trim_start_matches("Name:").trim().to_string());
            } else if line.starts_with("Description:") {
                current_description =
                    Some(line.trim_start_matches("Description:").trim().to_string());
            } else if line.starts_with("Flags:") {
                // Check for HARDWARE flag
                current_is_hardware = line.contains("HARDWARE");
            } else if line.contains("Sample Specification:") {
                // Try to parse sample rate from lines like "Sample Specification: s32le 2ch 192000Hz"
                if let Some(hz_pos) = line.find("Hz") {
                    let before_hz = &line[..hz_pos];
                    if let Some(last_space) = before_hz.rfind(' ') {
                        if let Ok(rate) = before_hz[last_space + 1..].parse::<u32>() {
                            current_max_rate = Some(rate);
                        }
                    }
                }
            } else if line.starts_with("device.bus = ") {
                // Parse device.bus property (e.g., "usb", "pci", "bluetooth")
                let bus = line
                    .trim_start_matches("device.bus = ")
                    .trim_matches('"')
                    .to_string();
                current_device_bus = Some(bus);
            }
        }

        // Don't forget the last device
        if let (Some(id), Some(name)) = (current_name, current_description) {
            let is_default = default_sink.as_ref().map(|d| d == &id).unwrap_or(false);
            devices.push(AudioDevice {
                id,
                name,
                description: None,
                is_default,
                max_sample_rate: current_max_rate,
                supported_sample_rates: None, // PipeWire handles sample rate conversion
                device_bus: current_device_bus,
                is_hardware: current_is_hardware,
            });
        }

        log::info!(
            "[PipeWire Backend] Enumerated {} devices via pactl",
            devices.len()
        );
        for (idx, dev) in devices.iter().enumerate() {
            log::info!(
                "  [{}] {} (id: {}, bus: {:?}, hw: {})",
                idx,
                dev.name,
                dev.id,
                dev.device_bus,
                dev.is_hardware
            );
        }

        Ok(devices)
    }

    /// Native-PipeWire enumeration via `pw-dump`.
    ///
    /// This talks to the PipeWire daemon over its own socket and needs ONLY
    /// `pipewire-bin` (the `pw-*` tools) — it does NOT require `pipewire-pulse`
    /// (the Pulse-protocol server `pactl` talks to) nor the `pipewire-alsa`
    /// bridge PCM that CPAL relies on. On a PipeWire-only box missing those
    /// packages (the reported Ubuntu "empty sink list" bug) the legacy `pactl`
    /// and CPAL paths return nothing, but `pw-dump` still reports every sink —
    /// and gives us the exact `alsa_output.*` `node.name` for free.
    ///
    /// Returns `None` when `pw-dump` is absent, fails, or finds no sink (so the
    /// caller falls back to `pactl`). Discovery only — never touches the
    /// stream-open path.
    fn enumerate_via_pw_dump(&self) -> Option<Vec<AudioDevice>> {
        let output = Command::new("pw-dump").output().ok()?;
        if !output.status.success() {
            return None;
        }
        let json = String::from_utf8_lossy(&output.stdout);
        let devices = parse_pw_dump_sinks(&json);
        if devices.is_empty() {
            return None;
        }
        log::info!(
            "[PipeWire Backend] Enumerated {} sink(s) via pw-dump (native, no pactl/pipewire-pulse needed)",
            devices.len()
        );
        for (idx, dev) in devices.iter().enumerate() {
            log::info!(
                "  [{}] {} (id: {}, bus: {:?}, hw: {}, default: {})",
                idx,
                dev.name,
                dev.id,
                dev.device_bus,
                dev.is_hardware,
                dev.is_default
            );
        }
        Some(devices)
    }
}

/// Runs `program args...` with all stdio discarded and reports whether it
/// exited successfully within `timeout`. Availability probing only — a missing
/// binary, a failing exit, or a timeout all mean "not available". Bounded so
/// the audio thread can never hang on a wedged daemon (#591/#592: playback
/// start must fail fast and fall back, not sit behind the 45s UI watchdog).
fn probe_command_ok(program: &str, args: &[&str], timeout: std::time::Duration) -> bool {
    use std::process::Stdio;
    let mut child = match Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(_) => return false, // binary not present (sandbox / not installed)
    };
    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) => {
                if start.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    log::warn!(
                        "[PipeWire Backend] availability probe `{}` timed out after {:?}",
                        program,
                        timeout
                    );
                    return false;
                }
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
            Err(e) => {
                log::warn!(
                    "[PipeWire Backend] availability probe `{}` failed: {}",
                    program,
                    e
                );
                return false;
            }
        }
    }
}

/// Parse `pw-dump` JSON into our `AudioDevice` list. Pure (no I/O) so it is
/// unit-testable against a captured fixture.
///
/// Selects objects of `type == "PipeWire:Interface:Node"` whose
/// `info.props["media.class"] == "Audio/Sink"`. The `node.name` is the id the
/// HiFi wizard otherwise asks the user to paste by hand. `device.bus` is read
/// from the node props (present in practice) and, when absent, cross-referenced
/// via the node's `device.id` against the `PipeWire:Interface:Device` objects.
/// `max_sample_rate` is intentionally left `None`: `pw-dump`'s `EnumFormat`
/// reports the CURRENTLY negotiated rate, not the device maximum — the
/// capability probe (`/proc/asound/.../stream0`) is the honest source for that.
fn parse_pw_dump_sinks(json: &str) -> Vec<AudioDevice> {
    let root: serde_json::Value = match serde_json::from_str(json) {
        Ok(v) => v,
        Err(e) => {
            log::warn!("[PipeWire Backend] pw-dump JSON parse failed: {}", e);
            return Vec::new();
        }
    };
    let arr = match root.as_array() {
        Some(a) => a,
        None => return Vec::new(),
    };

    // Default sink name lives in the "default" Metadata object.
    let mut default_sink: Option<String> = None;
    for obj in arr {
        let is_default_meta = obj.get("type").and_then(|v| v.as_str())
            == Some("PipeWire:Interface:Metadata")
            && obj
                .get("props")
                .and_then(|p| p.get("metadata.name"))
                .and_then(|v| v.as_str())
                == Some("default");
        if !is_default_meta {
            continue;
        }
        if let Some(entries) = obj.get("metadata").and_then(|m| m.as_array()) {
            for entry in entries {
                if entry.get("key").and_then(|v| v.as_str()) == Some("default.audio.sink") {
                    if let Some(name) = entry
                        .get("value")
                        .and_then(|v| v.get("name"))
                        .and_then(|v| v.as_str())
                    {
                        default_sink = Some(name.to_string());
                    }
                }
            }
        }
    }

    // device.id -> device.bus, used only when the bus is not on the node itself.
    let mut device_bus: std::collections::HashMap<i64, String> = std::collections::HashMap::new();
    for obj in arr {
        if obj.get("type").and_then(|v| v.as_str()) != Some("PipeWire:Interface:Device") {
            continue;
        }
        if let Some(id) = obj.get("id").and_then(|v| v.as_i64()) {
            if let Some(bus) = obj
                .get("info")
                .and_then(|i| i.get("props"))
                .and_then(|p| p.get("device.bus"))
                .and_then(|v| v.as_str())
            {
                device_bus.insert(id, bus.to_string());
            }
        }
    }

    let mut devices = Vec::new();
    for obj in arr {
        if obj.get("type").and_then(|v| v.as_str()) != Some("PipeWire:Interface:Node") {
            continue;
        }
        let props = match obj.get("info").and_then(|i| i.get("props")) {
            Some(p) => p,
            None => continue,
        };
        if props.get("media.class").and_then(|v| v.as_str()) != Some("Audio/Sink") {
            continue;
        }
        let node_name = match props.get("node.name").and_then(|v| v.as_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };
        let name = props
            .get("node.description")
            .and_then(|v| v.as_str())
            .or_else(|| props.get("node.nick").and_then(|v| v.as_str()))
            .map(|s| s.to_string())
            .unwrap_or_else(|| node_name.clone());
        let bus = props
            .get("device.bus")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .or_else(|| {
                props
                    .get("device.id")
                    .and_then(|v| v.as_i64())
                    .and_then(|did| device_bus.get(&did).cloned())
            });
        // A real ALSA-backed sink = hardware (USB/PCI DAC, internal card).
        let is_hardware = props.get("device.api").and_then(|v| v.as_str()) == Some("alsa")
            || props
                .get("factory.name")
                .and_then(|v| v.as_str())
                .map(|f| f.contains("alsa"))
                .unwrap_or(false);
        let is_default = default_sink.as_deref() == Some(node_name.as_str());

        devices.push(AudioDevice {
            id: node_name,
            name,
            description: None,
            is_default,
            max_sample_rate: None,
            supported_sample_rates: None,
            device_bus: bus,
            is_hardware,
        });
    }
    devices
}

impl AudioBackend for PipeWireBackend {
    fn backend_type(&self) -> AudioBackendType {
        AudioBackendType::PipeWire
    }

    fn enumerate_devices(&self) -> BackendResult<Vec<AudioDevice>> {
        // Primary: native PipeWire via `pw-dump`. Works on PipeWire-only systems
        // that are missing `pipewire-alsa` / `pipewire-pulse` (the Ubuntu
        // empty-list bug) and yields the exact `alsa_output.*` node.name.
        if let Some(devices) = self.enumerate_via_pw_dump() {
            return Ok(devices);
        }
        // Fallback: `pactl` (requires pipewire-pulse + pulseaudio-utils).
        log::info!(
            "[PipeWire Backend] pw-dump unavailable or empty — falling back to pactl enumeration"
        );
        self.enumerate_pipewire_sinks()
    }

    fn create_output_stream(&self, config: &BackendConfig) -> BackendResult<MixerDeviceSink> {
        let target_sink = config.device_id.clone();

        // Temporarily set default sink to target (if specified)
        // We DON'T restore it - let the user's system keep the selected device as default
        // This is actually the expected behavior: when you select a device, it becomes the default
        // When skip_sink_switch is true, skip this entirely to preserve external routing (JACK/qjackctl)
        if config.skip_sink_switch {
            log::info!("[PipeWire Backend] Skipping set-default-sink (skip_sink_switch enabled)");
        } else if let Some(sink_name) = &target_sink {
            log::info!("[PipeWire Backend] Setting default sink to: {}", sink_name);

            let set_result = Command::new("pactl")
                .args(["set-default-sink", sink_name])
                .output();

            match set_result {
                Ok(output) if output.status.success() => {
                    log::info!("[PipeWire Backend] Default sink set to {}", sink_name);
                }
                Ok(output) => {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    log::warn!("[PipeWire Backend] Failed to set default sink: {}", stderr);
                }
                Err(e) => {
                    log::warn!(
                        "[PipeWire Backend] Error executing pactl set-default-sink: {}",
                        e
                    );
                }
            }

            // Wait for PipeWire to process the default sink change
            std::thread::sleep(std::time::Duration::from_millis(200));
        }

        // Check if the DAC supports the requested sample rate.
        // Query via /proc/asound/ (USB DACs list discrete supported rates).
        // If unsupported, fall back to the nearest rate in the same family
        // (e.g., 176.4kHz → 88.2kHz). Rodio resamples from track rate to
        // stream rate automatically.
        let effective_sink = target_sink.clone().or_else(|| {
            Command::new("pactl")
                .args(["get-default-sink"])
                .output()
                .ok()
                .and_then(|o| {
                    if o.status.success() {
                        String::from_utf8(o.stdout)
                            .ok()
                            .map(|s| s.trim().to_string())
                    } else {
                        None
                    }
                })
        });

        let effective_rate = if let Some(ref sink_name) = effective_sink {
            match Self::get_sink_supported_rates(sink_name) {
                Some(rates) if rates.contains(&config.sample_rate) => {
                    log::info!(
                        "[PipeWire Backend] DAC supports {}Hz (available: {:?})",
                        config.sample_rate,
                        rates
                    );
                    config.sample_rate
                }
                Some(rates) => {
                    let fallback = Self::find_best_fallback_rate(config.sample_rate, &rates);
                    log::warn!(
                        "[PipeWire Backend] DAC doesn't support {}Hz. Supported: {:?}. Falling back to {}Hz (resampled by rodio)",
                        config.sample_rate, rates, fallback
                    );
                    fallback
                }
                None => {
                    log::info!(
                        "[PipeWire Backend] Could not determine DAC supported rates, using {}Hz",
                        config.sample_rate
                    );
                    config.sample_rate
                }
            }
        } else {
            config.sample_rate
        };

        // Force PipeWire to use the effective sample rate (for bit-perfect playback).
        //
        // Tier 1 (issue #263): when skip_sink_switch ("Lock output device") is ON, the
        // user has asked QBZ not to mutate GLOBAL graph state to preserve external
        // routing (JACK/qjackctl/qpwgraph). So we skip the global `clock.force-rate`
        // write too — not just `set-default-sink`. This trades device-native-rate
        // playback for routing freedom (PipeWire resamples when track rate != graph
        // rate). Safe: skip_sink_switch is transitively mutually exclusive with
        // dac_passthrough / pw_force_bitperfect (the bit-perfect clock path), so this
        // never collides with a forced-rate bit-perfect session.
        if config.skip_sink_switch {
            log::info!(
                "[PipeWire Backend] Skipping clock.force-rate (skip_sink_switch enabled) — \
                 external routing preserved; PipeWire may resample if graph rate differs"
            );
        } else {
            log::info!(
                "[PipeWire Backend] Forcing sample rate to {}Hz via pw-metadata",
                effective_rate
            );
            let metadata_result = Command::new("pw-metadata")
                .args([
                    "-n",
                    "settings",
                    "0",
                    "clock.force-rate",
                    &effective_rate.to_string(),
                ])
                .output();

            match metadata_result {
                Ok(output) if output.status.success() => {
                    log::info!(
                        "[PipeWire Backend] Sample rate forced to {}Hz",
                        effective_rate
                    );
                }
                Ok(output) => {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    log::warn!("[PipeWire Backend] Failed to force sample rate: {}", stderr);
                }
                Err(e) => {
                    log::warn!("[PipeWire Backend] Error executing pw-metadata: {}", e);
                }
            }
            // Remember WE forced the clock so the stop/suspend reset undoes it
            // (and only it). This runs for every non-locked PipeWire stream, so
            // a plain no-passthrough user no longer leaks a forced rate on stop.
            CLOCK_FORCE_APPLIED.store(true, Ordering::Relaxed);
        }

        // Note: clock.force-quantum is intentionally NOT set.
        // rodio 0.22's MixerDeviceSink has its own internal mixer thread that
        // cannot synchronize with PipeWire's forced quantum, causing massive
        // buffer underruns at sample rates >= 88.2kHz. clock.force-rate alone
        // is sufficient for bit-perfect sample rate switching.

        // Wait for PipeWire to apply the sample rate change.
        // USB hubs (e.g. Razer USB4 Dock) may need longer than direct DACs.
        std::thread::sleep(std::time::Duration::from_millis(500));

        // Create a NEW host (will use current default sink)
        log::info!("[PipeWire Backend] Creating fresh CPAL host...");
        let fresh_host = rodio::cpal::default_host();

        // Find a CPAL device backed by PulseAudio/PipeWire.
        // Newer CPAL description().name() returns friendly labels like
        // "PipeWire Sound Server" instead of raw ids ("pipewire"/"pulse").
        let mut best_device: Option<rodio::cpal::Device> = None;
        let mut best_score: u8 = 0;
        let mut available_output_devices: Vec<String> = Vec::new();

        for device in fresh_host
            .output_devices()
            .map_err(|e| format!("Failed to enumerate CPAL devices: {}", e))?
        {
            let device_name = device
                .description()
                .map(|desc| desc.name().to_string())
                .unwrap_or_else(|_| "unknown".to_string());
            let device_name_lower = device_name.to_ascii_lowercase();
            available_output_devices.push(device_name.clone());

            let score = if device_name_lower == "pipewire" || device_name_lower == "pulse" {
                3
            } else if device_name_lower.contains("pipewire sound server")
                || device_name_lower.contains("pulseaudio sound server")
            {
                2
            } else if device_name_lower.contains("pipewire")
                || device_name_lower.contains("pulseaudio")
            {
                1
            } else {
                0
            };

            if score > best_score {
                best_score = score;
                best_device = Some(device);
            }
        }

        let device = best_device.ok_or_else(|| {
            format!(
                "Could not find 'pulse' or 'pipewire' CPAL device. Is PulseAudio/PipeWire running? Available output devices: {:?}",
                available_output_devices
            )
        })?;

        let device_name = device
            .description()
            .map(|desc| desc.name().to_string())
            .unwrap_or_else(|_| "unknown".to_string());
        log::info!("[PipeWire Backend] Using CPAL device: {}", device_name);

        // Create output stream with custom sample rate configuration
        log::info!(
            "[PipeWire Backend] Creating stream: {}Hz (track: {}Hz), {} channels, exclusive: {}",
            effective_rate,
            config.sample_rate,
            config.channels,
            config.exclusive_mode
        );

        // Create StreamConfig with effective sample rate
        // Note: buffer_size here is unused — with_supported_config() resets it.
        // The actual buffer size is set via with_buffer_size() below.
        let stream_config = StreamConfig {
            channels: config.channels,
            sample_rate: effective_rate,
            buffer_size: BufferSize::Default,
        };

        // Check if CPAL device supports this configuration
        let supported_configs = device
            .supported_output_configs()
            .map_err(|e| format!("Failed to get supported configs: {}", e))?;

        let mut found_matching = false;
        for range in supported_configs {
            if range.channels() == config.channels
                && effective_rate >= range.min_sample_rate()
                && effective_rate <= range.max_sample_rate()
            {
                found_matching = true;
                log::info!(
                    "[PipeWire Backend] CPAL device supports {}Hz (range: {}-{}Hz)",
                    effective_rate,
                    range.min_sample_rate(),
                    range.max_sample_rate()
                );
                break;
            }
        }

        if !found_matching {
            log::warn!(
                "[PipeWire Backend] CPAL device may not support {}Hz, attempting anyway",
                effective_rate
            );
        }

        // Create SupportedStreamConfig
        let supported_config = SupportedStreamConfig::new(
            stream_config.channels,
            stream_config.sample_rate,
            SupportedBufferSize::Range { min: 64, max: 8192 },
            SampleFormat::F32,
        );

        // Compute buffer size — must be applied AFTER with_supported_config()
        // because that method resets buffer_size to Default via ..Default::default().
        // MixerDeviceSink has zero internal buffering, so CPAL's buffer is the
        // ONLY buffer between the mixer and audio hardware.
        let cpal_buffer_size = if config.exclusive_mode {
            BufferSize::Fixed(512) // Low latency for exclusive mode
        } else {
            // ~100ms buffer, matching old vendored cpal period size.
            // Prevents underruns at high sample rates (192kHz = 19200 frames).
            BufferSize::Fixed(effective_rate / 10)
        };
        log::info!("[PipeWire Backend] Buffer size: {:?}", cpal_buffer_size);

        // Tier 2a (#263): in locked mode (skip_sink_switch) QBZ does NOT steal the
        // system default sink, so route THIS stream to the selected sink via the
        // pipewire-ALSA plugin's PIPEWIRE_NODE env — it targets that node WITHOUT
        // changing the system default. The guard restores the prior env value
        // after the PCM open (kept alive until the end of this function).
        #[cfg(target_os = "linux")]
        let _pw_node_guard = if config.skip_sink_switch {
            target_sink.as_ref().map(|sink| {
                let prev = std::env::var("PIPEWIRE_NODE").ok();
                std::env::set_var("PIPEWIRE_NODE", sink);
                log::info!(
                    "[PipeWire Backend] Targeting sink '{}' via PIPEWIRE_NODE (locked mode, default unchanged)",
                    sink
                );
                PwNodeEnvGuard(prev)
            })
        } else {
            None
        };

        // Create MixerDeviceSink with custom config
        let mixer_sink = DeviceSinkBuilder::from_device(device)
            .map_err(|e| format!("Failed to create device sink builder: {}", e))?
            .with_supported_config(&supported_config)
            .with_buffer_size(cpal_buffer_size)
            .open_stream()
            .map_err(|e| {
                format!(
                    "Failed to create output stream at {}Hz: {}",
                    effective_rate, e
                )
            })?;

        log::info!(
            "[PipeWire Backend] Output stream created successfully at {}Hz",
            effective_rate
        );

        // Re-apply clock.force-rate AFTER stream creation.
        // When resuming after PipeWire dropped the stream during pause,
        // the graph may have reverted to the DAC's default rate (e.g. 44100).
        // The pre-stream force-rate can be ignored if no streams were active.
        // Re-applying now that the stream exists forces PipeWire to reconfigure
        // the graph at the correct rate.
        //
        // Tier 1 (issue #263): also gated by skip_sink_switch — in "Lock output
        // device" mode QBZ never writes the global clock force (pre-stream OR
        // re-apply), so the user's external routing/graph clock is left untouched.
        if !config.skip_sink_switch && effective_rate != 44100 && effective_rate != 48000 {
            let _ = Command::new("pw-metadata")
                .args([
                    "-n",
                    "settings",
                    "0",
                    "clock.force-rate",
                    &effective_rate.to_string(),
                ])
                .output();
            log::info!(
                "[PipeWire Backend] Re-applied clock.force-rate={}Hz after stream creation",
                effective_rate
            );

            // Verify PipeWire actually applied the rate.
            // USB hubs/docks may need extra time for rate switching.
            std::thread::sleep(std::time::Duration::from_millis(100));
            if let Some(actual_rate) = Self::get_pipewire_current_rate() {
                if actual_rate != effective_rate {
                    log::warn!(
                        "[PipeWire Backend] Rate mismatch: requested {}Hz but PipeWire reports {}Hz. \
                         Retrying with longer delay...",
                        effective_rate,
                        actual_rate
                    );
                    // Give slower USB devices more time, then force again
                    std::thread::sleep(std::time::Duration::from_millis(500));
                    let _ = Command::new("pw-metadata")
                        .args([
                            "-n",
                            "settings",
                            "0",
                            "clock.force-rate",
                            &effective_rate.to_string(),
                        ])
                        .output();
                    std::thread::sleep(std::time::Duration::from_millis(200));

                    if let Some(retry_rate) = Self::get_pipewire_current_rate() {
                        if retry_rate == effective_rate {
                            log::info!(
                                "[PipeWire Backend] Rate verified after retry: {}Hz",
                                retry_rate
                            );
                        } else {
                            log::warn!(
                                "[PipeWire Backend] Rate still {}Hz after retry (expected {}Hz). \
                                 Audio may play at wrong speed.",
                                retry_rate,
                                effective_rate
                            );
                        }
                    }
                } else {
                    log::info!("[PipeWire Backend] Rate verified: {}Hz", actual_rate);
                }
            }
        }

        Ok(mixer_sink)
    }

    fn is_available(&self) -> bool {
        // #591/#592: this probe used to require `pactl` (pulseaudio-utils +
        // pipewire-pulse). Sandboxed and minimal installs (Snap, Flatpak, .deb
        // without pulseaudio-utils) can lack it even though PipeWire itself is
        // up — device enumeration already succeeds natively via `pw-dump` on
        // those systems, so the availability probe must accept the same proof.
        // Order: native socket (no subprocess) → pw-dump → pactl (kept as the
        // fallback for PulseAudio-only systems, preserving pre-2.0 behavior).
        // Subprocess probes are time-bounded so a wedged daemon can never
        // stall stream init on the audio thread.
        if let Some(runtime_dir) = std::env::var_os("XDG_RUNTIME_DIR") {
            if std::path::Path::new(&runtime_dir)
                .join("pipewire-0")
                .exists()
            {
                return true;
            }
        }
        probe_command_ok("pw-dump", &[], std::time::Duration::from_secs(3))
            || probe_command_ok("pactl", &["info"], std::time::Duration::from_secs(3))
    }

    fn description(&self) -> &'static str {
        "PipeWire (Recommended) - Modern audio server with device sharing"
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[cfg(test)]
mod pw_dump_tests {
    use super::parse_pw_dump_sinks;

    // Minimal fixture mirroring the real `pw-dump` shape (Cambridge USB DAC +
    // internal PCI card + a capture source that must be filtered out).
    const FIXTURE: &str = r#"[
      {
        "id": 30,
        "type": "PipeWire:Interface:Metadata",
        "props": { "metadata.name": "default" },
        "metadata": [
          { "subject": 0, "key": "default.audio.sink", "type": "Spa:String:JSON",
            "value": { "name": "alsa_output.usb-Cambridge_Audio-00.analog-stereo" } }
        ]
      },
      {
        "id": 52, "type": "PipeWire:Interface:Device",
        "info": { "props": { "device.bus": "usb", "device.api": "alsa" } }
      },
      {
        "id": 53, "type": "PipeWire:Interface:Node",
        "info": { "props": {
          "media.class": "Audio/Sink",
          "node.name": "alsa_output.usb-Cambridge_Audio-00.analog-stereo",
          "node.description": "DacMagic Plus Analog Stereo",
          "device.id": 52, "device.bus": "usb", "device.api": "alsa",
          "factory.name": "api.alsa.pcm.sink"
        } }
      },
      {
        "id": 60, "type": "PipeWire:Interface:Device",
        "info": { "props": { "device.bus": "pci", "device.api": "alsa" } }
      },
      {
        "id": 61, "type": "PipeWire:Interface:Node",
        "info": { "props": {
          "media.class": "Audio/Sink",
          "node.name": "alsa_output.pci-0000_00_1f.3.analog-stereo",
          "node.description": "Built-in Audio Analog Stereo",
          "device.id": 60, "device.api": "alsa",
          "factory.name": "api.alsa.pcm.sink"
        } }
      },
      {
        "id": 70, "type": "PipeWire:Interface:Node",
        "info": { "props": {
          "media.class": "Audio/Source",
          "node.name": "alsa_input.usb-Cambridge_Audio-00.analog-stereo"
        } }
      }
    ]"#;

    #[test]
    fn parses_sinks_only_with_node_names_bus_and_default() {
        let devs = parse_pw_dump_sinks(FIXTURE);
        // The Audio/Source must be excluded.
        assert_eq!(
            devs.len(),
            2,
            "should parse exactly the two Audio/Sink nodes"
        );

        let usb = devs
            .iter()
            .find(|d| d.id == "alsa_output.usb-Cambridge_Audio-00.analog-stereo")
            .expect("usb sink present");
        assert_eq!(usb.name, "DacMagic Plus Analog Stereo");
        assert_eq!(usb.device_bus.as_deref(), Some("usb")); // read from node props
        assert!(usb.is_hardware);
        assert!(usb.is_default, "usb sink is the default per Metadata");
        assert!(
            usb.max_sample_rate.is_none(),
            "rate comes from the capability probe, not pw-dump"
        );

        let pci = devs
            .iter()
            .find(|d| d.id == "alsa_output.pci-0000_00_1f.3.analog-stereo")
            .expect("pci sink present");
        // Bus absent on the node -> cross-referenced via device.id 60.
        assert_eq!(pci.device_bus.as_deref(), Some("pci"));
        assert!(pci.is_hardware);
        assert!(!pci.is_default);
    }

    #[test]
    fn empty_or_garbage_json_yields_no_devices() {
        assert!(parse_pw_dump_sinks("not json").is_empty());
        assert!(parse_pw_dump_sinks("[]").is_empty());
        assert!(parse_pw_dump_sinks("{}").is_empty());
    }
}
