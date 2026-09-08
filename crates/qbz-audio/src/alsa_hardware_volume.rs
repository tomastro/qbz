//! ALSA hardware-volume discovery, identity, selection and control.
//!
//! This module is intentionally separate from `alsa_direct`: mixer probing is
//! control-plane work and must not change PCM open/configure/write/drain or any
//! sample-rate decision in the protected direct-output path.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::Arc;

/// The complete ALSA simple-mixer identity. Names alone are not unique.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AlsaMixerControlId {
    pub name: String,
    pub index: u32,
}

impl fmt::Display for AlsaMixerControlId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{},{}", self.name, self.index)
    }
}

/// Opaque, serializable key for one physical card and one PCM route.
///
/// The string form is deliberate: JSON maps only support string keys. Values
/// are versioned and contain a stable card identity, the PCM kind and `DEV=`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct StableAlsaRouteKey(String);

impl StableAlsaRouteKey {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn from_parts(identity: &StableAlsaCardIdentity, pcm: &str, device: u32) -> Self {
        let (kind, value) = match identity {
            StableAlsaCardIdentity::ById(value) => ("by-id", value.as_str()),
            StableAlsaCardIdentity::ByPath(value) => ("by-path", value.as_str()),
            StableAlsaCardIdentity::Card(value) => ("card", value.as_str()),
        };
        Self(format!(
            "alsa-route-v1|card={kind}:{}|pcm={}|DEV={device}",
            escape_key_component(value),
            escape_key_component(pcm)
        ))
    }
}

impl fmt::Display for StableAlsaRouteKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum StableAlsaCardIdentity {
    ById(String),
    ByPath(String),
    Card(String),
}

fn select_stable_card_identity(
    has_real_serial: bool,
    by_id: Option<String>,
    by_path: Option<String>,
    short_id: String,
) -> StableAlsaCardIdentity {
    if has_real_serial {
        if let Some(by_id) = by_id {
            return StableAlsaCardIdentity::ById(by_id);
        }
    }
    by_path
        .map(StableAlsaCardIdentity::ByPath)
        .unwrap_or(StableAlsaCardIdentity::Card(short_id))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct HardwareVolumeRange {
    pub min: i64,
    pub max: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HardwareVolumeChannelValue {
    pub channel: String,
    pub raw: i64,
    pub db_millibels: Option<i64>,
    pub playback_switch: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HardwareVolumeWritability {
    Verified,
    NeedsEnableCheck,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "detail", rename_all = "snake_case")]
pub enum HardwareVolumeRejectionReason {
    Inactive,
    CaptureOnly,
    UnsafeInputPath,
    CommonPlaybackCaptureVolume,
    NoPlaybackVolume,
    InvalidRange { min: i64, max: i64 },
    NoPlaybackChannels,
    ReadFailed(String),
    ReadOnly,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HardwareVolumeCandidate {
    pub id: AlsaMixerControlId,
    pub label: String,
    pub channels: Vec<String>,
    pub raw_range: Option<HardwareVolumeRange>,
    /// ALSA millibels (1/100 dB). `None` means raw-linear fallback.
    pub db_range: Option<HardwareVolumeRange>,
    pub current_values: Vec<HardwareVolumeChannelValue>,
    pub current_volume: Option<f32>,
    pub has_playback_switch: bool,
    pub recommended: bool,
    pub writability: HardwareVolumeWritability,
    pub rejection_reason: Option<HardwareVolumeRejectionReason>,
}

impl HardwareVolumeCandidate {
    pub fn is_valid(&self) -> bool {
        self.rejection_reason.is_none()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HardwareVolumeProbeErrorKind {
    InvalidDevice,
    PermissionDenied,
    DeviceBusy,
    DeviceUnavailable,
    NoMixer,
    UnsupportedPlatform,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HardwareVolumeProbeError {
    pub kind: HardwareVolumeProbeErrorKind,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HardwareVolumeProbe {
    pub device_id: String,
    pub ctl_name: String,
    pub route_key: Option<StableAlsaRouteKey>,
    /// Every simple element, including rejected controls for diagnostics.
    pub candidates: Vec<HardwareVolumeCandidate>,
    /// Read-only UCM hints that resolve to simple-mixer identities.
    pub ucm_controls: Vec<AlsaMixerControlId>,
    pub error: Option<HardwareVolumeProbeError>,
}

impl HardwareVolumeProbe {
    pub fn valid_candidates(&self) -> impl Iterator<Item = &HardwareVolumeCandidate> {
        self.candidates
            .iter()
            .filter(|candidate| candidate.is_valid())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HardwareVolumeSelectionSource {
    Persisted,
    Ucm,
    OnlyCandidate,
    UserChoice,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HardwareVolumeSelection {
    pub route_key: StableAlsaRouteKey,
    pub control: AlsaMixerControlId,
    pub candidate: HardwareVolumeCandidate,
    pub source: HardwareVolumeSelectionSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HardwareVolumeChoiceReason {
    Ambiguous,
    UcmAmbiguous,
    PersistedSelectionStale,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "detail", rename_all = "snake_case")]
pub enum HardwareVolumeUnsupportedReason {
    Probe(HardwareVolumeProbeError),
    NoValidControls,
    PersistedSelectionStale(AlsaMixerControlId),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum HardwareVolumeDecision {
    Selected {
        selection: HardwareVolumeSelection,
    },
    NeedsChoice {
        route_key: StableAlsaRouteKey,
        candidates: Vec<HardwareVolumeCandidate>,
        reason: HardwareVolumeChoiceReason,
    },
    Unsupported {
        route_key: Option<StableAlsaRouteKey>,
        reason: HardwareVolumeUnsupportedReason,
    },
}

/// A fresh physical-mixer snapshot after an exact read or write.
#[derive(Debug, Clone, PartialEq)]
pub struct HardwareVolumeSnapshot {
    pub control: AlsaMixerControlId,
    pub channels: Vec<HardwareVolumeChannelValue>,
    pub volume: f32,
    pub muted: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum HardwareVolumeEvent {
    Changed(HardwareVolumeSnapshot),
    Unavailable(String),
}

pub type HardwareVolumeEventCallback = Arc<dyn Fn(HardwareVolumeEvent) + Send + Sync + 'static>;

/// Owns the ctl event thread. Dropping it stops and joins the subscription.
pub struct HardwareVolumeEventSubscription {
    #[cfg(target_os = "linux")]
    stop: Arc<std::sync::atomic::AtomicBool>,
    #[cfg(target_os = "linux")]
    thread: Option<std::thread::JoinHandle<()>>,
}

#[cfg(target_os = "linux")]
impl Drop for HardwareVolumeEventSubscription {
    fn drop(&mut self) {
        use std::sync::atomic::Ordering;
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Decide without touching hardware. A stale persisted identity is a hard
/// stop: it must never silently fall through to a different control.
pub fn decide_hardware_volume(
    probe: &HardwareVolumeProbe,
    persisted: &HashMap<StableAlsaRouteKey, AlsaMixerControlId>,
) -> HardwareVolumeDecision {
    if let Some(error) = &probe.error {
        return HardwareVolumeDecision::Unsupported {
            route_key: probe.route_key.clone(),
            reason: HardwareVolumeUnsupportedReason::Probe(error.clone()),
        };
    }

    let Some(route_key) = probe.route_key.clone() else {
        return HardwareVolumeDecision::Unsupported {
            route_key: None,
            reason: HardwareVolumeUnsupportedReason::Probe(HardwareVolumeProbeError {
                kind: HardwareVolumeProbeErrorKind::InvalidDevice,
                message: "ALSA route has no stable identity".to_string(),
            }),
        };
    };

    let valid = probe
        .valid_candidates()
        .cloned()
        .collect::<Vec<HardwareVolumeCandidate>>();

    if let Some(saved) = persisted.get(&route_key) {
        if let Some(candidate) = valid.iter().find(|candidate| candidate.id == *saved) {
            return HardwareVolumeDecision::Selected {
                selection: HardwareVolumeSelection {
                    route_key,
                    control: saved.clone(),
                    candidate: candidate.clone(),
                    source: HardwareVolumeSelectionSource::Persisted,
                },
            };
        }
        if valid.is_empty() {
            return HardwareVolumeDecision::Unsupported {
                route_key: Some(route_key),
                reason: HardwareVolumeUnsupportedReason::PersistedSelectionStale(saved.clone()),
            };
        }
        return HardwareVolumeDecision::NeedsChoice {
            route_key,
            candidates: valid,
            reason: HardwareVolumeChoiceReason::PersistedSelectionStale,
        };
    }

    let ucm_ids = probe.ucm_controls.iter().cloned().collect::<HashSet<_>>();
    let ucm_matches = valid
        .iter()
        .filter(|candidate| ucm_ids.contains(&candidate.id))
        .cloned()
        .collect::<Vec<_>>();
    if ucm_matches.len() == 1 {
        let candidate = ucm_matches.into_iter().next().expect("length checked");
        return HardwareVolumeDecision::Selected {
            selection: HardwareVolumeSelection {
                route_key,
                control: candidate.id.clone(),
                candidate,
                source: HardwareVolumeSelectionSource::Ucm,
            },
        };
    }
    if ucm_matches.len() > 1 {
        return HardwareVolumeDecision::NeedsChoice {
            route_key,
            candidates: valid,
            reason: HardwareVolumeChoiceReason::UcmAmbiguous,
        };
    }

    match valid.len() {
        0 => HardwareVolumeDecision::Unsupported {
            route_key: Some(route_key),
            reason: HardwareVolumeUnsupportedReason::NoValidControls,
        },
        1 => {
            let candidate = valid.into_iter().next().expect("length checked");
            HardwareVolumeDecision::Selected {
                selection: HardwareVolumeSelection {
                    route_key,
                    control: candidate.id.clone(),
                    candidate,
                    source: HardwareVolumeSelectionSource::OnlyCandidate,
                },
            }
        }
        _ => HardwareVolumeDecision::NeedsChoice {
            route_key,
            candidates: valid,
            reason: HardwareVolumeChoiceReason::Ambiguous,
        },
    }
}

pub fn resolve_hardware_volume(
    device_id: &str,
    persisted: &HashMap<StableAlsaRouteKey, AlsaMixerControlId>,
) -> HardwareVolumeDecision {
    decide_hardware_volume(&enumerate_hardware_volume_controls(device_id), persisted)
}

fn escape_key_component(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.') {
            escaped.push(char::from(byte));
        } else {
            use std::fmt::Write;
            let _ = write!(escaped, "%{byte:02X}");
        }
    }
    escaped
}

fn hardware_volume_rank(name: &str) -> u8 {
    let lower = name.to_ascii_lowercase();
    [
        ("master", 100),
        ("pcm", 90),
        ("speaker", 80),
        ("headphone", 70),
        ("digital", 60),
        ("dac", 50),
        ("line out", 40),
        ("playback", 30),
    ]
    .into_iter()
    .find_map(|(token, score)| lower.contains(token).then_some(score))
    .unwrap_or(1)
}

fn unsafe_input_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    ["capture", "mic", "boost", "sidetone", "loopback"]
        .iter()
        .any(|token| lower.contains(token))
}

#[derive(Debug, Clone, Copy)]
struct CandidateFacts<'a> {
    name: &'a str,
    active: bool,
    has_playback: bool,
    has_capture: bool,
    has_common_volume: bool,
    raw_range: Option<HardwareVolumeRange>,
    playback_channels: usize,
}

/// Pure structural validation, shared by the live ALSA probe and unit tests.
/// Read failures and ctl writability are checked immediately after this gate.
fn structural_rejection(facts: CandidateFacts<'_>) -> Option<HardwareVolumeRejectionReason> {
    if !facts.active {
        return Some(HardwareVolumeRejectionReason::Inactive);
    }
    if facts.has_common_volume {
        return Some(HardwareVolumeRejectionReason::CommonPlaybackCaptureVolume);
    }
    if facts.has_capture && !facts.has_playback {
        return Some(HardwareVolumeRejectionReason::CaptureOnly);
    }
    if unsafe_input_name(facts.name) {
        return Some(HardwareVolumeRejectionReason::UnsafeInputPath);
    }
    if !facts.has_playback {
        return Some(HardwareVolumeRejectionReason::NoPlaybackVolume);
    }
    let range = facts
        .raw_range
        .unwrap_or(HardwareVolumeRange { min: 0, max: 0 });
    if range.max <= range.min {
        return Some(HardwareVolumeRejectionReason::InvalidRange {
            min: range.min,
            max: range.max,
        });
    }
    if facts.playback_channels == 0 {
        return Some(HardwareVolumeRejectionReason::NoPlaybackChannels);
    }
    None
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct HardwareVolumeWritePlan {
    playback_switch: Option<bool>,
    write_level: bool,
}

fn hardware_volume_write_plan(scalar: f32, has_playback_switch: bool) -> HardwareVolumeWritePlan {
    if scalar <= f32::EPSILON && has_playback_switch {
        HardwareVolumeWritePlan {
            playback_switch: Some(false),
            write_level: false,
        }
    } else {
        HardwareVolumeWritePlan {
            playback_switch: has_playback_switch.then_some(true),
            write_level: true,
        }
    }
}

fn suppress_local_write_echo(written: f32, age_millis: u128, observed: f32) -> bool {
    age_millis <= 300 && (written - observed).abs() < 0.001
}

fn writability_rejection(writable: Option<bool>) -> Option<HardwareVolumeRejectionReason> {
    (writable == Some(false)).then_some(HardwareVolumeRejectionReason::ReadOnly)
}

fn mark_recommended(candidates: &mut [HardwareVolumeCandidate]) {
    let best = candidates
        .iter()
        .filter(|candidate| candidate.is_valid())
        .map(|candidate| hardware_volume_rank(&candidate.id.name))
        .max();
    for candidate in candidates {
        candidate.recommended =
            candidate.is_valid() && best == Some(hardware_volume_rank(&candidate.id.name));
    }
}

fn normalize_level(
    raw_range: HardwareVolumeRange,
    db_range: Option<HardwareVolumeRange>,
    values: &[HardwareVolumeChannelValue],
) -> f32 {
    let switches = values
        .iter()
        .filter_map(|value| value.playback_switch)
        .collect::<Vec<_>>();
    if !switches.is_empty() && switches.iter().all(|enabled| !enabled) {
        return 0.0;
    }

    if let Some(db_range) = db_range {
        let levels = values
            .iter()
            .map(|value| value.db_millibels)
            .collect::<Option<Vec<_>>>();
        if let Some(levels) = levels.filter(|levels| !levels.is_empty()) {
            return levels
                .into_iter()
                .map(|value| normalized(value, db_range))
                .fold(0.0_f32, f32::max);
        }
    }

    values
        .iter()
        .map(|value| normalized(value.raw, raw_range))
        .fold(0.0_f32, f32::max)
}

fn normalized(value: i64, range: HardwareVolumeRange) -> f32 {
    if range.max <= range.min {
        return 0.0;
    }
    ((value - range.min) as f32 / (range.max - range.min) as f32).clamp(0.0, 1.0)
}

fn channel_targets(values: &[i64], range: HardwareVolumeRange, scalar: f32) -> Vec<i64> {
    if values.is_empty() || range.max <= range.min {
        return Vec::new();
    }
    let current_peak = values.iter().copied().max().unwrap_or(range.min);
    let target_peak =
        range.min + (((range.max - range.min) as f32 * scalar.clamp(0.0, 1.0)).round() as i64);
    values
        .iter()
        .map(|value| (target_peak + (*value - current_peak)).clamp(range.min, range.max))
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedAlsaRoute {
    card: String,
    pcm: String,
    device: u32,
}

fn parse_alsa_route(device_id: &str) -> Option<ParsedAlsaRoute> {
    let (prefix, arguments) = device_id.split_once(':')?;
    let pcm = match prefix {
        "hw" | "plughw" => "hw",
        "front" => "front",
        "iec958" => "iec958",
        "hdmi" => "hdmi",
        "sysdefault" => "sysdefault",
        other => other,
    }
    .to_string();

    let mut card = None;
    let mut device = None;
    for (position, argument) in arguments.split(',').enumerate() {
        let argument = argument.trim();
        if let Some(value) = argument.strip_prefix("CARD=") {
            card = Some(value.to_string());
        } else if let Some(value) = argument.strip_prefix("DEV=") {
            device = value.parse::<u32>().ok();
        } else if position == 0 && !argument.is_empty() {
            card = Some(argument.to_string());
        } else if position == 1 {
            device = argument.parse::<u32>().ok();
        }
    }

    Some(ParsedAlsaRoute {
        card: card?,
        pcm,
        device: device.unwrap_or(0),
    })
}

#[cfg(target_os = "linux")]
fn ucm_parser_identifier(value_name: &str) -> Option<&'static str> {
    match value_name {
        "PlaybackVolume" => Some("PlaybackVolume"),
        "PlaybackMasterElem" | "PlaybackMixerElem" => Some("PlaybackMixerId"),
        _ => None,
    }
}

/// Resolve a PCM id to the card ctl name accepted by ALSA mixer/control APIs.
pub fn mixer_ctl_name(device_id: &str) -> String {
    let Some(route) = parse_alsa_route(device_id) else {
        return device_id.to_string();
    };
    if route
        .card
        .chars()
        .all(|character| character.is_ascii_digit())
    {
        format!("hw:{}", route.card)
    } else {
        format!("hw:CARD={}", route.card)
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use alsa::ctl::Ctl;
    use alsa_sys as ffi;
    use std::ffi::{CStr, CString};
    use std::fs;
    use std::os::unix::fs::MetadataExt;
    use std::path::{Path, PathBuf};
    use std::ptr;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};

    const DB_GAIN_MUTE: i64 = -9_999_999;
    const CHANNELS: &[(ffi::snd_mixer_selem_channel_id_t, &str)] = &[
        (ffi::SND_MIXER_SCHN_FRONT_LEFT, "Front Left"),
        (ffi::SND_MIXER_SCHN_FRONT_RIGHT, "Front Right"),
        (ffi::SND_MIXER_SCHN_REAR_LEFT, "Rear Left"),
        (ffi::SND_MIXER_SCHN_REAR_RIGHT, "Rear Right"),
        (ffi::SND_MIXER_SCHN_FRONT_CENTER, "Front Center"),
        (ffi::SND_MIXER_SCHN_WOOFER, "Woofer"),
        (ffi::SND_MIXER_SCHN_SIDE_LEFT, "Side Left"),
        (ffi::SND_MIXER_SCHN_SIDE_RIGHT, "Side Right"),
        (ffi::SND_MIXER_SCHN_REAR_CENTER, "Rear Center"),
    ];

    #[cfg(target_pointer_width = "64")]
    fn c_long_to_i64(value: libc::c_long) -> i64 {
        value
    }

    #[cfg(not(target_pointer_width = "64"))]
    fn c_long_to_i64(value: libc::c_long) -> i64 {
        i64::from(value)
    }

    #[repr(C)]
    struct SndUseCaseMgr {
        _private: [u8; 0],
    }

    #[link(name = "asound")]
    unsafe extern "C" {
        fn snd_use_case_mgr_open(
            manager: *mut *mut SndUseCaseMgr,
            card_name: *const libc::c_char,
        ) -> libc::c_int;
        fn snd_use_case_mgr_close(manager: *mut SndUseCaseMgr) -> libc::c_int;
        fn snd_use_case_get(
            manager: *mut SndUseCaseMgr,
            identifier: *const libc::c_char,
            value: *mut *const libc::c_char,
        ) -> libc::c_int;
        fn snd_use_case_get_list(
            manager: *mut SndUseCaseMgr,
            identifier: *const libc::c_char,
            list: *mut *mut *const libc::c_char,
        ) -> libc::c_int;
        fn snd_use_case_free_list(
            list: *const *const libc::c_char,
            items: libc::c_int,
        ) -> libc::c_int;
        fn snd_use_case_parse_selem_id(
            destination: *mut ffi::snd_mixer_selem_id_t,
            ucm_id: *const libc::c_char,
            value: *const libc::c_char,
        ) -> libc::c_int;
        fn snd_use_case_parse_ctl_elem_id(
            destination: *mut ffi::snd_ctl_elem_id_t,
            ucm_id: *const libc::c_char,
            value: *const libc::c_char,
        ) -> libc::c_int;
    }

    struct UcmManager(*mut SndUseCaseMgr);

    impl UcmManager {
        fn open(card: &str) -> Option<Self> {
            let card = CString::new(format!("hw:{card}")).ok()?;
            let mut manager = ptr::null_mut();
            let result = unsafe { snd_use_case_mgr_open(&mut manager, card.as_ptr()) };
            // Do not express this with bool::then_some(Self(manager)):
            // then_some eagerly constructs and drops the owner on failure,
            // calling snd_use_case_mgr_close on an invalid pointer.
            if result < 0 || manager.is_null() {
                return None;
            }
            Some(Self(manager))
        }

        fn get(&self, identifier: &str) -> Option<String> {
            let identifier = CString::new(identifier).ok()?;
            let mut value = ptr::null();
            if unsafe { snd_use_case_get(self.0, identifier.as_ptr(), &mut value) } < 0
                || value.is_null()
            {
                return None;
            }
            let result = unsafe { CStr::from_ptr(value) }
                .to_string_lossy()
                .into_owned();
            unsafe {
                libc::free(value.cast_mut().cast());
            }
            (!result.is_empty()).then_some(result)
        }

        fn list(&self, identifier: &str) -> Vec<String> {
            let Ok(identifier) = CString::new(identifier) else {
                return Vec::new();
            };
            let mut list = ptr::null_mut();
            let count = unsafe { snd_use_case_get_list(self.0, identifier.as_ptr(), &mut list) };
            if count <= 0 || list.is_null() {
                return Vec::new();
            }
            let values = (0..count)
                .filter_map(|index| {
                    let value = unsafe { *list.add(index as usize) };
                    (!value.is_null()).then(|| {
                        unsafe { CStr::from_ptr(value) }
                            .to_string_lossy()
                            .into_owned()
                    })
                })
                .collect();
            unsafe {
                snd_use_case_free_list(list.cast_const(), count);
            }
            values
        }
    }

    impl Drop for UcmManager {
        fn drop(&mut self) {
            unsafe {
                snd_use_case_mgr_close(self.0);
            }
        }
    }

    struct RawMixer(*mut ffi::snd_mixer_t);

    impl RawMixer {
        fn open(device_id: &str) -> Result<Self, HardwareVolumeProbeError> {
            let ctl_name = mixer_ctl_name(device_id);
            let ctl = CString::new(ctl_name.as_str()).map_err(|_| HardwareVolumeProbeError {
                kind: HardwareVolumeProbeErrorKind::InvalidDevice,
                message: format!("Invalid ALSA ctl name for {device_id}"),
            })?;
            let mut mixer = ptr::null_mut();
            alsa_call(
                unsafe { ffi::snd_mixer_open(&mut mixer, 0) },
                "open ALSA mixer",
            )?;
            let owner = Self(mixer);
            alsa_call(
                unsafe { ffi::snd_mixer_attach(owner.0, ctl.as_ptr()) },
                &format!("attach ALSA mixer to {ctl_name}"),
            )?;
            alsa_call(
                unsafe { ffi::snd_mixer_selem_register(owner.0, ptr::null_mut(), ptr::null_mut()) },
                "register ALSA simple mixer elements",
            )?;
            alsa_call(unsafe { ffi::snd_mixer_load(owner.0) }, "load ALSA mixer")?;
            Ok(owner)
        }

        fn find(&self, id: &AlsaMixerControlId) -> Option<*mut ffi::snd_mixer_elem_t> {
            let mut element = unsafe { ffi::snd_mixer_first_elem(self.0) };
            while !element.is_null() {
                if unsafe { ffi::snd_mixer_elem_get_type(element) } == ffi::SND_MIXER_ELEM_SIMPLE {
                    let candidate = selem_id(element);
                    if candidate.as_ref() == Some(id) {
                        return Some(element);
                    }
                }
                element = unsafe { ffi::snd_mixer_elem_next(element) };
            }
            None
        }
    }

    impl Drop for RawMixer {
        fn drop(&mut self) {
            unsafe {
                ffi::snd_mixer_close(self.0);
            }
        }
    }

    struct RawCtl(*mut ffi::snd_ctl_t);

    impl RawCtl {
        fn open(ctl_name: &str, nonblock: bool) -> Result<Self, HardwareVolumeProbeError> {
            let name = CString::new(ctl_name).map_err(|_| HardwareVolumeProbeError {
                kind: HardwareVolumeProbeErrorKind::InvalidDevice,
                message: format!("Invalid ALSA ctl name: {ctl_name}"),
            })?;
            let mut ctl = ptr::null_mut();
            alsa_call(
                unsafe { ffi::snd_ctl_open(&mut ctl, name.as_ptr(), i32::from(nonblock)) },
                &format!("open ALSA ctl {ctl_name}"),
            )?;
            Ok(Self(ctl))
        }

        /// `None` means the Selem -> ctl translation was not reliable enough;
        /// the explicit enable path will do a per-channel current-value writeback.
        fn writable(&self, id: &AlsaMixerControlId) -> Option<bool> {
            let raw_names = if id.name.ends_with(" Playback Volume") {
                vec![id.name.clone()]
            } else {
                vec![format!("{} Playback Volume", id.name), id.name.clone()]
            };
            for raw_name in raw_names {
                let Ok(raw_name) = CString::new(raw_name) else {
                    continue;
                };
                let mut info = ptr::null_mut();
                if unsafe { ffi::snd_ctl_elem_info_malloc(&mut info) } < 0 {
                    return None;
                }
                unsafe {
                    ffi::snd_ctl_elem_info_set_interface(info, ffi::SND_CTL_ELEM_IFACE_MIXER);
                    ffi::snd_ctl_elem_info_set_name(info, raw_name.as_ptr());
                    ffi::snd_ctl_elem_info_set_index(info, id.index);
                }
                let result = unsafe { ffi::snd_ctl_elem_info(self.0, info) };
                let writable =
                    (result >= 0).then(|| unsafe { ffi::snd_ctl_elem_info_is_writable(info) > 0 });
                unsafe {
                    ffi::snd_ctl_elem_info_free(info);
                }
                if writable.is_some() {
                    return writable;
                }
            }
            None
        }
    }

    impl Drop for RawCtl {
        fn drop(&mut self) {
            unsafe {
                ffi::snd_ctl_close(self.0);
            }
        }
    }

    fn alsa_call(code: i32, operation: &str) -> Result<(), HardwareVolumeProbeError> {
        if code >= 0 {
            return Ok(());
        }
        let errno = -code;
        let detail = unsafe {
            let message = ffi::snd_strerror(code);
            if message.is_null() {
                format!("ALSA error {code}")
            } else {
                CStr::from_ptr(message).to_string_lossy().into_owned()
            }
        };
        let kind = match errno {
            libc::EACCES | libc::EPERM => HardwareVolumeProbeErrorKind::PermissionDenied,
            libc::EBUSY => HardwareVolumeProbeErrorKind::DeviceBusy,
            libc::ENODEV | libc::ENXIO => HardwareVolumeProbeErrorKind::DeviceUnavailable,
            libc::ENOENT => HardwareVolumeProbeErrorKind::NoMixer,
            _ => HardwareVolumeProbeErrorKind::Other,
        };
        Err(HardwareVolumeProbeError {
            kind,
            message: format!("Failed to {operation}: {detail}"),
        })
    }

    fn selem_id(element: *mut ffi::snd_mixer_elem_t) -> Option<AlsaMixerControlId> {
        let name = unsafe { ffi::snd_mixer_selem_get_name(element) };
        if name.is_null() {
            return None;
        }
        Some(AlsaMixerControlId {
            name: unsafe { CStr::from_ptr(name) }
                .to_string_lossy()
                .into_owned(),
            index: unsafe { ffi::snd_mixer_selem_get_index(element) },
        })
    }

    fn playback_channels(element: *mut ffi::snd_mixer_elem_t) -> Vec<(i32, String)> {
        CHANNELS
            .iter()
            .filter(|(channel, _)| unsafe {
                ffi::snd_mixer_selem_has_playback_channel(element, *channel) > 0
            })
            .map(|(channel, name)| (*channel, (*name).to_string()))
            .collect()
    }

    fn finite_db_range(
        element: *mut ffi::snd_mixer_elem_t,
        raw_range: HardwareVolumeRange,
    ) -> Option<HardwareVolumeRange> {
        let mut min = 0;
        let mut max = 0;
        if unsafe { ffi::snd_mixer_selem_get_playback_dB_range(element, &mut min, &mut max) } < 0
            || max <= min
        {
            return None;
        }
        let mut min = c_long_to_i64(min);
        let max = c_long_to_i64(max);
        if min <= DB_GAIN_MUTE {
            if raw_range.max <= raw_range.min {
                return None;
            }
            let mut finite = 0;
            if unsafe {
                ffi::snd_mixer_selem_ask_playback_vol_dB(
                    element,
                    (raw_range.min + 1) as _,
                    &mut finite,
                )
            } < 0
                || c_long_to_i64(finite) <= DB_GAIN_MUTE
            {
                return None;
            }
            min = c_long_to_i64(finite);
        }
        (max > min).then_some(HardwareVolumeRange { min, max })
    }

    fn read_channels(
        element: *mut ffi::snd_mixer_elem_t,
        channels: &[(i32, String)],
        db_range: Option<HardwareVolumeRange>,
        has_switch: bool,
    ) -> Result<Vec<HardwareVolumeChannelValue>, String> {
        let mut values = Vec::with_capacity(channels.len());
        for (channel, name) in channels {
            let mut raw = 0;
            let code =
                unsafe { ffi::snd_mixer_selem_get_playback_volume(element, *channel, &mut raw) };
            if code < 0 {
                return Err(format!("{}: {}", name, alsa_error_text(code)));
            }
            let db_millibels = db_range.and_then(|_| {
                let mut value = 0;
                (unsafe { ffi::snd_mixer_selem_get_playback_dB(element, *channel, &mut value) }
                    >= 0)
                    .then_some(c_long_to_i64(value))
            });
            let playback_switch = has_switch.then(|| {
                let mut enabled = 0;
                let code = unsafe {
                    ffi::snd_mixer_selem_get_playback_switch(element, *channel, &mut enabled)
                };
                (code >= 0).then_some(enabled != 0)
            });
            if has_switch && playback_switch.flatten().is_none() {
                return Err(format!("{}: playback switch read failed", name));
            }
            values.push(HardwareVolumeChannelValue {
                channel: name.clone(),
                raw: c_long_to_i64(raw),
                db_millibels,
                playback_switch: playback_switch.flatten(),
            });
        }
        Ok(values)
    }

    fn alsa_error_text(code: i32) -> String {
        unsafe {
            let message = ffi::snd_strerror(code);
            if message.is_null() {
                format!("ALSA error {code}")
            } else {
                CStr::from_ptr(message).to_string_lossy().into_owned()
            }
        }
    }

    fn probe_candidate(
        element: *mut ffi::snd_mixer_elem_t,
        ctl: Option<&RawCtl>,
    ) -> Option<HardwareVolumeCandidate> {
        let id = selem_id(element)?;
        let mut candidate = HardwareVolumeCandidate {
            label: if id.index == 0 {
                id.name.clone()
            } else {
                format!("{} ({})", id.name, id.index)
            },
            id,
            channels: Vec::new(),
            raw_range: None,
            db_range: None,
            current_values: Vec::new(),
            current_volume: None,
            has_playback_switch: unsafe { ffi::snd_mixer_selem_has_playback_switch(element) > 0 },
            recommended: false,
            writability: HardwareVolumeWritability::NeedsEnableCheck,
            rejection_reason: None,
        };

        let active = unsafe { ffi::snd_mixer_selem_is_active(element) } > 0;
        let has_playback = unsafe { ffi::snd_mixer_selem_has_playback_volume(element) } > 0;
        let has_capture = unsafe { ffi::snd_mixer_selem_has_capture_volume(element) } > 0;
        let has_common_volume = unsafe { ffi::snd_mixer_selem_has_common_volume(element) } > 0;

        let mut min = 0;
        let mut max = 0;
        let raw_range = has_playback.then(|| {
            let _ = unsafe {
                ffi::snd_mixer_selem_get_playback_volume_range(element, &mut min, &mut max)
            };
            HardwareVolumeRange {
                min: c_long_to_i64(min),
                max: c_long_to_i64(max),
            }
        });
        candidate.raw_range = raw_range;
        let channels = if has_playback {
            playback_channels(element)
        } else {
            Vec::new()
        };
        candidate.channels = channels.iter().map(|(_, name)| name.clone()).collect();
        if let Some(reason) = structural_rejection(CandidateFacts {
            name: &candidate.id.name,
            active,
            has_playback,
            has_capture,
            has_common_volume,
            raw_range,
            playback_channels: channels.len(),
        }) {
            candidate.rejection_reason = Some(reason);
            return Some(candidate);
        }
        let raw_range = raw_range.expect("structural validation requires a valid range");
        candidate.db_range = finite_db_range(element, raw_range);
        match read_channels(
            element,
            &channels,
            candidate.db_range,
            candidate.has_playback_switch,
        ) {
            Ok(values) => candidate.current_values = values,
            Err(error) => {
                candidate.rejection_reason = Some(HardwareVolumeRejectionReason::ReadFailed(error));
                return Some(candidate);
            }
        }

        let writable = ctl.and_then(|ctl| ctl.writable(&candidate.id));
        if let Some(reason) = writability_rejection(writable) {
            candidate.rejection_reason = Some(reason);
            return Some(candidate);
        }
        if writable == Some(true) {
            candidate.writability = HardwareVolumeWritability::Verified;
        }
        candidate.current_volume = Some(normalize_level(
            raw_range,
            candidate.db_range,
            &candidate.current_values,
        ));
        Some(candidate)
    }

    fn ucm_identifier(value: &str, device: Option<&str>) -> String {
        match device {
            Some(device) => format!("={value}/{device}/"),
            None => format!("={value}//"),
        }
    }

    fn ucm_route_matches(
        manager: &UcmManager,
        route: &ParsedAlsaRoute,
        device: Option<&str>,
    ) -> bool {
        let identifier = ucm_identifier("PlaybackPCM", device);
        let Some(pcm) = manager.get(&identifier) else {
            return false;
        };
        parse_alsa_route(&pcm)
            .is_some_and(|ucm_route| ucm_route.device == route.device && ucm_route.pcm == route.pcm)
    }

    fn parse_ucm_control(
        value_name: &str,
        value: &str,
        candidates: &[HardwareVolumeCandidate],
    ) -> Option<AlsaMixerControlId> {
        // The parser API takes a canonical UCM value kind, not the
        // `snd_use_case_get()` path used to retrieve it. ALSA names the
        // simple-element parser kind `PlaybackMixerId` even when the value
        // originated in PlaybackMasterElem or PlaybackMixerElem.
        let parser_id = CString::new(ucm_parser_identifier(value_name)?).ok()?;
        let value_c = CString::new(value).ok()?;
        let parsed = if value_name == "PlaybackVolume" {
            let mut raw_id = ptr::null_mut();
            if unsafe { ffi::snd_ctl_elem_id_malloc(&mut raw_id) } < 0 || raw_id.is_null() {
                return None;
            }
            let code = unsafe {
                snd_use_case_parse_ctl_elem_id(raw_id, parser_id.as_ptr(), value_c.as_ptr())
            };
            let result = (code >= 0).then(|| {
                let name = unsafe { ffi::snd_ctl_elem_id_get_name(raw_id) };
                AlsaMixerControlId {
                    name: if name.is_null() {
                        String::new()
                    } else {
                        unsafe { CStr::from_ptr(name) }
                            .to_string_lossy()
                            .into_owned()
                    },
                    index: unsafe { ffi::snd_ctl_elem_id_get_index(raw_id) },
                }
            });
            unsafe {
                ffi::snd_ctl_elem_id_free(raw_id);
            }
            result
        } else {
            let mut selem_id = ptr::null_mut();
            if unsafe { ffi::snd_mixer_selem_id_malloc(&mut selem_id) } < 0 || selem_id.is_null() {
                return None;
            }
            let code = unsafe {
                snd_use_case_parse_selem_id(selem_id, parser_id.as_ptr(), value_c.as_ptr())
            };
            let result = (code >= 0).then(|| {
                let name = unsafe { ffi::snd_mixer_selem_id_get_name(selem_id) };
                AlsaMixerControlId {
                    name: if name.is_null() {
                        String::new()
                    } else {
                        unsafe { CStr::from_ptr(name) }
                            .to_string_lossy()
                            .into_owned()
                    },
                    index: unsafe { ffi::snd_mixer_selem_id_get_index(selem_id) },
                }
            });
            unsafe {
                ffi::snd_mixer_selem_id_free(selem_id);
            }
            result
        }
        .or_else(|| parse_ucm_control_text(value))?;

        candidates
            .iter()
            .find(|candidate| candidate.id == parsed)
            .map(|candidate| candidate.id.clone())
            .or_else(|| {
                let selem_name = parsed
                    .name
                    .strip_suffix(" Playback Volume")
                    .or_else(|| parsed.name.strip_suffix(" Volume"))?;
                candidates
                    .iter()
                    .find(|candidate| {
                        candidate.id.index == parsed.index && candidate.id.name == selem_name
                    })
                    .map(|candidate| candidate.id.clone())
            })
    }

    fn parse_ucm_control_text(value: &str) -> Option<AlsaMixerControlId> {
        let trimmed = value.trim();
        let name = if let Some(position) = trimmed.find("name=") {
            let rest = trimmed[position + 5..].trim_start();
            quoted_or_token(rest)?
        } else {
            quoted_or_token(trimmed)?
        };
        let index = trimmed
            .find("index=")
            .and_then(|position| {
                trimmed[position + 6..]
                    .trim_start()
                    .split(|character: char| !character.is_ascii_digit())
                    .next()
            })
            .and_then(|value| value.parse::<u32>().ok())
            .or_else(|| {
                trimmed
                    .rsplit_once(',')
                    .and_then(|(_, value)| value.trim().parse::<u32>().ok())
            })
            .unwrap_or(0);
        Some(AlsaMixerControlId { name, index })
    }

    fn quoted_or_token(value: &str) -> Option<String> {
        let value = value.trim_start();
        let first = value.chars().next()?;
        if first == '\'' || first == '"' {
            let rest = &value[first.len_utf8()..];
            let end = rest.find(first)?;
            return Some(rest[..end].to_string());
        }
        let end = value
            .find(|character: char| character == ',' || character.is_whitespace())
            .unwrap_or(value.len());
        (end > 0).then(|| value[..end].to_string())
    }

    fn ucm_controls_for_route(
        route: &ParsedAlsaRoute,
        candidates: &[HardwareVolumeCandidate],
    ) -> Vec<AlsaMixerControlId> {
        let Some(manager) = UcmManager::open(&route.card) else {
            return Vec::new();
        };
        let Some(verb) = manager.get("_verb") else {
            return Vec::new();
        };
        if verb.eq_ignore_ascii_case("Inactive") {
            return Vec::new();
        }

        let enabled_devices = manager.list("_enadevs");
        let mut contexts = Vec::new();
        if ucm_route_matches(&manager, route, None) {
            contexts.push(None);
        }
        contexts.extend(
            enabled_devices
                .iter()
                .filter(|device| ucm_route_matches(&manager, route, Some(device)))
                .map(|device| Some(device.as_str())),
        );
        if contexts.is_empty() {
            return Vec::new();
        }

        for value_name in ["PlaybackMasterElem", "PlaybackMixerElem", "PlaybackVolume"] {
            let mut controls = HashSet::new();
            for device in &contexts {
                if value_name == "PlaybackMasterElem" {
                    let master_type = manager
                        .get(&ucm_identifier("PlaybackMasterType", *device))
                        .or_else(|| {
                            device
                                .is_some()
                                .then(|| manager.get(&ucm_identifier("PlaybackMasterType", None)))
                                .flatten()
                        });
                    if master_type.is_some_and(|value| value.eq_ignore_ascii_case("soft")) {
                        continue;
                    }
                }
                let identifier = ucm_identifier(value_name, *device);
                let Some(value) = manager.get(&identifier) else {
                    continue;
                };
                if let Some(control) = parse_ucm_control(value_name, &value, candidates) {
                    controls.insert(control);
                }
            }
            if !controls.is_empty() {
                let mut controls = controls.into_iter().collect::<Vec<_>>();
                controls.sort_by(|left, right| {
                    left.name
                        .cmp(&right.name)
                        .then_with(|| left.index.cmp(&right.index))
                });
                return controls;
            }
        }
        Vec::new()
    }

    pub(super) fn stable_route_key(
        device_id: &str,
    ) -> Result<StableAlsaRouteKey, HardwareVolumeProbeError> {
        let route = parse_alsa_route(device_id).ok_or_else(|| HardwareVolumeProbeError {
            kind: HardwareVolumeProbeErrorKind::InvalidDevice,
            message: format!("Cannot parse ALSA PCM route: {device_id}"),
        })?;
        let (card_number, short_id) =
            resolve_card(&route.card).ok_or_else(|| HardwareVolumeProbeError {
                kind: HardwareVolumeProbeErrorKind::DeviceUnavailable,
                message: format!("ALSA card '{}' is not present", route.card),
            })?;
        let control = PathBuf::from(format!("/dev/snd/controlC{card_number}"));
        let identity = select_stable_card_identity(
            has_real_card_serial(card_number),
            find_stable_link("/dev/snd/by-id", &control),
            find_stable_link("/dev/snd/by-path", &control),
            short_id,
        );
        Ok(StableAlsaRouteKey::from_parts(
            &identity,
            &route.pcm,
            route.device,
        ))
    }

    /// A udev `by-id` link may exist even when USB's ID_SERIAL was synthesized
    /// from vendor + model alone (for example `Fosi_Fosi_Audio_ZH3`). Only a
    /// usable serial attribute on the owning USB device is strong enough to
    /// distinguish two otherwise-identical DACs; without it, port identity
    /// (`by-path`) is the safe key. Some devices publish placeholders such as
    /// `0000`, which udev still includes in `ID_SERIAL`; those are not stable
    /// identities. Stop at the first USB device so a root hub's own serial
    /// cannot be mistaken for the DAC's.
    fn has_real_card_serial(card_number: u32) -> bool {
        let Ok(device) = fs::canonicalize(format!("/sys/class/sound/card{card_number}/device"))
        else {
            return false;
        };
        has_real_usb_serial(device, Path::new("/sys/devices"))
    }

    fn has_real_usb_serial(mut device: PathBuf, devices_root: &Path) -> bool {
        while device.starts_with(devices_root) {
            if device.join("idVendor").is_file() && device.join("idProduct").is_file() {
                return fs::read_to_string(device.join("serial"))
                    .ok()
                    .is_some_and(|serial| is_usable_usb_serial(&serial));
            }
            if !device.pop() {
                break;
            }
        }
        false
    }

    fn is_usable_usb_serial(serial: &str) -> bool {
        let serial = serial.trim();
        if serial.is_empty()
            || matches!(
                serial.to_ascii_lowercase().as_str(),
                "none" | "null" | "unknown" | "default" | "n/a" | "na"
            )
        {
            return false;
        }

        // A run of zeroes is a common firmware placeholder (the DacMagic Plus
        // reports `0000`). Keep zero-padded real identifiers such as `0001`.
        serial
            .chars()
            .any(|character| character.is_ascii_alphanumeric() && character != '0')
    }

    fn resolve_card(card: &str) -> Option<(u32, String)> {
        if let Ok(number) = card.parse::<u32>() {
            let short = fs::read_to_string(format!("/proc/asound/card{number}/id"))
                .ok()?
                .trim()
                .to_string();
            return (!short.is_empty()).then_some((number, short));
        }
        let entries = fs::read_dir("/proc/asound").ok()?;
        for entry in entries.flatten() {
            let file_name = entry.file_name();
            let Some(suffix) = file_name
                .to_string_lossy()
                .strip_prefix("card")
                .map(str::to_string)
            else {
                continue;
            };
            let Ok(number) = suffix.parse::<u32>() else {
                continue;
            };
            let Ok(short) = fs::read_to_string(entry.path().join("id")) else {
                continue;
            };
            if short.trim() == card {
                return Some((number, card.to_string()));
            }
        }
        None
    }

    fn find_stable_link(directory: &str, control: &Path) -> Option<String> {
        let target = fs::canonicalize(control).ok()?;
        let target_metadata = fs::metadata(&target).ok();
        let mut matches = fs::read_dir(directory)
            .ok()?
            .flatten()
            .filter_map(|entry| {
                let resolved = fs::canonicalize(entry.path()).ok()?;
                let same_path = resolved == target;
                let same_device = target_metadata.as_ref().is_some_and(|target| {
                    fs::metadata(&resolved)
                        .ok()
                        .is_some_and(|candidate| candidate.rdev() == target.rdev())
                });
                (same_path || same_device).then(|| entry.file_name().to_string_lossy().into_owned())
            })
            .collect::<Vec<_>>();
        matches.sort();
        matches.into_iter().next()
    }

    pub(super) fn enumerate(device_id: &str) -> HardwareVolumeProbe {
        let ctl_name = mixer_ctl_name(device_id);
        let route_key = stable_route_key(device_id);
        let mixer = match RawMixer::open(device_id) {
            Ok(mixer) => mixer,
            Err(error) => {
                return HardwareVolumeProbe {
                    device_id: device_id.to_string(),
                    ctl_name,
                    route_key: route_key.ok(),
                    candidates: Vec::new(),
                    ucm_controls: Vec::new(),
                    error: Some(error),
                };
            }
        };
        let ctl = RawCtl::open(&ctl_name, false).ok();
        let mut candidates = Vec::new();
        let mut element = unsafe { ffi::snd_mixer_first_elem(mixer.0) };
        while !element.is_null() {
            if unsafe { ffi::snd_mixer_elem_get_type(element) } == ffi::SND_MIXER_ELEM_SIMPLE {
                if let Some(candidate) = probe_candidate(element, ctl.as_ref()) {
                    candidates.push(candidate);
                }
            }
            element = unsafe { ffi::snd_mixer_elem_next(element) };
        }
        mark_recommended(&mut candidates);
        candidates.sort_by(|left, right| {
            right
                .is_valid()
                .cmp(&left.is_valid())
                .then_with(|| right.recommended.cmp(&left.recommended))
                .then_with(|| left.id.name.cmp(&right.id.name))
                .then_with(|| left.id.index.cmp(&right.id.index))
        });

        let (route_key, error) = match route_key {
            Ok(route_key) => (Some(route_key), None),
            Err(error) => (None, Some(error)),
        };
        let ucm_controls = parse_alsa_route(device_id)
            .map(|route| ucm_controls_for_route(&route, &candidates))
            .unwrap_or_default();
        HardwareVolumeProbe {
            device_id: device_id.to_string(),
            ctl_name,
            route_key,
            candidates,
            ucm_controls,
            error,
        }
    }

    fn exact_candidate(
        device_id: &str,
        id: &AlsaMixerControlId,
    ) -> Result<HardwareVolumeCandidate, String> {
        let probe = enumerate(device_id);
        if let Some(error) = probe.error {
            return Err(error.message);
        }
        probe
            .candidates
            .into_iter()
            .find(|candidate| candidate.id == *id && candidate.is_valid())
            .ok_or_else(|| format!("ALSA mixer control {id} is missing or no longer valid"))
    }

    pub(super) fn activate(
        device_id: &str,
        id: &AlsaMixerControlId,
    ) -> Result<HardwareVolumeSnapshot, String> {
        let candidate = exact_candidate(device_id, id)?;
        if candidate.writability == HardwareVolumeWritability::NeedsEnableCheck {
            let mixer = RawMixer::open(device_id).map_err(|error| error.message)?;
            let element = mixer
                .find(id)
                .ok_or_else(|| format!("ALSA mixer control {id} disappeared during enable"))?;
            let channels = playback_channels(element);
            let values = read_channels(
                element,
                &channels,
                candidate.db_range,
                candidate.has_playback_switch,
            )?;
            // Explicit user action only: write each channel's current value
            // back independently so the capability check cannot alter balance.
            for ((channel, name), value) in channels.iter().zip(&values) {
                let code = unsafe {
                    ffi::snd_mixer_selem_set_playback_volume(element, *channel, value.raw as _)
                };
                if code < 0 {
                    return Err(format!(
                        "ALSA mixer control {id} is not writable on {name}: {}",
                        alsa_error_text(code)
                    ));
                }
            }
        }
        read(device_id, id)
    }

    pub(super) fn read(
        device_id: &str,
        id: &AlsaMixerControlId,
    ) -> Result<HardwareVolumeSnapshot, String> {
        let mixer = RawMixer::open(device_id).map_err(|error| error.message)?;
        let element = mixer
            .find(id)
            .ok_or_else(|| format!("ALSA mixer control {id} is missing"))?;
        let channels = playback_channels(element);
        if channels.is_empty() {
            return Err(format!("ALSA mixer control {id} has no playback channels"));
        }
        let mut min = 0;
        let mut max = 0;
        if unsafe { ffi::snd_mixer_selem_get_playback_volume_range(element, &mut min, &mut max) }
            < 0
            || max <= min
        {
            return Err(format!(
                "ALSA mixer control {id} has no usable volume range"
            ));
        }
        let raw_range = HardwareVolumeRange {
            min: c_long_to_i64(min),
            max: c_long_to_i64(max),
        };
        let db_range = finite_db_range(element, raw_range);
        let has_switch = unsafe { ffi::snd_mixer_selem_has_playback_switch(element) } > 0;
        let values = read_channels(element, &channels, db_range, has_switch)?;
        let muted = has_switch
            && values
                .iter()
                .filter_map(|value| value.playback_switch)
                .all(|enabled| !enabled);
        Ok(HardwareVolumeSnapshot {
            control: id.clone(),
            volume: normalize_level(raw_range, db_range, &values),
            channels: values,
            muted,
        })
    }

    pub(super) fn set(
        device_id: &str,
        id: &AlsaMixerControlId,
        scalar: f32,
    ) -> Result<HardwareVolumeSnapshot, String> {
        let candidate = exact_candidate(device_id, id)?;
        let raw_range = candidate
            .raw_range
            .ok_or_else(|| format!("ALSA mixer control {id} has no usable raw range"))?;
        let mixer = RawMixer::open(device_id).map_err(|error| error.message)?;
        let element = mixer
            .find(id)
            .ok_or_else(|| format!("ALSA mixer control {id} disappeared before write"))?;
        let channels = playback_channels(element);
        let scalar = scalar.clamp(0.0, 1.0);
        let write_plan = hardware_volume_write_plan(scalar, candidate.has_playback_switch);

        if write_plan.playback_switch == Some(false) {
            for (channel, name) in &channels {
                let code =
                    unsafe { ffi::snd_mixer_selem_set_playback_switch(element, *channel, 0) };
                if code < 0 {
                    return Err(format!(
                        "Failed to mute {id} on {name}: {}",
                        alsa_error_text(code)
                    ));
                }
            }
            let snapshot = read(device_id, id)?;
            remember_local_write(device_id, id, snapshot.volume);
            return Ok(snapshot);
        }

        if write_plan.playback_switch == Some(true) {
            // Restore the hardware route before moving its volume away from zero.
            for (channel, name) in &channels {
                let code =
                    unsafe { ffi::snd_mixer_selem_set_playback_switch(element, *channel, 1) };
                if code < 0 {
                    return Err(format!(
                        "Failed to unmute {id} on {name}: {}",
                        alsa_error_text(code)
                    ));
                }
            }
        }
        debug_assert!(write_plan.write_level);

        let values = read_channels(
            element,
            &channels,
            candidate.db_range,
            candidate.has_playback_switch,
        )?;
        let mut used_db = false;
        // With no playback switch, UI zero means the raw minimum even when a
        // driver's finite dB range deliberately excludes its mute sentinel.
        if scalar > f32::EPSILON {
            if let Some(db_range) = candidate.db_range {
                let current_db = values
                    .iter()
                    .map(|value| value.db_millibels)
                    .collect::<Option<Vec<_>>>();
                if let Some(current_db) = current_db {
                    let targets = channel_targets(&current_db, db_range, scalar);
                    if targets.len() == channels.len() {
                        let mut failed = None;
                        for ((channel, name), target) in channels.iter().zip(targets) {
                            let code = unsafe {
                                ffi::snd_mixer_selem_set_playback_dB(
                                    element,
                                    *channel,
                                    target as _,
                                    0,
                                )
                            };
                            if code < 0 {
                                failed = Some(format!("{name}: {}", alsa_error_text(code)));
                                break;
                            }
                        }
                        if let Some(error) = failed {
                            log::warn!(
                                "[ALSA HW Volume] dB write failed for {id}; using raw fallback: {error}"
                            );
                        } else {
                            used_db = true;
                        }
                    }
                }
            }
        }

        if !used_db {
            let current_raw = values.iter().map(|value| value.raw).collect::<Vec<_>>();
            let targets = channel_targets(&current_raw, raw_range, scalar);
            for ((channel, name), target) in channels.iter().zip(targets) {
                let code = unsafe {
                    ffi::snd_mixer_selem_set_playback_volume(element, *channel, target as _)
                };
                if code < 0 {
                    return Err(format!(
                        "Failed to set raw ALSA volume for {id} on {name}: {}",
                        alsa_error_text(code)
                    ));
                }
            }
        }

        let snapshot = read(device_id, id)?;
        remember_local_write(device_id, id, snapshot.volume);
        Ok(snapshot)
    }

    type LocalWriteMap = HashMap<String, (f32, Instant)>;
    static LOCAL_WRITES: std::sync::OnceLock<std::sync::Mutex<LocalWriteMap>> =
        std::sync::OnceLock::new();

    fn runtime_control_key(device_id: &str, id: &AlsaMixerControlId) -> String {
        format!("{device_id}\u{1f}{}\u{1f}{}", id.name, id.index)
    }

    fn remember_local_write(device_id: &str, id: &AlsaMixerControlId, volume: f32) {
        let writes = LOCAL_WRITES.get_or_init(Default::default);
        if let Ok(mut writes) = writes.lock() {
            writes.insert(runtime_control_key(device_id, id), (volume, Instant::now()));
        }
    }

    fn is_recent_local_write(device_id: &str, id: &AlsaMixerControlId, volume: f32) -> bool {
        let writes = LOCAL_WRITES.get_or_init(Default::default);
        let Ok(mut writes) = writes.lock() else {
            return false;
        };
        let key = runtime_control_key(device_id, id);
        let suppress = writes.get(&key).is_some_and(|(written, at)| {
            suppress_local_write_echo(*written, at.elapsed().as_millis(), volume)
        });
        if suppress {
            writes.remove(&key);
        }
        suppress
    }

    pub(super) fn subscribe(
        device_id: &str,
        id: &AlsaMixerControlId,
        callback: HardwareVolumeEventCallback,
    ) -> Result<HardwareVolumeEventSubscription, String> {
        let ctl_name = mixer_ctl_name(device_id);
        let ctl = Ctl::new(&ctl_name, true)
            .map_err(|error| format!("Failed to open ALSA ctl {ctl_name}: {error}"))?;
        ctl.subscribe_events(true)
            .map_err(|error| format!("Failed to subscribe to ALSA ctl {ctl_name}: {error}"))?;
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = stop.clone();
        let device_id = device_id.to_string();
        let id = id.clone();
        let thread = std::thread::Builder::new()
            .name("qbz-alsa-volume-events".to_string())
            .spawn(move || {
                let mut last_published = read(&device_id, &id).ok().map(|snapshot| snapshot.volume);
                'events: while !thread_stop.load(Ordering::SeqCst) {
                    match ctl.wait(Some(100)) {
                        Ok(true) => {
                            let mut changed = false;
                            loop {
                                match ctl.read() {
                                    Ok(Some(event)) => changed |= event.get_mask().value(),
                                    Ok(None) => break,
                                    Err(error) => {
                                        callback(HardwareVolumeEvent::Unavailable(format!(
                                            "ALSA ctl event read failed: {error}"
                                        )));
                                        break 'events;
                                    }
                                }
                            }
                            if !changed {
                                continue;
                            }
                            std::thread::sleep(Duration::from_millis(35));
                            match read(&device_id, &id) {
                                Ok(snapshot) => {
                                    if is_recent_local_write(&device_id, &id, snapshot.volume) {
                                        last_published = Some(snapshot.volume);
                                        continue;
                                    }
                                    if last_published.is_some_and(|previous| {
                                        (previous - snapshot.volume).abs() < 0.001
                                    }) {
                                        continue;
                                    }
                                    last_published = Some(snapshot.volume);
                                    callback(HardwareVolumeEvent::Changed(snapshot));
                                }
                                Err(error) => {
                                    callback(HardwareVolumeEvent::Unavailable(error));
                                    break;
                                }
                            }
                        }
                        Ok(false) => {}
                        Err(error) => {
                            callback(HardwareVolumeEvent::Unavailable(format!(
                                "ALSA ctl event subscription failed: {error}"
                            )));
                            break;
                        }
                    }
                }
                let _ = ctl.subscribe_events(false);
            })
            .map_err(|error| format!("Failed to start ALSA volume event thread: {error}"))?;
        Ok(HardwareVolumeEventSubscription {
            stop,
            thread: Some(thread),
        })
    }

    #[cfg(test)]
    mod sysfs_tests {
        use super::*;

        #[test]
        fn dac_without_serial_does_not_inherit_the_root_hub_serial() {
            let root = std::env::temp_dir().join(format!(
                "qbz-alsa-serial-test-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .expect("clock after epoch")
                    .as_nanos()
            ));
            let hub = root.join("usb3");
            let dac = hub.join("3-5");
            let interface = dac.join("3-5:1.1");
            fs::create_dir_all(&interface).expect("create fake sysfs tree");
            for device in [&hub, &dac] {
                fs::write(device.join("idVendor"), "1234").expect("write vendor");
                fs::write(device.join("idProduct"), "5678").expect("write product");
            }
            fs::write(hub.join("serial"), "0000:00:14.0").expect("write hub serial");

            assert!(!has_real_usb_serial(interface.clone(), &root));
            fs::write(dac.join("serial"), "0000").expect("write placeholder DAC serial");
            assert!(!has_real_usb_serial(interface.clone(), &root));
            fs::write(dac.join("serial"), "REAL-DAC-SERIAL").expect("write DAC serial");
            assert!(has_real_usb_serial(interface, &root));

            fs::remove_dir_all(root).expect("remove fake sysfs tree");
        }

        #[test]
        fn usb_serial_placeholders_are_not_stable_identities() {
            for placeholder in [
                "", "   ", "0000", "none", "NULL", "unknown", "default", "N/A",
            ] {
                assert!(!is_usable_usb_serial(placeholder), "{placeholder:?}");
            }
            assert!(is_usable_usb_serial("0001"));
            assert!(is_usable_usb_serial("REAL-DAC-SERIAL"));
        }
    }
}

/// Enumerate every simple-mixer element and retain rejected candidates for
/// diagnostics. This function never writes to ALSA.
pub fn enumerate_hardware_volume_controls(device_id: &str) -> HardwareVolumeProbe {
    #[cfg(target_os = "linux")]
    {
        linux::enumerate(device_id)
    }
    #[cfg(not(target_os = "linux"))]
    {
        HardwareVolumeProbe {
            device_id: device_id.to_string(),
            ctl_name: device_id.to_string(),
            route_key: None,
            candidates: Vec::new(),
            ucm_controls: Vec::new(),
            error: Some(HardwareVolumeProbeError {
                kind: HardwareVolumeProbeErrorKind::UnsupportedPlatform,
                message: "ALSA hardware volume is only available on Linux".to_string(),
            }),
        }
    }
}

pub fn stable_alsa_route_key(device_id: &str) -> Result<StableAlsaRouteKey, String> {
    #[cfg(target_os = "linux")]
    {
        linux::stable_route_key(device_id).map_err(|error| error.message)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = device_id;
        Err("ALSA route identities are only available on Linux".to_string())
    }
}

/// Explicit enable-time validation. If ctl identity cannot prove writability,
/// this performs a per-channel writeback of the values just read.
pub fn activate_hardware_volume_control(
    device_id: &str,
    id: &AlsaMixerControlId,
) -> Result<HardwareVolumeSnapshot, String> {
    #[cfg(target_os = "linux")]
    {
        linux::activate(device_id, id)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (device_id, id);
        Err("ALSA hardware volume is only available on Linux".to_string())
    }
}

pub fn read_hardware_volume(
    device_id: &str,
    id: &AlsaMixerControlId,
) -> Result<HardwareVolumeSnapshot, String> {
    #[cfg(target_os = "linux")]
    {
        linux::read(device_id, id)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (device_id, id);
        Err("ALSA hardware volume is only available on Linux".to_string())
    }
}

pub fn set_hardware_volume(
    device_id: &str,
    id: &AlsaMixerControlId,
    volume: f32,
) -> Result<HardwareVolumeSnapshot, String> {
    #[cfg(target_os = "linux")]
    {
        linux::set(device_id, id, volume)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (device_id, id, volume);
        Err("ALSA hardware volume is only available on Linux".to_string())
    }
}

pub fn subscribe_hardware_volume_events(
    device_id: &str,
    id: &AlsaMixerControlId,
    callback: HardwareVolumeEventCallback,
) -> Result<HardwareVolumeEventSubscription, String> {
    #[cfg(target_os = "linux")]
    {
        linux::subscribe(device_id, id, callback)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (device_id, id, callback);
        Err("ALSA hardware volume is only available on Linux".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(name: &str, index: u32, valid: bool) -> HardwareVolumeCandidate {
        HardwareVolumeCandidate {
            id: AlsaMixerControlId {
                name: name.to_string(),
                index,
            },
            label: name.to_string(),
            channels: vec!["Front Left".to_string(), "Front Right".to_string()],
            raw_range: Some(HardwareVolumeRange { min: 0, max: 100 }),
            db_range: Some(HardwareVolumeRange { min: -6000, max: 0 }),
            current_values: Vec::new(),
            current_volume: Some(0.5),
            has_playback_switch: true,
            recommended: false,
            writability: HardwareVolumeWritability::Verified,
            rejection_reason: (!valid).then_some(HardwareVolumeRejectionReason::ReadOnly),
        }
    }

    fn probe(candidates: Vec<HardwareVolumeCandidate>) -> HardwareVolumeProbe {
        HardwareVolumeProbe {
            device_id: "front:CARD=D50,DEV=0".to_string(),
            ctl_name: "hw:CARD=D50".to_string(),
            route_key: Some(StableAlsaRouteKey(
                "alsa-route-v1|card=card:D50|pcm=front|DEV=0".to_string(),
            )),
            candidates,
            ucm_controls: Vec::new(),
            error: None,
        }
    }

    #[test]
    fn unknown_single_candidate_is_selected() {
        let decision =
            decide_hardware_volume(&probe(vec![candidate("D50 III", 0, true)]), &HashMap::new());
        assert!(matches!(
            decision,
            HardwareVolumeDecision::Selected {
                selection: HardwareVolumeSelection {
                    source: HardwareVolumeSelectionSource::OnlyCandidate,
                    ..
                }
            }
        ));
    }

    #[test]
    fn two_unknown_candidates_need_choice() {
        let decision = decide_hardware_volume(
            &probe(vec![
                candidate("Output A", 0, true),
                candidate("Output B", 0, true),
            ]),
            &HashMap::new(),
        );
        assert!(matches!(
            decision,
            HardwareVolumeDecision::NeedsChoice {
                reason: HardwareVolumeChoiceReason::Ambiguous,
                ..
            }
        ));
    }

    #[test]
    fn same_name_different_indices_remain_distinct() {
        let decision = decide_hardware_volume(
            &probe(vec![candidate("DAC", 0, true), candidate("DAC", 1, true)]),
            &HashMap::new(),
        );
        let HardwareVolumeDecision::NeedsChoice { candidates, .. } = decision else {
            panic!("expected choice");
        };
        assert_eq!(candidates.len(), 2);
        assert_ne!(candidates[0].id, candidates[1].id);
    }

    #[test]
    fn invalid_master_does_not_block_valid_unknown_control() {
        let decision = decide_hardware_volume(
            &probe(vec![
                candidate("Master", 0, false),
                candidate("D50 III", 0, true),
            ]),
            &HashMap::new(),
        );
        let HardwareVolumeDecision::Selected { selection } = decision else {
            panic!("expected selection");
        };
        assert_eq!(selection.control.name, "D50 III");
    }

    #[test]
    fn persisted_exact_match_wins_but_stale_does_not_fall_through() {
        let probe = probe(vec![
            candidate("Master", 0, true),
            candidate("DAC", 0, true),
        ]);
        let route = probe.route_key.clone().unwrap();
        let mut persisted = HashMap::new();
        persisted.insert(
            route.clone(),
            AlsaMixerControlId {
                name: "DAC".into(),
                index: 0,
            },
        );
        let selected = decide_hardware_volume(&probe, &persisted);
        assert!(
            matches!(selected, HardwareVolumeDecision::Selected { selection } if selection.control.name == "DAC")
        );

        persisted.insert(
            route,
            AlsaMixerControlId {
                name: "Gone".into(),
                index: 0,
            },
        );
        assert!(matches!(
            decide_hardware_volume(&probe, &persisted),
            HardwareVolumeDecision::NeedsChoice {
                reason: HardwareVolumeChoiceReason::PersistedSelectionStale,
                ..
            }
        ));
    }

    #[test]
    fn ucm_must_be_unambiguous() {
        let mut probe = probe(vec![
            candidate("Speaker", 0, true),
            candidate("Headphone", 0, true),
        ]);
        probe.ucm_controls = vec![probe.candidates[1].id.clone()];
        assert!(matches!(
            decide_hardware_volume(&probe, &HashMap::new()),
            HardwareVolumeDecision::Selected { selection }
                if selection.source == HardwareVolumeSelectionSource::Ucm
                    && selection.control.name == "Headphone"
        ));
        probe.ucm_controls = probe
            .candidates
            .iter()
            .map(|candidate| candidate.id.clone())
            .collect();
        assert!(matches!(
            decide_hardware_volume(&probe, &HashMap::new()),
            HardwareVolumeDecision::NeedsChoice {
                reason: HardwareVolumeChoiceReason::UcmAmbiguous,
                ..
            }
        ));
    }

    #[test]
    fn rejected_ucm_hint_does_not_make_one_valid_ucm_control_ambiguous() {
        let mut probe = probe(vec![
            candidate("Speaker", 0, true),
            candidate("Mic Boost", 0, false),
        ]);
        probe.ucm_controls = probe
            .candidates
            .iter()
            .map(|candidate| candidate.id.clone())
            .collect();

        assert!(matches!(
            decide_hardware_volume(&probe, &HashMap::new()),
            HardwareVolumeDecision::Selected { selection }
                if selection.source == HardwareVolumeSelectionSource::Ucm
                    && selection.control.name == "Speaker"
        ));
    }

    #[test]
    fn stable_keys_prefer_serial_then_path_then_symbolic_card_and_include_route() {
        assert_eq!(
            select_stable_card_identity(
                true,
                Some("usb-Fosi_ZH3_REAL123-00".into()),
                Some("pci-usb-port-1".into()),
                "ZH3".into(),
            ),
            StableAlsaCardIdentity::ById("usb-Fosi_ZH3_REAL123-00".into())
        );
        assert_eq!(
            select_stable_card_identity(
                false,
                Some("usb-Fosi_Fosi_Audio_ZH3-00".into()),
                Some("pci-usb-port-1".into()),
                "ZH3".into(),
            ),
            StableAlsaCardIdentity::ByPath("pci-usb-port-1".into())
        );
        assert_eq!(
            select_stable_card_identity(
                false,
                Some("usb-iBasso_Macaron-01".into()),
                None,
                "Macaron".into(),
            ),
            StableAlsaCardIdentity::Card("Macaron".into())
        );

        let by_id = StableAlsaRouteKey::from_parts(
            &StableAlsaCardIdentity::ById("usb-Topping_D50_III_1234-00".into()),
            "front",
            0,
        );
        let by_path = StableAlsaRouteKey::from_parts(
            &StableAlsaCardIdentity::ByPath("pci-0000:00:14.0-usb-0:2:1.0".into()),
            "front",
            0,
        );
        let card =
            StableAlsaRouteKey::from_parts(&StableAlsaCardIdentity::Card("D50".into()), "hdmi", 1);
        assert!(by_id.as_str().contains("card=by-id:"));
        assert!(by_path.as_str().contains("card=by-path:"));
        assert!(card.as_str().contains("card=card:D50"));
        assert!(card.as_str().contains("pcm=hdmi|DEV=1"));
        assert!(!card.as_str().contains("hw:0"));
    }

    #[test]
    fn parser_keeps_pcm_device_in_route_identity() {
        assert_eq!(
            parse_alsa_route("iec958:CARD=PCH,DEV=1"),
            Some(ParsedAlsaRoute {
                card: "PCH".into(),
                pcm: "iec958".into(),
                device: 1,
            })
        );
        assert_eq!(mixer_ctl_name("hw:2,7"), "hw:2");
        assert_eq!(mixer_ctl_name("front:CARD=USB,DEV=0"), "hw:CARD=USB");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn ucm_parser_uses_canonical_alsa_value_identifiers() {
        assert_eq!(
            ucm_parser_identifier("PlaybackVolume"),
            Some("PlaybackVolume")
        );
        assert_eq!(
            ucm_parser_identifier("PlaybackMasterElem"),
            Some("PlaybackMixerId")
        );
        assert_eq!(
            ucm_parser_identifier("PlaybackMixerElem"),
            Some("PlaybackMixerId")
        );
        assert_eq!(ucm_parser_identifier("PlaybackSwitch"), None);
    }

    #[test]
    fn channel_mapping_preserves_balance_and_clamps() {
        let range = HardwareVolumeRange { min: -6000, max: 0 };
        assert_eq!(
            channel_targets(&[-1200, -1800], range, 0.5),
            vec![-3000, -3600]
        );
        assert_eq!(channel_targets(&[-1200, -1800], range, 1.0), vec![0, -600]);
        assert_eq!(
            channel_targets(&[-1200, -1800], range, 0.0),
            vec![-6000, -6000]
        );
    }

    #[test]
    fn recommendation_never_resurrects_rejected_candidate() {
        let mut candidates = vec![candidate("Master", 0, false), candidate("D50 III", 0, true)];
        mark_recommended(&mut candidates);
        assert!(!candidates[0].recommended);
        assert!(candidates[1].recommended);
    }

    #[test]
    fn structural_validation_rejects_unsafe_candidate_shapes_with_exact_reasons() {
        let valid = CandidateFacts {
            name: "D50 III",
            active: true,
            has_playback: true,
            has_capture: false,
            has_common_volume: false,
            raw_range: Some(HardwareVolumeRange { min: 0, max: 100 }),
            playback_channels: 2,
        };
        assert_eq!(structural_rejection(valid), None);
        assert_eq!(
            structural_rejection(CandidateFacts {
                raw_range: Some(HardwareVolumeRange { min: 7, max: 7 }),
                ..valid
            }),
            Some(HardwareVolumeRejectionReason::InvalidRange { min: 7, max: 7 })
        );
        assert_eq!(
            structural_rejection(CandidateFacts {
                playback_channels: 0,
                ..valid
            }),
            Some(HardwareVolumeRejectionReason::NoPlaybackChannels)
        );
        assert_eq!(
            structural_rejection(CandidateFacts {
                has_common_volume: true,
                ..valid
            }),
            Some(HardwareVolumeRejectionReason::CommonPlaybackCaptureVolume)
        );
        assert_eq!(
            structural_rejection(CandidateFacts {
                has_playback: false,
                has_capture: true,
                raw_range: None,
                playback_channels: 0,
                ..valid
            }),
            Some(HardwareVolumeRejectionReason::CaptureOnly)
        );
        assert_eq!(
            structural_rejection(CandidateFacts {
                name: "Mic Boost",
                ..valid
            }),
            Some(HardwareVolumeRejectionReason::UnsafeInputPath)
        );
        assert_eq!(
            writability_rejection(Some(false)),
            Some(HardwareVolumeRejectionReason::ReadOnly)
        );
        assert_eq!(writability_rejection(Some(true)), None);
        assert_eq!(writability_rejection(None), None);
    }

    #[test]
    fn raw_and_db_normalization_use_db_when_complete_and_raw_as_fallback() {
        let raw_range = HardwareVolumeRange { min: 0, max: 100 };
        let db_range = HardwareVolumeRange { min: -6000, max: 0 };
        let db_values = vec![
            HardwareVolumeChannelValue {
                channel: "L".into(),
                raw: 80,
                db_millibels: Some(-3000),
                playback_switch: Some(true),
            },
            HardwareVolumeChannelValue {
                channel: "R".into(),
                raw: 70,
                db_millibels: Some(-3600),
                playback_switch: Some(true),
            },
        ];
        assert!((normalize_level(raw_range, Some(db_range), &db_values) - 0.5).abs() < 0.001);

        let mut broken_db = db_values.clone();
        broken_db[0].db_millibels = None;
        broken_db[1].db_millibels = None;
        assert!((normalize_level(raw_range, Some(db_range), &broken_db) - 0.8).abs() < 0.001);

        let mut partial_db = db_values.clone();
        partial_db[0].db_millibels = None;
        assert!((normalize_level(raw_range, Some(db_range), &partial_db) - 0.8).abs() < 0.001);
    }

    #[test]
    fn mute_unmute_plan_and_partial_channel_switches_are_safe() {
        assert_eq!(
            hardware_volume_write_plan(0.0, true),
            HardwareVolumeWritePlan {
                playback_switch: Some(false),
                write_level: false,
            }
        );
        assert_eq!(
            hardware_volume_write_plan(0.25, true),
            HardwareVolumeWritePlan {
                playback_switch: Some(true),
                write_level: true,
            }
        );
        assert_eq!(
            hardware_volume_write_plan(0.0, false),
            HardwareVolumeWritePlan {
                playback_switch: None,
                write_level: true,
            }
        );

        let range = HardwareVolumeRange { min: 0, max: 100 };
        let mut values = vec![
            HardwareVolumeChannelValue {
                channel: "L".into(),
                raw: 40,
                db_millibels: None,
                playback_switch: Some(false),
            },
            HardwareVolumeChannelValue {
                channel: "R".into(),
                raw: 60,
                db_millibels: None,
                playback_switch: Some(true),
            },
        ];
        assert!((normalize_level(range, None, &values) - 0.6).abs() < 0.001);
        values[1].playback_switch = Some(false);
        assert_eq!(normalize_level(range, None, &values), 0.0);
    }

    #[test]
    fn local_write_feedback_is_suppressed_only_for_matching_recent_echoes() {
        assert!(suppress_local_write_echo(0.42, 10, 0.420_4));
        assert!(!suppress_local_write_echo(0.42, 301, 0.42));
        assert!(!suppress_local_write_echo(0.42, 10, 0.5));
    }
}
