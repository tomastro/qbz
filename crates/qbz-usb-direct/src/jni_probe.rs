//! Temporary hardware-validation JNI entry point.
//!
//! The production Android shell will register a persistent route instead. The
//! probe entry point lets the existing diagnostic APK exercise this Rust
//! transport on the real Galaxy/Panasonic pair before UI integration.

use crate::{AndroidUsbDirectConfig, AndroidUsbDirectStream, AndroidUsbFs, PcmFormat};
use jni::objects::JClass;
use jni::sys::{jint, jstring};
use jni::JNIEnv;
use std::f32::consts::TAU;

#[no_mangle]
pub extern "system" fn Java_dev_qbz_usbdirectprobe_RustUsbAudio_playTestTone(
    env: JNIEnv,
    _class: JClass,
    source_fd: jint,
    endpoint_address: jint,
    feedback_endpoint_address: jint,
    max_packet_size: jint,
    interval: jint,
    sample_rate: jint,
    channels: jint,
    subslot_bytes: jint,
    bit_resolution: jint,
    duration_ms: jint,
) -> jstring {
    let result = play_test_tone(
        source_fd,
        endpoint_address,
        feedback_endpoint_address,
        max_packet_size,
        interval,
        sample_rate,
        channels,
        subslot_bytes,
        bit_resolution,
        duration_ms,
    );
    env.new_string(result)
        .map(|value| value.into_raw())
        .unwrap_or(std::ptr::null_mut())
}

#[allow(clippy::too_many_arguments)]
fn play_test_tone(
    source_fd: i32,
    endpoint_address: i32,
    feedback_endpoint_address: i32,
    max_packet_size: i32,
    interval: i32,
    sample_rate: i32,
    channels: i32,
    subslot_bytes: i32,
    bit_resolution: i32,
    duration_ms: i32,
) -> String {
    if source_fd < 0
        || !(1..=255).contains(&endpoint_address)
        || !(0..=255).contains(&feedback_endpoint_address)
        || max_packet_size <= 0
        || !(1..=16).contains(&interval)
        || sample_rate <= 0
        || channels <= 0
        || subslot_bytes <= 0
        || bit_resolution <= 0
        || duration_ms <= 0
    {
        return "FAIL invalid Rust USB probe arguments".to_string();
    }

    let speed_probe = match AndroidUsbFs::duplicate_from_java_fd(source_fd) {
        Ok(usb) => usb,
        Err(error) => return format!("FAIL duplicate fd: {error}"),
    };
    let speed = match speed_probe.usb_speed() {
        Ok(speed) => speed,
        Err(error) => return format!("FAIL USBDEVFS_GET_SPEED: {error}"),
    };
    let divisor = 1_u32 << (interval as u32 - 1);
    let service_rate = if speed >= 3 { 8_000 } else { 1_000 } / divisor;
    if service_rate == 0 {
        return "FAIL invalid USB service rate".to_string();
    }

    let format = PcmFormat {
        sample_rate: sample_rate as u32,
        channels: channels as u16,
        subslot_bytes: subslot_bytes as u8,
        bit_resolution: bit_resolution as u8,
    };
    let stream = match AndroidUsbDirectStream::new(AndroidUsbDirectConfig {
        java_fd: source_fd,
        data_endpoint: endpoint_address as u8,
        feedback_endpoint: feedback_endpoint_address as u8,
        max_packet_size: max_packet_size as usize,
        service_rate,
        format,
    }) {
        Ok(stream) => stream,
        Err(error) => return format!("FAIL create Rust USB stream: {error}"),
    };

    let chunk_frames = (sample_rate as usize).div_ceil(100);
    let chunk_samples = chunk_frames * channels as usize;
    let total_frames = (sample_rate as usize * duration_ms as usize) / 1_000;
    let mut sent_frames = 0usize;
    let mut phase = 0.0_f32;
    let phase_step = TAU * 997.0 / sample_rate as f32;
    let mut pcm = Vec::with_capacity(chunk_samples);
    while sent_frames < total_frames {
        let frames = chunk_frames.min(total_frames - sent_frames);
        pcm.clear();
        for _ in 0..frames {
            let sample = phase.sin() * 0.125_892_54;
            phase = (phase + phase_step) % TAU;
            for _ in 0..channels {
                pcm.push(sample);
            }
        }
        if let Err(error) = stream.write_f32(&pcm) {
            return format!("FAIL Rust USB write: {error}");
        }
        sent_frames += frames;
    }
    if let Err(error) = stream.drain() {
        return format!("FAIL Rust USB drain: {error}");
    }
    format!(
        "PASS rust_usbfs frames={sent_frames} packet_errors=0 usb_speed={speed} service_rate={service_rate}/s feedback=0x{:02X}",
        stream.feedback_endpoint()
    )
}
