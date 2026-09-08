//! PulseAudio backend (legacy compatibility)
//!
//! Similar to PipeWire backend but for systems running PulseAudio only.
//! Uses same approach: pactl + PULSE_SINK + CPAL "pulse" device.

use super::backend::{AudioBackend, AudioBackendType, AudioDevice, BackendConfig, BackendResult};
use super::pipewire_backend::PipeWireBackend;
use rodio::MixerDeviceSink;

pub struct PulseBackend {
    // Reuse PipeWire backend implementation (they're compatible)
    inner: PipeWireBackend,
}

impl PulseBackend {
    pub fn new() -> BackendResult<Self> {
        Ok(Self {
            inner: PipeWireBackend::new()?,
        })
    }
}

impl AudioBackend for PulseBackend {
    fn backend_type(&self) -> AudioBackendType {
        AudioBackendType::Pulse
    }

    fn enumerate_devices(&self) -> BackendResult<Vec<AudioDevice>> {
        // Delegate to PipeWire backend (pactl works for both)
        self.inner.enumerate_devices()
    }

    fn create_output_stream(&self, config: &BackendConfig) -> BackendResult<MixerDeviceSink> {
        // Delegate to PipeWire backend (same mechanism)
        self.inner.create_output_stream(config)
    }

    fn is_available(&self) -> bool {
        // Check if PulseAudio is running
        std::process::Command::new("pactl")
            .arg("info")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    fn description(&self) -> &'static str {
        "PulseAudio (Legacy) - Older audio server, compatible fallback"
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
