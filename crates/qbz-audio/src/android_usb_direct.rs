//! Adapter from the Android USB Host transport to QBZ's shared DirectSink.

use std::sync::{Arc, OnceLock};

pub use qbz_usb_direct::{AndroidUsbDirectConfig, AndroidUsbDirectStream, PcmFormat};

/// Platform glue installed by the Android frontend.  USB permission,
/// descriptor parsing, interface claiming and UAC clock controls belong to
/// Android's UsbManager edge; the player and the direct writer stay UI-free.
pub type AndroidUsbDirectFactory =
    dyn Fn(u32, u16, u32) -> Result<Arc<dyn crate::backend::DirectSink>, String>
        + Send
        + Sync
        + 'static;

static FACTORY: OnceLock<Arc<AndroidUsbDirectFactory>> = OnceLock::new();

pub fn install_factory(factory: Arc<AndroidUsbDirectFactory>) -> Result<(), String> {
    FACTORY
        .set(factory)
        .map_err(|_| "Android USB Direct factory is already installed".to_string())
}

pub fn open(
    sample_rate: u32,
    channels: u16,
    bit_depth: u32,
) -> Result<Arc<dyn crate::backend::DirectSink>, String> {
    FACTORY
        .get()
        .ok_or_else(|| "Android USB Direct bridge is not initialized".to_string())?
        (sample_rate, channels, bit_depth)
}

impl crate::backend::DirectSink for AndroidUsbDirectStream {
    fn write_f32(&self, samples: &[f32]) -> Result<(), String> {
        AndroidUsbDirectStream::write_f32(self, samples)
    }

    fn drain(&self) -> Result<(), String> {
        AndroidUsbDirectStream::drain(self)
    }

    fn stop(&self) -> Result<(), String> {
        AndroidUsbDirectStream::stop(self)
    }

    fn sample_rate(&self) -> u32 {
        AndroidUsbDirectStream::sample_rate(self)
    }

    fn channels(&self) -> u16 {
        AndroidUsbDirectStream::channels(self)
    }

    fn playback_delay_frames(&self) -> Result<u64, String> {
        AndroidUsbDirectStream::playback_delay_frames(self)
    }

    fn log_label(&self) -> &'static str {
        "Android USB Direct"
    }
}
