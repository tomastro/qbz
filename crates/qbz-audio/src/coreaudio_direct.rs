//! CoreAudio direct access for macOS
//!
//! Provides device capability probing and sample rate switching on macOS
//! using the coreaudio-rs safe wrappers.
//!
//! Phase 1: Device probing + nominal sample rate switching (shared mode)
//! Phase 2 (future): Hog mode + integer mode + IO proc for bit-perfect playback

#![cfg_attr(target_os = "macos", allow(deprecated))]

#[cfg(target_os = "macos")]
use coreaudio::audio_unit::{macos_helpers, Scope};
#[cfg(target_os = "macos")]
use coreaudio::Error;
#[cfg(target_os = "macos")]
use objc2_core_audio::{
    kAudioDevicePropertyNominalSampleRate, kAudioDevicePropertyVolumeScalar,
    kAudioObjectPropertyElementMaster, kAudioObjectPropertyScopeGlobal,
    kAudioObjectPropertyScopeOutput, AudioObjectGetPropertyData, AudioObjectHasProperty,
    AudioObjectPropertyAddress, AudioObjectSetPropertyData,
};
#[cfg(target_os = "macos")]
use std::{
    mem,
    ptr::{null, NonNull},
};

/// CoreAudio device ID (re-exported so callers don't need objc2_core_audio)
#[cfg(target_os = "macos")]
pub type AudioDeviceID = u32;

// CoreAudio transport type constants (FourCC values from AudioHardware.h)
#[cfg(target_os = "macos")]
mod transport_types {
    pub const BUILT_IN: u32 = 0x626c746e; // 'bltn'
    pub const USB: u32 = 0x75736220; // 'usb '
    pub const BLUETOOTH: u32 = 0x626c7565; // 'blue'
    pub const BLUETOOTH_LE: u32 = 0x626c6561; // 'blea'
    pub const HDMI: u32 = 0x68646d69; // 'hdmi'
    pub const DISPLAY_PORT: u32 = 0x64707274; // 'dprt'
    pub const THUNDERBOLT: u32 = 0x7468756e; // 'thun'
    pub const FIREWIRE: u32 = 0x31333934; // '1394'
    pub const VIRTUAL: u32 = 0x76697274; // 'virt'
    pub const AGGREGATE: u32 = 0x67727570; // 'grup'
}

/// Common audio sample rates to check against device capabilities
#[cfg(target_os = "macos")]
const COMMON_SAMPLE_RATES: &[u32] = &[
    44100, 48000, 88200, 96000, 176400, 192000, 352800, 384000, 705600, 768000,
];

/// Query supported sample rates for a CoreAudio device.
/// Returns discrete rates from the device's available nominal sample rate ranges.
#[cfg(target_os = "macos")]
pub fn query_supported_sample_rates(device_id: AudioDeviceID) -> Result<Vec<u32>, String> {
    let ranges = macos_helpers::get_available_sample_rates(device_id)
        .map_err(|e| format!("Failed to get sample rate ranges: {:?}", e))?;

    let mut rates = Vec::new();
    for range in &ranges {
        if (range.mMinimum - range.mMaximum).abs() < 0.5 {
            // Point value (min == max)
            rates.push(range.mMinimum as u32);
        } else {
            // Continuous range — check which common rates fall within it
            for &rate in COMMON_SAMPLE_RATES {
                let rate_f = rate as f64;
                if rate_f >= range.mMinimum && rate_f <= range.mMaximum {
                    rates.push(rate);
                }
            }
        }
    }

    rates.sort_unstable();
    rates.dedup();
    Ok(rates)
}

/// Set the nominal sample rate of a device.
/// Delegates to coreaudio-rs which handles async confirmation with a 2-second timeout.
#[cfg(target_os = "macos")]
pub fn set_nominal_sample_rate(device_id: AudioDeviceID, target_rate: u32) -> Result<(), String> {
    log::info!(
        "[CoreAudio] Switching sample rate to {}Hz on device {}",
        target_rate,
        device_id
    );

    macos_helpers::set_device_sample_rate(device_id, target_rate as f64)
        .map_err(|e| format!("Failed to set sample rate to {}Hz: {:?}", target_rate, e))?;

    log::info!("[CoreAudio] Sample rate switched to {}Hz", target_rate);
    Ok(())
}

/// Get the current nominal sample rate of a CoreAudio device.
#[cfg(target_os = "macos")]
pub fn get_nominal_sample_rate(device_id: AudioDeviceID) -> Result<u32, String> {
    unsafe {
        let property_address = AudioObjectPropertyAddress {
            mSelector: kAudioDevicePropertyNominalSampleRate,
            mScope: kAudioObjectPropertyScopeGlobal,
            mElement: kAudioObjectPropertyElementMaster,
        };
        let mut rate = 0.0_f64;
        let data_size = mem::size_of::<f64>() as u32;
        let status = AudioObjectGetPropertyData(
            device_id,
            NonNull::from(&property_address),
            0,
            null(),
            NonNull::from(&data_size),
            NonNull::from(&mut rate).cast(),
        );
        Error::from_os_status(status)
            .map_err(|e| format!("Failed to query CoreAudio nominal sample rate: {:?}", e))?;
        if !rate.is_finite() || rate <= 0.0 {
            return Err(format!("Invalid CoreAudio nominal sample rate: {}", rate));
        }
        Ok(rate.round() as u32)
    }
}

/// Get the default output device ID.
#[cfg(target_os = "macos")]
pub fn get_default_output_device() -> Result<AudioDeviceID, String> {
    macos_helpers::get_default_device_id(false)
        .ok_or_else(|| "No default output device found".to_string())
}

/// Get all output device IDs.
#[cfg(target_os = "macos")]
pub fn get_output_device_ids() -> Result<Vec<AudioDeviceID>, String> {
    macos_helpers::get_audio_device_ids_for_scope(Scope::Output)
        .map_err(|e| format!("Failed to enumerate output devices: {:?}", e))
}

/// Get the name of a CoreAudio device.
#[cfg(target_os = "macos")]
pub fn get_device_name(device_id: AudioDeviceID) -> Result<String, String> {
    macos_helpers::get_device_name(device_id)
        .map_err(|e| format!("Failed to get device name: {:?}", e))
}

/// Find a CoreAudio output device ID by its name.
#[cfg(target_os = "macos")]
pub fn find_device_by_name(name: &str) -> Result<Option<AudioDeviceID>, String> {
    // get_device_id_from_name: input=false means output device
    Ok(macos_helpers::get_device_id_from_name(name, false))
}

/// Resolve an optional QBZ output device name to a CoreAudio output device ID.
/// `None` means the current system default output device.
#[cfg(target_os = "macos")]
pub fn resolve_output_device_id(device_name: Option<&str>) -> Result<AudioDeviceID, String> {
    match device_name {
        Some(name) => find_device_by_name(name)?
            .ok_or_else(|| format!("CoreAudio output device '{}' not found", name)),
        None => get_default_output_device(),
    }
}

/// Resolve an optional QBZ output device name to the exact CoreAudio device name.
/// `None` means the current system default output device.
#[cfg(target_os = "macos")]
pub fn resolve_output_device_name(device_name: Option<&str>) -> Result<String, String> {
    let device_id = resolve_output_device_id(device_name)?;
    get_device_name(device_id)
}

/// Return the PID currently owning CoreAudio Hog Mode for this device.
/// CoreAudio uses -1 when no process owns the device.
#[cfg(target_os = "macos")]
pub fn get_hogging_pid(device_id: AudioDeviceID) -> Result<i32, String> {
    macos_helpers::get_hogging_pid(device_id)
        .map(|pid| pid as i32)
        .map_err(|e| format!("Failed to query CoreAudio Hog Mode owner: {:?}", e))
}

/// Enable or disable CoreAudio Hog Mode for a device.
#[cfg(target_os = "macos")]
pub fn set_hog_mode(device_id: AudioDeviceID, enabled: bool) -> Result<(), String> {
    let current_pid = get_hogging_pid(device_id)?;
    let our_pid = std::process::id() as i32;

    if enabled {
        if current_pid == our_pid {
            log::info!(
                "[CoreAudio] Hog Mode already owned by QBZ for device {}",
                device_id
            );
            return Ok(());
        }
        if current_pid != -1 && current_pid != 0 {
            return Err(format!(
                "CoreAudio device {} is already hogged by pid {}",
                device_id, current_pid
            ));
        }

        let new_pid = macos_helpers::toggle_hog_mode(device_id)
            .map(|pid| pid as i32)
            .map_err(|e| format!("Failed to enable CoreAudio Hog Mode: {:?}", e))?;
        if new_pid != our_pid {
            return Err(format!(
                "CoreAudio Hog Mode was not acquired for device {} (owner pid: {})",
                device_id, new_pid
            ));
        }

        log::info!("[CoreAudio] Hog Mode acquired for device {}", device_id);
        return Ok(());
    }

    if current_pid == our_pid {
        let new_pid = macos_helpers::toggle_hog_mode(device_id)
            .map(|pid| pid as i32)
            .map_err(|e| format!("Failed to release CoreAudio Hog Mode: {:?}", e))?;
        log::info!(
            "[CoreAudio] Hog Mode released for device {} (owner pid now {})",
            device_id,
            new_pid
        );
    } else {
        log::debug!(
            "[CoreAudio] Hog Mode release skipped for device {} (owner pid: {})",
            device_id,
            current_pid
        );
    }

    Ok(())
}

/// Get a CoreAudio device's current hardware output volume as scalar 0.0..1.0.
/// Tries master output first, then common stereo channel elements.
#[cfg(target_os = "macos")]
pub fn get_hardware_volume(device_id: AudioDeviceID) -> Result<f32, String> {
    for element in [kAudioObjectPropertyElementMaster, 1, 2] {
        let property_address = AudioObjectPropertyAddress {
            mSelector: kAudioDevicePropertyVolumeScalar,
            mScope: kAudioObjectPropertyScopeOutput,
            mElement: element,
        };

        let has_property =
            unsafe { AudioObjectHasProperty(device_id, NonNull::from(&property_address)) };
        if !has_property {
            continue;
        }

        let mut value = 0.0_f32;
        let data_size = mem::size_of::<f32>() as u32;
        let status = unsafe {
            AudioObjectGetPropertyData(
                device_id,
                NonNull::from(&property_address),
                0,
                null(),
                NonNull::from(&data_size),
                NonNull::from(&mut value).cast(),
            )
        };

        if status == 0 {
            return Ok(value.clamp(0.0, 1.0));
        }
    }

    Err(format!(
        "CoreAudio device {} does not expose readable output hardware volume",
        device_id
    ))
}

/// Set a CoreAudio device's hardware output volume using scalar 0.0..1.0.
/// Tries master output first, then common stereo channel elements.
#[cfg(target_os = "macos")]
pub fn set_hardware_volume(device_id: AudioDeviceID, volume: f32) -> Result<(), String> {
    let clamped = volume.clamp(0.0, 1.0);
    let mut last_error = None;
    let mut channel_success = false;

    for element in [kAudioObjectPropertyElementMaster, 1, 2] {
        let property_address = AudioObjectPropertyAddress {
            mSelector: kAudioDevicePropertyVolumeScalar,
            mScope: kAudioObjectPropertyScopeOutput,
            mElement: element,
        };

        let has_property =
            unsafe { AudioObjectHasProperty(device_id, NonNull::from(&property_address)) };
        if !has_property {
            continue;
        }

        let mut value = clamped;
        let status = unsafe {
            AudioObjectSetPropertyData(
                device_id,
                NonNull::from(&property_address),
                0,
                null(),
                mem::size_of::<f32>() as u32,
                NonNull::new((&mut value as *mut f32).cast()).expect("volume pointer"),
            )
        };

        if status == 0 {
            log::debug!(
                "[CoreAudio] Set hardware volume for device {} element {} to {:.0}%",
                device_id,
                element,
                clamped * 100.0
            );
            if element == kAudioObjectPropertyElementMaster {
                return Ok(());
            }
            channel_success = true;
            continue;
        }

        last_error = Some(status);
    }

    if channel_success {
        return Ok(());
    }

    Err(format!(
        "CoreAudio device {} does not expose settable output hardware volume{}",
        device_id,
        last_error
            .map(|status| format!(" (last OSStatus {})", status))
            .unwrap_or_default()
    ))
}

/// RAII owner for CoreAudio Hog Mode.
///
/// Captures the device's hardware volume on acquire and restores it
/// when released, so leaving Exclusive Mode returns the device to the
/// volume the user had set before QBZ took over.
#[cfg(target_os = "macos")]
#[derive(Debug)]
pub struct CoreAudioExclusiveGuard {
    device_id: AudioDeviceID,
    active: bool,
    original_hardware_volume: Option<f32>,
}

#[cfg(target_os = "macos")]
impl CoreAudioExclusiveGuard {
    /// Acquire CoreAudio Hog Mode for the given device.
    ///
    /// The guard is constructed *before* the FFI call so that any
    /// partial-acquire failure (e.g. CoreAudio transfers ownership to
    /// us but the readback fails) still triggers `Drop`, which calls
    /// `set_hog_mode(false)`. That release is a no-op when we don't
    /// actually own the device, so it's safe in either outcome and
    /// avoids leaving the device hogged on error.
    pub fn acquire(device_id: AudioDeviceID) -> Result<Self, String> {
        // Snapshot the current hardware volume before we touch anything,
        // so the user's pre-Exclusive level can be restored on release.
        // Devices without a readable volume property (knob-only DACs)
        // simply don't get a snapshot — restoration is best-effort.
        let original_hardware_volume = get_hardware_volume(device_id).ok();
        let guard = Self {
            device_id,
            active: true,
            original_hardware_volume,
        };
        set_hog_mode(device_id, true)?;
        Ok(guard)
    }

    pub fn release(&mut self) -> Result<(), String> {
        if !self.active {
            return Ok(());
        }

        // Restore the hardware volume *before* releasing Hog Mode, while
        // we still own the device. After release any other process can
        // change the volume, so doing it before keeps our restoration
        // authoritative.
        if let Some(original) = self.original_hardware_volume.take() {
            if let Err(e) = set_hardware_volume(self.device_id, original) {
                log::warn!(
                    "[CoreAudio] Failed to restore hardware volume on release: {}",
                    e
                );
            }
        }

        set_hog_mode(self.device_id, false)?;
        self.active = false;
        Ok(())
    }

    pub fn set_hardware_volume(&self, volume: f32) -> Result<(), String> {
        set_hardware_volume(self.device_id, volume)
    }
}

#[cfg(target_os = "macos")]
impl Drop for CoreAudioExclusiveGuard {
    fn drop(&mut self) {
        if let Err(e) = self.release() {
            log::warn!("[CoreAudio] Failed to release Hog Mode on drop: {}", e);
        }
    }
}

/// Get the transport type of a device (USB, built-in, Bluetooth, etc.)
#[cfg(target_os = "macos")]
pub fn get_device_transport_type(device_id: AudioDeviceID) -> Option<String> {
    let transport = macos_helpers::get_device_transport_type(device_id).ok()?;

    let transport_str = if transport == transport_types::BUILT_IN {
        "built-in"
    } else if transport == transport_types::USB {
        "usb"
    } else if transport == transport_types::BLUETOOTH || transport == transport_types::BLUETOOTH_LE
    {
        "bluetooth"
    } else if transport == transport_types::HDMI || transport == transport_types::DISPLAY_PORT {
        "hdmi"
    } else if transport == transport_types::THUNDERBOLT {
        "thunderbolt"
    } else if transport == transport_types::FIREWIRE {
        "firewire"
    } else if transport == transport_types::VIRTUAL {
        "virtual"
    } else if transport == transport_types::AGGREGATE {
        "aggregate"
    } else {
        "unknown"
    };

    Some(transport_str.to_string())
}

// ---- Non-macOS stubs ----

/// Query supported sample rates (stub for non-macOS)
#[cfg(not(target_os = "macos"))]
pub fn query_supported_sample_rates(_device_name: &str) -> Result<Vec<u32>, String> {
    Ok(Vec::new())
}

/// Get the current nominal sample rate (stub for non-macOS)
#[cfg(not(target_os = "macos"))]
pub fn get_nominal_sample_rate_by_name(_device_name: &str) -> Result<u32, String> {
    Err("CoreAudio is only available on macOS".to_string())
}

/// Set the nominal sample rate (stub for non-macOS)
#[cfg(not(target_os = "macos"))]
pub fn set_nominal_sample_rate_by_name(
    _device_name: &str,
    _target_rate: u32,
) -> Result<(), String> {
    Err("CoreAudio is only available on macOS".to_string())
}

/// Non-macOS placeholder so shared backend/player signatures can mention the type.
#[cfg(not(target_os = "macos"))]
#[derive(Debug)]
pub struct CoreAudioExclusiveGuard;
