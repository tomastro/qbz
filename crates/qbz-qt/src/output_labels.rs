//! The two output LEDs of the now-playing quality stamp — a verbatim port of
//! `fn output_labels` in `crates/qbz/src/settings.rs`.
//!
//! `(backend_label, mode_label, backend_active, mode_active)` is derived
//! PURELY from the persisted `AudioSettings`; the "active" half is what
//! lights the LED. This module only READS AudioSettings — the audio path
//! (qbz-audio / qbz-player) is never touched from here.
//!
//! Published onto `QbzPlayer` (`np_output_*`) from THREE edges, never a poll:
//!   1. `settings_qt::publish_snapshot` — every settings change (and the
//!      Settings document's own republish).
//!   2. the TRACK edge (`playback_qt::refresh_now_playing`) — the moment a
//!      new stream is about to open, which is when the backend + mode are
//!      actually decided. Without this the LEDs only refreshed when the
//!      settings document republished, i.e. when the user changed page.
//!   3. the STREAM edge (`now_playing::set_effective_stream`, deduped inside
//!      on the delivered params) — the first tick after the engine reports
//!      real stream params, so the labels are correct even if the settings
//!      were touched between track start and stream open.
//!
//! Same shape as the Slint, where `apply_snapshot` mirrors the four values
//! onto `NowPlayingState` alongside `SettingsState`.

use cxx_qt_lib::QString;
use qbz_audio::backend::{AlsaPlugin, AudioBackendType};
use qbz_audio::settings::AudioSettings;
use std::sync::{LazyLock, Mutex};

/// Last human-readable device label resolved by Settings' device enumeration.
/// Track/stream edges republish the LED state without enumerating hardware;
/// the settings edge refreshes this cache when the route changes.
#[derive(Clone)]
struct DeviceLabel {
    backend: Option<AudioBackendType>,
    device_id: Option<String>,
    label: String,
}

static DEVICE_LABEL: LazyLock<Mutex<Option<DeviceLabel>>> = LazyLock::new(|| Mutex::new(None));

/// The four LED values. `*_active` = the LED is lit (a deliberate,
/// bit-perfect-capable route) rather than merely named.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutputLabels {
    /// PIPEWIRE | ALSA | JACK | PULS | SYST | AUTO
    pub backend: &'static str,
    /// DACPASS | BITPERF | EXCL | DIRECT | LOCKED | ROUTED | SHARED | DEFAULT
    pub mode: &'static str,
    pub backend_active: bool,
    pub mode_active: bool,
}

impl Default for OutputLabels {
    fn default() -> Self {
        // The Slint NowPlayingState defaults (state.slint).
        Self {
            backend: "SYST",
            mode: "DEFAULT",
            backend_active: false,
            mode_active: false,
        }
    }
}

/// `settings.rs::output_labels`, ported verbatim — this mapping IS the
/// contract for the two LEDs; do not "improve" it.
pub fn output_labels(audio: &AudioSettings) -> OutputLabels {
    let (backend, backend_active) = match audio.backend_type {
        Some(AudioBackendType::PipeWire) => ("PIPEWIRE", true),
        Some(AudioBackendType::Alsa) => ("ALSA", true),
        Some(AudioBackendType::Jack) => ("JACK", true),
        Some(AudioBackendType::Pulse) => ("PULS", true),
        Some(AudioBackendType::WasapiExclusive) => ("WASAPI", true),
        Some(AudioBackendType::SystemDefault) => ("SYST", false),
        None => ("AUTO", false),
    };
    let (mode, mode_active) = match audio.backend_type {
        Some(AudioBackendType::PipeWire) => {
            if audio.dac_passthrough {
                ("DACPASS", true)
            } else if audio.pw_force_bitperfect {
                ("BITPERF", true)
            } else {
                ("SHARED", false)
            }
        }
        Some(AudioBackendType::Alsa) => match audio.alsa_plugin {
            Some(AlsaPlugin::Hw) => {
                if audio.exclusive_mode {
                    ("EXCL", true)
                } else {
                    ("DIRECT", true)
                }
            }
            _ => ("SHARED", false),
        },
        Some(AudioBackendType::Jack) => {
            if audio.reserve_dac_while_running {
                ("LOCKED", true)
            } else {
                ("ROUTED", false)
            }
        }
        Some(AudioBackendType::Pulse) => ("SHARED", false),
        // Exclusive needs a CHOSEN device: with none the player falls through
        // to the shared stream, and lighting EXCL there would be the LED
        // lying, which is the one thing these two must never do.
        Some(AudioBackendType::WasapiExclusive) => {
            if audio.output_device.is_some() {
                ("EXCL", true)
            } else {
                ("SHARED", false)
            }
        }
        Some(AudioBackendType::SystemDefault) | None => ("DEFAULT", false),
    };
    OutputLabels {
        backend,
        mode,
        backend_active,
        mode_active,
    }
}

/// Whether the app's SOFTWARE volume control is inert on the current output
/// route — i.e. moving the slider changes nothing in the signal path.
///
/// This is not a guess: `PlaybackEngine::set_volume`
/// (`qbz-player/src/player/playback_engine.rs`) is a documented NO-OP on a
/// direct sink without hardware volume. ALSA can opt into its DAC mixer via
/// `alsa_hardware_volume`; WASAPI Exclusive deliberately has no such path.
/// Both route predicates include the selected device id: their no-device
/// fallbacks land on CPAL/Rodio and keep software volume.
///
/// READ-ONLY derivation from the persisted `AudioSettings` — no audio
/// behaviour is changed anywhere by this, the UI just stops lying about it.
pub fn volume_locked(audio: &AudioSettings) -> bool {
    let alsa_direct =
        qbz_audio::alsa_direct::uses_alsa_direct_route(audio) && !audio.alsa_hardware_volume;
    let wasapi_exclusive = audio.backend_type == Some(AudioBackendType::WasapiExclusive)
        && audio.output_device.is_some();
    alsa_direct || wasapi_exclusive
}

/// Derive + push the four LED values (and the volume-lock flag) onto the
/// player bridge (Qt-thread hop). Called from `settings_qt::publish_snapshot`
/// AND from the track/stream edges, so the labels follow the settings without
/// a poll of their own.
fn cached_device_label(audio: &AudioSettings) -> String {
    DEVICE_LABEL
        .lock()
        .ok()
        .and_then(|guard| guard.clone())
        .filter(|cached| {
            cached.backend == audio.backend_type && cached.device_id == audio.output_device
        })
        .map(|cached| cached.label)
        .unwrap_or_else(|| {
            audio
                .output_device
                .clone()
                .unwrap_or_else(|| qbz_i18n::t("System default"))
        })
}

fn publish_resolved(audio: &AudioSettings, device_label: String) {
    let l = output_labels(audio);
    let locked = volume_locked(audio);
    let backend_tooltip = format!("{} — {device_label}", l.backend);
    crate::player_bridge::ui(move |mut b| {
        b.as_mut()
            .set_np_output_backend_label(QString::from(l.backend));
        b.as_mut().set_np_output_mode_label(QString::from(l.mode));
        b.as_mut().set_np_output_backend_active(l.backend_active);
        b.as_mut().set_np_output_mode_active(l.mode_active);
        b.as_mut()
            .set_np_output_backend_tooltip(QString::from(backend_tooltip.as_str()));
        b.as_mut().set_np_volume_locked(locked);
    });
}

pub fn publish(audio: &AudioSettings) {
    publish_resolved(audio, cached_device_label(audio));
}

/// Settings has just enumerated the active backend and therefore owns the
/// authoritative display label. Cache it for later track/stream-edge
/// republishes and update the bridge in the same operation.
pub fn publish_with_device_label(audio: &AudioSettings, label: String) {
    if let Ok(mut cached) = DEVICE_LABEL.lock() {
        *cached = Some(DeviceLabel {
            backend: audio.backend_type,
            device_id: audio.output_device.clone(),
            label: label.clone(),
        });
    }
    publish_resolved(audio, label);
}

/// Re-derive from the LIVE audio settings and publish. This is the TRACK /
/// STREAM edge entry point: the settings store is the source of truth for
/// which backend + mode the engine is about to use, so reading it at the
/// moment a stream opens costs one cheap WAL read and no settings-document
/// rebuild (`publish_snapshot` is the expensive one — device enumeration,
/// integrations, the whole Settings JSON; it must NOT be called from here).
pub fn publish_current() {
    publish(&crate::settings_qt::audio_settings());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(backend: Option<AudioBackendType>) -> AudioSettings {
        AudioSettings {
            backend_type: backend,
            ..AudioSettings::default()
        }
    }

    #[test]
    fn auto_backend_is_the_unlit_default() {
        let l = output_labels(&settings(None));
        assert_eq!((l.backend, l.mode), ("AUTO", "DEFAULT"));
        assert!(!l.backend_active && !l.mode_active);
    }

    #[test]
    fn pipewire_modes() {
        let mut s = settings(Some(AudioBackendType::PipeWire));
        s.dac_passthrough = false;
        s.pw_force_bitperfect = false;
        assert_eq!(output_labels(&s).mode, "SHARED");
        assert!(!output_labels(&s).mode_active);
        s.pw_force_bitperfect = true;
        assert_eq!(output_labels(&s).mode, "BITPERF");
        // dac_passthrough wins over pw_force_bitperfect.
        s.dac_passthrough = true;
        let l = output_labels(&s);
        assert_eq!((l.backend, l.mode), ("PIPEWIRE", "DACPASS"));
        assert!(l.backend_active && l.mode_active);
    }

    #[test]
    fn alsa_hw_is_direct_or_exclusive_and_plug_is_shared() {
        let mut s = settings(Some(AudioBackendType::Alsa));
        s.alsa_plugin = Some(AlsaPlugin::Hw);
        s.exclusive_mode = false;
        let l = output_labels(&s);
        assert_eq!((l.backend, l.mode), ("ALSA", "DIRECT"));
        assert!(l.mode_active);
        s.exclusive_mode = true;
        assert_eq!(output_labels(&s).mode, "EXCL");
        s.alsa_plugin = Some(AlsaPlugin::PlugHw);
        let l = output_labels(&s);
        assert_eq!(l.mode, "SHARED");
        assert!(!l.mode_active);
    }

    #[test]
    fn jack_reservation_lights_the_mode_led() {
        let mut s = settings(Some(AudioBackendType::Jack));
        s.reserve_dac_while_running = false;
        assert_eq!(output_labels(&s).mode, "ROUTED");
        assert!(!output_labels(&s).mode_active);
        s.reserve_dac_while_running = true;
        assert_eq!(output_labels(&s).mode, "LOCKED");
        assert!(output_labels(&s).mode_active);
    }

    #[test]
    fn volume_is_locked_only_on_direct_engines_without_a_usable_mixer() {
        // Shared/routed backends keep software volume.
        for b in [
            None,
            Some(AudioBackendType::PipeWire),
            Some(AudioBackendType::Pulse),
            Some(AudioBackendType::Jack),
            Some(AudioBackendType::SystemDefault),
        ] {
            assert!(!volume_locked(&settings(b)), "{b:?} must keep the slider");
        }
        let mut s = settings(Some(AudioBackendType::Alsa));
        s.output_device = Some("front:CARD=USB,DEV=0".to_string());
        // hw = the bit-perfect direct path: engine set_volume is a no-op.
        s.alsa_plugin = Some(AlsaPlugin::Hw);
        s.alsa_hardware_volume = false;
        assert_eq!(volume_locked(&s), cfg!(target_os = "linux"));
        // ...unless the DAC's own mixer is driven instead.
        s.alsa_hardware_volume = true;
        assert!(!volume_locked(&s));
        // plughw still runs the AlsaDirect engine -> still inert.
        s.alsa_hardware_volume = false;
        s.alsa_plugin = Some(AlsaPlugin::PlugHw);
        assert_eq!(volume_locked(&s), cfg!(target_os = "linux"));
        // Pcm opts out of the direct engine (CPAL/rodio) -> software volume.
        s.alsa_plugin = Some(AlsaPlugin::Pcm);
        assert!(!volume_locked(&s));
        // ALSA default/sysdefault use CPAL/Rodio rather than AlsaDirect.
        s.alsa_plugin = Some(AlsaPlugin::Hw);
        s.output_device = None;
        assert!(!volume_locked(&s));
        s.output_device = Some("sysdefault:CARD=USB".to_string());
        assert!(!volume_locked(&s));

        // WASAPI Exclusive is another DirectSink and intentionally rules out
        // endpoint volume. Without a chosen endpoint it falls back to shared
        // CPAL, so that exact fallback keeps the slider live.
        let mut wasapi = settings(Some(AudioBackendType::WasapiExclusive));
        assert!(!volume_locked(&wasapi));
        wasapi.output_device = Some("{endpoint-id}".to_string());
        assert!(volume_locked(&wasapi));
        wasapi.alsa_hardware_volume = true;
        assert!(volume_locked(&wasapi));
    }

    #[test]
    fn pulse_and_system_default() {
        let l = output_labels(&settings(Some(AudioBackendType::Pulse)));
        assert_eq!((l.backend, l.mode), ("PULS", "SHARED"));
        assert!(l.backend_active && !l.mode_active);
        let l = output_labels(&settings(Some(AudioBackendType::SystemDefault)));
        assert_eq!((l.backend, l.mode), ("SYST", "DEFAULT"));
        assert!(!l.backend_active && !l.mode_active);
    }
}
