//! WASAPI exclusive-mode output (Windows) — the bit-perfect path.
//!
//! Shape: `alsa_direct.rs` on the OUTSIDE, so `PlaybackEngine` can clone the
//! arm it already has for ALSA. NOT on the inside: WASAPI's COM objects are
//! `!Send`, so exactly one render thread owns every COM handle and this struct
//! is a channel plus atomics.
//!
//! Everything above the `#[cfg(windows)]` block is pure and compiles on every
//! host, which is the point — the format ladder, the PCM packing and the
//! period arithmetic are where the platform-specific mistakes live, and they
//! are testable on the machine that runs CI as well as on the one that ships.
//!
//! # The one that bites
//!
//! Windows 24-in-32 is **LEFT-aligned**: the 24 valid bits sit in the HIGH
//! bytes of the 32-bit container. ALSA's `S24_LE` is right-aligned. Porting
//! the ALSA packing verbatim shifts every sample down 8 bits and plays 48 dB
//! quiet, which sounds like a volume bug rather than a format bug — so
//! `s24_in_32_is_left_aligned_with_zero_low_byte` exists to fail loudly.

use crate::backend::BitPerfectMode;

/// A format rung, in the order the ladder tries them.
///
/// `S24Packed` first: it is the shape a DAC that wants 24-bit really wants,
/// and some USB Audio Class devices accept only it. `F32` last and separate —
/// see [`Rung::bit_perfect_mode`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rung {
    /// 3 bytes per sample, no padding. ALSA's `S24_3LE`.
    S24Packed,
    /// 32-bit container, 32 valid bits.
    S32,
    /// 32-bit container, 24 valid bits, LEFT-aligned (Windows' own shape).
    S24In32,
    S16,
    /// 32-bit IEEE float. A COMPATIBILITY rung: the device mixes rather than
    /// takes the samples as given, so it is never bit-perfect.
    F32,
}

/// The order `WasapiDirectStream::new` probes. Measured on the owner's
/// Cambridge Audio USB Audio 2.0 (research/05-spike-results.md): only
/// `S24In32` is accepted, at 44.1/48/88.2/96/192 kHz — so on that DAC the
/// ladder is one rung deep. Other devices differ, which is why it stays a
/// ladder.
pub const LADDER: [Rung; 5] = [
    Rung::S24Packed,
    Rung::S32,
    Rung::S24In32,
    Rung::S16,
    Rung::F32,
];

impl Rung {
    pub fn container_bits(self) -> u16 {
        match self {
            Rung::S24Packed => 24,
            Rung::S16 => 16,
            _ => 32,
        }
    }

    pub fn valid_bits(self) -> u16 {
        match self {
            Rung::S24Packed | Rung::S24In32 => 24,
            Rung::S16 => 16,
            _ => 32,
        }
    }

    pub fn is_float(self) -> bool {
        matches!(self, Rung::F32)
    }

    /// Bytes per FRAME, i.e. per sample across every channel.
    pub fn block_align(self, channels: u16) -> u16 {
        self.container_bits() / 8 * channels
    }

    /// The float rung means the device took floats and will convert them, so
    /// the samples that reach the DAC are not the ones we produced.
    pub fn bit_perfect_mode(self) -> BitPerfectMode {
        if self.is_float() {
            BitPerfectMode::Disabled
        } else {
            BitPerfectMode::DirectHardware
        }
    }
}

/// f32 samples to the wire bytes of one rung, appended to `out`.
///
/// Scaling matches `alsa_direct.rs` so the two backends cannot disagree about
/// what full scale is. The `S24In32` arm is the one that differs, and it
/// differs on purpose: `v << 8`, low byte zero.
pub fn pack_f32(samples: &[f32], rung: &Rung, out: &mut Vec<u8>) {
    match rung {
        Rung::S16 => {
            for &s in samples {
                let v = (s.clamp(-1.0, 1.0) * 32767.0).round() as i16;
                out.extend_from_slice(&v.to_le_bytes());
            }
        }
        Rung::S24Packed => {
            for &s in samples {
                let v = (s.clamp(-1.0, 1.0) * 8_388_607.0).round() as i32;
                out.extend_from_slice(&v.to_le_bytes()[..3]);
            }
        }
        Rung::S24In32 => {
            for &s in samples {
                let v = (s.clamp(-1.0, 1.0) * 8_388_607.0).round() as i32;
                // LEFT-aligned: the valid 24 bits occupy the HIGH bytes.
                out.extend_from_slice(&(v << 8).to_le_bytes());
            }
        }
        Rung::S32 => {
            for &s in samples {
                let v = (s.clamp(-1.0, 1.0) as f64 * i32::MAX as f64).round() as i32;
                out.extend_from_slice(&v.to_le_bytes());
            }
        }
        Rung::F32 => {
            for &s in samples {
                out.extend_from_slice(&s.to_le_bytes());
            }
        }
    }
}

/// 100-ns units to frames, rounded to nearest.
pub fn hns_to_frames(hns: i64, rate: u32) -> u32 {
    ((hns as f64 * rate as f64 / 10_000_000.0) + 0.5) as u32
}

/// Frames to 100-ns units, rounded to nearest.
pub fn frames_to_hns(frames: u32, rate: u32) -> i64 {
    ((frames as f64 * 10_000_000.0 / rate as f64) + 0.5) as i64
}

/// Round a period up so `frames * block_align` is a whole number of 128-byte
/// blocks.
///
/// This is pre-alignment: `AUDCLNT_E_BUFFER_SIZE_NOT_ALIGNED` is what an
/// unaligned period earns, and recovering from it costs a `GetBufferSize`, a
/// fresh `IAudioClient` and a second `Initialize`. Asking for an aligned
/// period in the first place means the error usually never fires — measured on
/// the owner's DAC, it did not fire at all. Keep the retry anyway: Intel HDA
/// devices are the documented case that still needs it.
pub fn aligned_period_hns(requested_hns: i64, rate: u32, block_align: u16) -> i64 {
    let frames = hns_to_frames(requested_hns, rate).max(1);
    let bytes = frames as u64 * block_align as u64;
    let aligned_bytes = bytes.div_ceil(128) * 128;
    let aligned_frames = (aligned_bytes / block_align as u64) as u32;
    frames_to_hns(aligned_frames, rate)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The -48 dB trap. Windows puts the 24 valid bits in the HIGH bytes;
    /// porting ALSA's right-aligned S24_LE here plays quiet, not broken, which
    /// is the kind of defect that survives a listening test.
    #[test]
    fn s24_in_32_is_left_aligned_with_zero_low_byte() {
        let mut out = Vec::new();
        pack_f32(&[1.0, -1.0, 0.0], &Rung::S24In32, &mut out);
        let w: Vec<i32> = out
            .chunks(4)
            .map(|b| i32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .collect();
        assert_eq!(w[0], 8_388_607 << 8);
        assert_eq!(w[1], -8_388_607 << 8);
        assert_eq!(w[2], 0);
        assert!(
            out.chunks(4).all(|b| b[0] == 0),
            "the low byte of every container must be zero"
        );
    }

    #[test]
    fn s24_packed_matches_alsa_s24_3le_byte_order() {
        let mut out = Vec::new();
        pack_f32(&[1.0], &Rung::S24Packed, &mut out);
        assert_eq!(out, vec![0xFF, 0xFF, 0x7F]); // little-endian 0x7FFFFF
    }

    #[test]
    fn s32_and_s16_scale_like_alsa_direct() {
        let mut out = Vec::new();
        pack_f32(&[1.0], &Rung::S32, &mut out);
        assert_eq!(
            i32::from_le_bytes([out[0], out[1], out[2], out[3]]),
            i32::MAX
        );
        out.clear();
        pack_f32(&[-1.0], &Rung::S16, &mut out);
        assert_eq!(i16::from_le_bytes([out[0], out[1]]), -32767);
    }

    #[test]
    fn packing_is_clamped_not_wrapped() {
        // A sample above full scale must saturate. Wrapping would invert the
        // waveform's peaks, which is audible as a click, not as clipping.
        let mut out = Vec::new();
        pack_f32(&[2.0, -2.0], &Rung::S16, &mut out);
        assert_eq!(i16::from_le_bytes([out[0], out[1]]), 32767);
        assert_eq!(i16::from_le_bytes([out[2], out[3]]), -32767);
    }

    #[test]
    fn ladder_order_and_geometry() {
        assert_eq!(
            LADDER,
            [
                Rung::S24Packed,
                Rung::S32,
                Rung::S24In32,
                Rung::S16,
                Rung::F32
            ]
        );
        assert_eq!(Rung::S24Packed.block_align(2), 6);
        assert_eq!(Rung::S24In32.block_align(2), 8);
        assert_eq!(Rung::S16.block_align(2), 4);
        assert_eq!(Rung::S24In32.valid_bits(), 24);
        assert_eq!(Rung::S24In32.container_bits(), 32);
        assert_eq!(Rung::S24Packed.valid_bits(), 24);
        assert_eq!(Rung::S24Packed.container_bits(), 24);
    }

    #[test]
    fn float_rung_is_not_bit_perfect() {
        assert_eq!(Rung::F32.bit_perfect_mode(), BitPerfectMode::Disabled);
        assert_eq!(
            Rung::S24Packed.bit_perfect_mode(),
            BitPerfectMode::DirectHardware
        );
        assert_eq!(
            Rung::S24In32.bit_perfect_mode(),
            BitPerfectMode::DirectHardware
        );
    }

    #[test]
    fn aligned_period_is_a_multiple_of_128_bytes() {
        // 10 ms at 44.1k stereo 24-packed: 441 frames * 6 B = 2646 B, which is
        // not a whole number of 128-byte blocks.
        let hns = aligned_period_hns(100_000, 44100, 6);
        let frames = hns_to_frames(hns, 44100);
        assert_eq!(frames as u64 * 6 % 128, 0);
        assert!(
            (frames as i64 - 441).abs() <= 22,
            "stay near the requested period, got {frames} frames"
        );
    }

    #[test]
    fn aligned_period_leaves_an_already_aligned_one_alone() {
        // 3 ms at 192k stereo 24-in-32: 576 frames * 8 B = 4608 B = 36 blocks.
        // This is the owner's DAC's minimum period, measured; it must not move.
        let hns = aligned_period_hns(30_000, 192_000, 8);
        assert_eq!(hns_to_frames(hns, 192_000), 576);
    }

    #[test]
    fn period_conversions_round_trip() {
        for (rate, frames) in [(44100u32, 441u32), (192_000, 576), (96_000, 960)] {
            let hns = frames_to_hns(frames, rate);
            assert_eq!(hns_to_frames(hns, rate), frames, "rate {rate}");
        }
    }
}

// ===========================================================================
// Public handle
// ===========================================================================

/// How the render thread is woken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WasapiTiming {
    /// The device signals an event each period. Lower latency, and what the
    /// spike measured as glitch-free on the owner's DAC at the minimum period.
    Events,
    /// The thread sleeps and polls the padding. More devices tolerate it, and
    /// it avoids the stuttering some USB-class drivers show in event mode.
    Polling,
}

/// Everything the opened stream settled on. Logged once, and read by the
/// backend so the UI can say what actually happened.
#[derive(Debug, Clone)]
pub struct WasapiOpenInfo {
    pub endpoint_name: String,
    pub rate: u32,
    pub channels: u16,
    pub rung: Rung,
    pub period_hns: i64,
    pub buffer_frames: u32,
    /// True when AUDCLNT_E_BUFFER_SIZE_NOT_ALIGNED fired and the client had to
    /// be rebuilt at the device's own aligned size.
    pub realigned: bool,
    pub timing: WasapiTiming,
}

/// A live exclusive-mode stream.
///
/// The COM objects live on one render thread; this is the feeder side. Every
/// method here is a channel send or an atomic read, which is what makes the
/// handle `Send + Sync` even though `IAudioClient` is not.
pub struct WasapiDirectStream {
    #[cfg(windows)]
    inner: imp::Inner,
    #[cfg(not(windows))]
    _unused: (),
}

// ===========================================================================
// Windows
// ===========================================================================

#[cfg(windows)]
mod imp {
    use super::{aligned_period_hns, Rung, WasapiOpenInfo, WasapiTiming, LADDER};
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::mpsc::{
        channel, sync_channel, Receiver, RecvTimeoutError, Sender, SyncSender, TryRecvError,
    };
    use std::sync::Arc;
    use std::thread::JoinHandle;
    use std::time::{Duration, Instant};
    use wasapi::{
        initialize_mta, AudioClient, BufferFlags, DeviceEnumerator, Direction, SampleType,
        ShareMode, StreamMode, WaveFormat,
    };

    /// Out-of-band commands. PCM travels on its own channel so a pause never
    /// waits behind eight queued buffers.
    pub(super) enum Ctl {
        Pause,
        Resume,
        /// Carries the reply channel the caller blocks on. `drain()` must not
        /// return until the queue really has played out, because the player's
        /// writer thread treats its return as "the source is finished" - and
        /// the ALSA drain it shares that thread with does block.
        /// `true` = the queue really played out; `false` = a Stop threw it
        /// away. The caller must be able to tell those apart.
        Drain(Sender<bool>),
        /// Halt and discard, then WAIT for the next track -- ALSA's
        /// "prepare for next playback". The render thread survives it,
        /// because the player reuses one stream across track changes.
        /// Carries the acknowledgement channel. `stop()` MUST NOT return
        /// before the halt-and-purge has run: the player sends Stop and then
        /// immediately queues the next track's PCM on the other channel, so a
        /// fire-and-forget Stop lets the purge below eat audio belonging to
        /// the track that is starting. Same failure as the original bug, one
        /// track later and intermittent.
        Stop(Sender<()>),
        /// Real teardown: leave the loop and let the thread end. Only `Drop`
        /// sends this. Keeping it separate from `Stop` is the whole point --
        /// when `Stop` also meant "exit", `Drop` joined a thread that had
        /// already gone, and the day `Stop` stopped meaning that, `Drop`
        /// joined a thread that never would.
        Shutdown,
    }

    pub(super) struct Inner {
        pcm: SyncSender<Vec<u8>>,
        ctl: Sender<Ctl>,
        dead: Arc<AtomicBool>,
        underruns: Arc<AtomicU64>,
        timeouts: Arc<AtomicU64>,
        info: WasapiOpenInfo,
        join: Option<JoinHandle<()>>,
    }

    /// Depth of the PCM queue, in periods. Small on purpose: this IS the
    /// back-pressure, the same role `snd_pcm_writei` blocking plays for ALSA.
    /// Too deep and a seek keeps playing stale audio; too shallow and any
    /// scheduling hiccup underruns.
    const PCM_QUEUE_PERIODS: usize = 8;

    /// A drain cannot wait forever on a device that stopped consuming.
    const DRAIN_DEADLINE: Duration = Duration::from_secs(10);
    /// A Stop is acknowledged from the top of the render loop, so the wait is
    /// at most one event timeout plus the halt itself. Generous by 10x, and
    /// bounded so a dead device cannot wedge a track change.
    const STOP_DEADLINE: Duration = Duration::from_secs(2);

    fn wave_format(rung: Rung, rate: u32, channels: u16) -> WaveFormat {
        let ty = if rung.is_float() {
            SampleType::Float
        } else {
            SampleType::Int
        };
        WaveFormat::new(
            rung.container_bits() as usize,
            rung.valid_bits() as usize,
            &ty,
            rate as usize,
            channels as usize,
            None,
        )
    }

    /// Raise the render thread to the "Pro Audio" MMCSS class.
    ///
    /// Best effort: a failure costs scheduling headroom, not correctness, and
    /// the spike measured zero timeouts for ten minutes WITHOUT it, so this is
    /// margin rather than a requirement.
    fn join_pro_audio() {
        use windows::core::PCWSTR;
        use windows::Win32::System::Threading::AvSetMmThreadCharacteristicsW;
        let name: Vec<u16> = "Pro Audio"
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let mut index: u32 = 0;
        // SAFETY: `name` is NUL-terminated and outlives the call; `index` is a
        // valid out-slot. The returned handle is intentionally not revoked -
        // the thread keeps the class for its whole life and dies with it.
        let ok = unsafe { AvSetMmThreadCharacteristicsW(PCWSTR(name.as_ptr()), &mut index) };
        match ok {
            Ok(_) => log::debug!("[WASAPI Direct] render thread joined MMCSS Pro Audio"),
            Err(e) => log::warn!("[WASAPI Direct] MMCSS Pro Audio unavailable ({e}); continuing"),
        }
    }

    /// What the render thread reports back once it has (or has not) opened.
    type OpenReply = Result<WasapiOpenInfo, String>;

    /// Open the endpoint, walking the ladder. Runs ON the render thread,
    /// because every object it returns is a COM object and those are `!Send`.
    fn open_exclusive(
        endpoint_id: &str,
        rate: u32,
        channels: u16,
        timing: WasapiTiming,
    ) -> Result<(AudioClient, WasapiOpenInfo, Option<wasapi::Handle>), String> {
        let enumerator = DeviceEnumerator::new().map_err(|e| format!("DeviceEnumerator: {e}"))?;
        let collection = enumerator
            .get_device_collection(&Direction::Render)
            .map_err(|e| format!("device collection: {e}"))?;
        let count = collection
            .get_nbr_devices()
            .map_err(|e| format!("device count: {e}"))?;

        // By ENDPOINT ID, never by name: Windows hands several endpoints the
        // same friendly name (measured on this box: three called "Altavoces",
        // one of them the DAC).
        let mut found = None;
        for i in 0..count {
            let d = collection
                .get_device_at_index(i)
                .map_err(|e| format!("device {i}: {e}"))?;
            if d.get_id().map(|id| id == endpoint_id).unwrap_or(false) {
                found = Some(d);
                break;
            }
        }
        let device = found.ok_or_else(|| format!("endpoint '{endpoint_id}' not found"))?;
        let endpoint_name = device
            .get_friendlyname()
            .unwrap_or_else(|_| "<unnamed>".to_string());

        // --- the ladder -------------------------------------------------
        let mut chosen: Option<(Rung, WaveFormat)> = None;
        for rung in LADDER {
            let wf = wave_format(rung, rate, channels);
            let client = device
                .get_iaudioclient()
                .map_err(|e| format!("IAudioClient: {e}"))?;
            match client.is_supported(&wf, &ShareMode::Exclusive) {
                Ok(None) => {
                    chosen = Some((rung, wf));
                    break;
                }
                // The docs promise exclusive mode never answers S_FALSE. If a
                // driver does it anyway, say so loudly and treat it as a no:
                // a "closest match" is by definition not the format we asked
                // for, and accepting it would silently stop being bit-perfect.
                Ok(Some(_)) => log::error!(
                    "[WASAPI Direct] {endpoint_name}: {rung:?} @ {rate} answered S_FALSE in \
                     EXCLUSIVE mode, which the documentation says cannot happen; treating as \
                     unsupported"
                ),
                Err(_) => {}
            }
        }
        let (rung, wf) = chosen.ok_or_else(|| {
            format!("{endpoint_name} accepts no exclusive format at {rate} Hz, {channels} ch")
        })?;
        let block_align = rung.block_align(channels);

        // --- period ------------------------------------------------------
        let probe = device
            .get_iaudioclient()
            .map_err(|e| format!("IAudioClient: {e}"))?;
        let (_default_hns, min_hns) = probe
            .get_device_period()
            .map_err(|e| format!("device period: {e}"))?;
        drop(probe);
        let want_hns = aligned_period_hns(min_hns, rate, block_align);

        let mode = |hns: i64| match timing {
            WasapiTiming::Events => StreamMode::EventsExclusive { period_hns: hns },
            WasapiTiming::Polling => StreamMode::PollingExclusive {
                buffer_duration_hns: hns,
                period_hns: hns,
            },
        };

        // A fresh client per attempt: an IAudioClient that failed Initialize
        // cannot be re-initialised, which is the whole reason the documented
        // NOT_ALIGNED recovery says to Activate a new one.
        let mut client = device
            .get_iaudioclient()
            .map_err(|e| format!("IAudioClient: {e}"))?;
        let mut realigned = false;
        let mut period_hns = want_hns;
        if let Err(first) = client.initialize_client(&wf, &Direction::Render, &mode(want_hns)) {
            let msg = first.to_string();
            // AUDCLNT_E_BUFFER_SIZE_NOT_ALIGNED (0x88890019). Pre-alignment
            // usually prevents it - it did not fire once on the owner's DAC -
            // but Intel HDA parts still need the recovery.
            if !msg.contains("88890019") {
                return Err(format!("Initialize(EXCLUSIVE) failed: {msg}"));
            }
            let frames = client
                .get_buffer_size()
                .map_err(|e| format!("GetBufferSize after NOT_ALIGNED: {e}"))?;
            period_hns = super::frames_to_hns(frames, rate);
            realigned = true;
            client = device
                .get_iaudioclient()
                .map_err(|e| format!("IAudioClient (realign): {e}"))?;
            client
                .initialize_client(&wf, &Direction::Render, &mode(period_hns))
                .map_err(|e| format!("Initialize(EXCLUSIVE) after realign: {e}"))?;
        }

        let handle = match timing {
            WasapiTiming::Events => Some(
                client
                    .set_get_eventhandle()
                    .map_err(|e| format!("event handle: {e}"))?,
            ),
            WasapiTiming::Polling => None,
        };
        let buffer_frames = client
            .get_buffer_size()
            .map_err(|e| format!("GetBufferSize: {e}"))?;

        let info = WasapiOpenInfo {
            endpoint_name,
            rate,
            channels,
            rung,
            period_hns,
            buffer_frames,
            realigned,
            timing,
        };
        Ok((client, info, handle))
    }

    /// The render thread. Owns every COM object for the life of the stream.
    fn render_thread(
        endpoint_id: String,
        rate: u32,
        channels: u16,
        timing: WasapiTiming,
        reply: Sender<OpenReply>,
        pcm: Receiver<Vec<u8>>,
        ctl: Receiver<Ctl>,
        dead: Arc<AtomicBool>,
        underruns: Arc<AtomicU64>,
        timeouts: Arc<AtomicU64>,
    ) {
        if let Err(e) = initialize_mta().ok() {
            let _ = reply.send(Err(format!("CoInitializeEx(MTA): {e}")));
            return;
        }
        let (client, info, handle) = match open_exclusive(&endpoint_id, rate, channels, timing) {
            Ok(v) => v,
            Err(e) => {
                let _ = reply.send(Err(e));
                return;
            }
        };
        let block_align = info.rung.block_align(info.channels) as usize;
        let render = match client.get_audiorenderclient() {
            Ok(r) => r,
            Err(e) => {
                let _ = reply.send(Err(format!("IAudioRenderClient: {e}")));
                return;
            }
        };

        join_pro_audio();

        // The one line every later debugging session starts from.
        log::info!(
            "[WASAPI Direct] Initialized EXCLUSIVE: {} @ {} Hz, {} ch, {:?} ({}/{} bits), \
             period={} hns, buffer={} frames, realigned={}, timing={:?}",
            info.endpoint_name,
            info.rate,
            info.channels,
            info.rung,
            info.rung.valid_bits(),
            info.rung.container_bits(),
            info.period_hns,
            info.buffer_frames,
            info.realigned,
            info.timing,
        );
        let period_ms = (info.period_hns / 10_000).max(1) as u32;
        // Half a period of slack for the feeder. The device is holding a
        // whole period, so this can never make the stream late.
        let slack_us = ((info.period_hns / 10) / 2).max(200) as u64;
        let event_timeout_ms = period_ms.saturating_mul(8).max(200);
        let _ = reply.send(Ok(info.clone()));

        let mut carry: VecDeque<u8> = VecDeque::with_capacity(block_align * 4096);

        // Do NOT start until real audio is in hand. The reply above is what
        // lets the caller write at all, so starting before it lands guarantees
        // the first periods are empty - measured as exactly 2 underruns every
        // run, which is a fault this code invented rather than one it found.
        // Playback now begins on audio, and the device buffer is pre-rolled
        // from it.
        let first_deadline = Instant::now() + Duration::from_secs(5);
        while carry.is_empty() && Instant::now() < first_deadline {
            match ctl.try_recv() {
                Ok(Ctl::Shutdown) | Err(TryRecvError::Disconnected) => return,
                // Nothing has started yet, so a Stop has nothing to halt;
                // discard whatever arrived and keep waiting for real audio.
                Ok(Ctl::Stop(ack)) => {
                    carry.clear();
                    let _ = ack.send(());
                }
                _ => {}
            }
            if let Ok(chunk) = pcm.recv_timeout(Duration::from_millis(20)) {
                carry.extend(chunk);
            }
        }
        if let Ok(n) = client.get_available_space_in_frames() {
            let need = n as usize * block_align;
            let take = need.min(carry.len());
            let mut buf: Vec<u8> = carry.drain(..take).collect();
            buf.resize(need, 0);
            let _ = render.write_to_device(n as usize, &buf, None);
        }
        if let Err(e) = client.start_stream() {
            log::error!("[WASAPI Direct] Start failed: {e}");
            dead.store(true, Ordering::SeqCst);
            return;
        }
        let mut paused = false;
        // Halted by Ctl::Stop and waiting for the next track's first chunk.
        // NOT a teardown: see the Ctl::Stop arm below.
        let mut stopped = false;
        let mut draining: Option<(Instant, Sender<bool>)> = None;
        // Starvation is counted as an EVENT, not per period: one real underrun
        // otherwise reports as hundreds while the queue stays empty.
        let mut starved = false;

        loop {
            // Control first: a pause must not wait behind queued PCM.
            match ctl.try_recv() {
                Ok(Ctl::Pause) => {
                    if !paused {
                        let _ = client.stop_stream();
                        paused = true;
                    }
                }
                Ok(Ctl::Resume) => {
                    if paused {
                        paused = false;
                        // While `stopped` the client is Reset and unprimed,
                        // and the re-arm block below owns the restart: it
                        // pre-rolls BEFORE Start. Starting here as well would
                        // play a buffer of silence and then, on the second
                        // Start, fail with AUDCLNT_E_NOT_STOPPED.
                        if !stopped {
                            let _ = client.start_stream();
                        }
                    }
                }
                Ok(Ctl::Drain(done)) => {
                    if stopped {
                        // A halted, Reset stream holds nothing and plays
                        // nothing, so it has drained by definition. Parking
                        // this until the NEXT track arrives would block the
                        // caller for the whole drain deadline.
                        let _ = done.send(true);
                    } else {
                        draining = Some((Instant::now() + DRAIN_DEADLINE, done));
                    }
                }
                Ok(Ctl::Stop(ack)) => {
                    // ALSA's contract, which this mirrors deliberately:
                    // "Stop PCM immediately (prepare for next playback)" --
                    // DROP to halt and discard, then prepare, and the stream
                    // OBJECT survives because the player reuses it for the next
                    // track. Breaking here instead made the stream single-use:
                    // the player calls engine.stop() on every track change, so
                    // the render thread died before the first track's audio had
                    // even finished buffering and every later write answered
                    // "render thread is gone". Same shape as the drain defect,
                    // second instance -- a control that means "get ready" was
                    // read as "we are done".
                    // The results are NOT ignored. If the halt half fails the
                    // stream is not in a state a later Start can trust, and
                    // reporting it as stopped-and-ready would be the same kind
                    // of lie this whole fix exists to remove.
                    let halted = client
                        .stop_stream()
                        .map_err(|e| format!("stop_stream: {e}"))
                        .and_then(|()| {
                            client
                                .reset_stream()
                                .map_err(|e| format!("reset_stream: {e}"))
                        });
                    carry.clear();
                    // Discard what the previous track had already handed over,
                    // the way DROP discards queued frames. Safe here and ONLY
                    // here because `stop()` blocks on the ack below: nothing
                    // written after stop() returns can be in flight yet.
                    while pcm.try_recv().is_ok() {}
                    // A drain waiting on this stream will never see its audio
                    // play out -- it was just thrown away. Say so.
                    if let Some((_, done)) = draining.take() {
                        let _ = done.send(false);
                    }
                    starved = false;
                    paused = false;
                    stopped = true;
                    // Acknowledge before any bail-out, so a caller blocked in
                    // stop() is never stranded by a failure.
                    let _ = ack.send(());
                    if let Err(e) = halted {
                        log::error!("[WASAPI Direct] halt failed ({e}); stream is dead");
                        dead.store(true, Ordering::SeqCst);
                        break;
                    }
                }
                Ok(Ctl::Shutdown) | Err(TryRecvError::Disconnected) => break,
                Err(TryRecvError::Empty) => {}
            }

            // Re-arm: hold the device open, idle, until the next track's first
            // chunk arrives, then pre-roll and start again exactly as the
            // initial start did. Keeping the client open is the point -- the
            // exclusive lock stays ours across the track change.
            if stopped && !paused {
                match pcm.recv_timeout(Duration::from_millis(20)) {
                    Ok(chunk) => carry.extend(chunk),
                    Err(RecvTimeoutError::Timeout) => continue,
                    Err(RecvTimeoutError::Disconnected) => break,
                }
                // Pre-roll BEFORE Start, and only claim the stream is running
                // if the pre-roll actually happened: a Start over an unprimed
                // exclusive buffer plays a period of silence and then starves.
                let primed = client
                    .get_available_space_in_frames()
                    .map_err(|e| format!("get_available_space_in_frames: {e}"))
                    .and_then(|n| {
                        let need = n as usize * block_align;
                        let take = need.min(carry.len());
                        let mut buf: Vec<u8> = carry.drain(..take).collect();
                        buf.resize(need, 0);
                        render
                            .write_to_device(n as usize, &buf, None)
                            .map_err(|e| format!("write_to_device: {e}"))
                    });
                if let Err(e) = primed {
                    log::error!("[WASAPI Direct] Re-arm prefill failed ({e}); stream is dead");
                    dead.store(true, Ordering::SeqCst);
                    break;
                }
                if let Err(e) = client.start_stream() {
                    log::error!("[WASAPI Direct] Restart after stop failed: {e}");
                    dead.store(true, Ordering::SeqCst);
                    break;
                }
                stopped = false;
            }

            if paused {
                std::thread::sleep(Duration::from_millis(period_ms as u64));
                continue;
            }

            // Wait for the device to want more.
            match timing {
                WasapiTiming::Events => {
                    if let Some(h) = handle.as_ref() {
                        // GENEROUS on purpose. This deadline exists to stop the
                        // thread hanging on a device that died, not to detect
                        // glitches - the device's own buffer is what runs dry,
                        // and that is not observable from here.
                        //
                        // Measured: at a 3 ms period, a 2x deadline expired on
                        // 10% of wakeups (337 in 10 s) on a stream that was
                        // otherwise clean, because Windows scheduling simply is
                        // not that punctual. The spike waited 1000 ms and saw
                        // none in ten minutes. Treating those expiries as
                        // glitches invented a fault that was not there.
                        if h.wait_for_event(event_timeout_ms).is_err() {
                            timeouts.fetch_add(1, Ordering::Relaxed);
                            continue;
                        }
                    }
                }
                WasapiTiming::Polling => {
                    std::thread::sleep(Duration::from_millis((period_ms / 2).max(1) as u64));
                }
            }

            let n = match client.get_available_space_in_frames() {
                Ok(n) => n as usize,
                Err(e) => {
                    log::warn!("[WASAPI Direct] device gone ({e}); stream is dead");
                    dead.store(true, Ordering::SeqCst);
                    break;
                }
            };
            if n == 0 {
                continue;
            }
            let need = n * block_align;

            // Pull what has arrived, and be willing to WAIT a little for the
            // rest. The device still holds a full period of audio at this
            // point, so a short wait costs nothing; declaring an underrun the
            // moment the feeder happens to be mid-pack costs a false glitch
            // count on every jittery period. Measured: try_recv alone reported
            // 339 "underruns" in 10 s on a stream that never actually starved,
            // while the same hardware ran 10 minutes clean in the spike.
            let pull_deadline = Instant::now() + Duration::from_micros(slack_us);
            while carry.len() < need {
                match pcm.try_recv() {
                    Ok(chunk) => {
                        carry.extend(chunk);
                        continue;
                    }
                    Err(TryRecvError::Disconnected) => break,
                    Err(TryRecvError::Empty) => {}
                }
                let now = Instant::now();
                if now >= pull_deadline {
                    break;
                }
                match pcm.recv_timeout(pull_deadline - now) {
                    Ok(chunk) => carry.extend(chunk),
                    Err(_) => break,
                }
            }

            if carry.is_empty() {
                // Nothing at all: the documented cheap path is a silent buffer.
                let mut flags = BufferFlags::none();
                flags.silent = true;
                let filler = vec![0u8; need];
                if render.write_to_device(n, &filler, Some(flags)).is_err() {
                    dead.store(true, Ordering::SeqCst);
                    break;
                }
                // Trailing silence during a DRAIN is the drain working, not a
                // fault. Counting it reported 2 underruns on every clean run.
                if draining.is_none() && !starved {
                    underruns.fetch_add(1, Ordering::Relaxed);
                    starved = true;
                }
                // The queue is empty, so a drain in flight is COMPLETE. Answer
                // the caller and carry on: the stream stays open for the next
                // source, which is what makes gapless work. Ending the thread
                // here - as this used to - left a later append writing into a
                // dead render thread.
                if draining
                    .as_ref()
                    .map(|(d, _)| Instant::now() >= *d)
                    .unwrap_or(false)
                    || draining.is_some()
                {
                    if let Some((_, done)) = draining.take() {
                        let _ = done.send(true);
                    }
                }
                continue;
            }

            let mut buf = Vec::with_capacity(need);
            let take = need.min(carry.len());
            buf.extend(carry.drain(..take));
            if buf.len() < need {
                // Partial period. Zero-padding keeps the samples we DO have
                // instead of dropping them, which a silent buffer would.
                buf.resize(need, 0);
                // Counted EVERY time, unlike the fully-empty case: a partial
                // period is a discrete glitch that just happened, whereas an
                // empty queue is one starvation that lasts. Same reasoning as
                // above for why a drain's last partial period is not a fault.
                if draining.is_none() {
                    underruns.fetch_add(1, Ordering::Relaxed);
                }
            }
            if render.write_to_device(n, &buf, None).is_err() {
                dead.store(true, Ordering::SeqCst);
                break;
            }
            starved = false;
        }

        let _ = client.stop_stream();
        let _ = client.reset_stream();
        let n = underruns.load(Ordering::Relaxed);
        let t = timeouts.load(Ordering::Relaxed);
        if n > 0 || t > 0 {
            log::warn!("[WASAPI Direct] stream closed with {n} underrun(s), {t} event timeout(s)");
        } else {
            log::info!("[WASAPI Direct] stream closed clean");
        }
    }

    impl Inner {
        pub(super) fn new(
            endpoint_id: &str,
            rate: u32,
            channels: u16,
            timing: WasapiTiming,
        ) -> Result<Self, String> {
            let (reply_tx, reply_rx) = channel::<OpenReply>();
            let (pcm_tx, pcm_rx) = sync_channel::<Vec<u8>>(PCM_QUEUE_PERIODS);
            let (ctl_tx, ctl_rx) = channel::<Ctl>();
            let dead = Arc::new(AtomicBool::new(false));
            let underruns = Arc::new(AtomicU64::new(0));

            let id = endpoint_id.to_string();
            let dead_t = Arc::clone(&dead);
            let under_t = Arc::clone(&underruns);
            let timeouts = Arc::new(AtomicU64::new(0));
            let time_t = Arc::clone(&timeouts);
            let join = std::thread::Builder::new()
                .name("qbz-wasapi-render".to_string())
                .spawn(move || {
                    render_thread(
                        id, rate, channels, timing, reply_tx, pcm_rx, ctl_rx, dead_t, under_t,
                        time_t,
                    )
                })
                .map_err(|e| format!("spawn render thread: {e}"))?;

            // The thread answers once it has opened or given up. No timeout:
            // every call it makes before replying is a bounded COM call, and a
            // deadline here would race a slow-but-fine driver.
            let info = match reply_rx.recv() {
                Ok(Ok(info)) => info,
                Ok(Err(e)) => {
                    let _ = join.join();
                    return Err(e);
                }
                Err(_) => {
                    let _ = join.join();
                    return Err("render thread died before reporting".to_string());
                }
            };

            Ok(Inner {
                pcm: pcm_tx,
                ctl: ctl_tx,
                dead,
                underruns,
                timeouts,
                info,
                join: Some(join),
            })
        }

        fn check_alive(&self) -> Result<(), String> {
            if self.dead.load(Ordering::SeqCst) {
                // The player's reaction is to reinitialise, which is why this
                // has to be an error and not a silent no-op.
                return Err("device invalidated".to_string());
            }
            Ok(())
        }

        pub(super) fn write_bytes(&self, bytes: Vec<u8>) -> Result<(), String> {
            self.check_alive()?;
            // BLOCKS when the queue is full. That is the point: it is the
            // back-pressure that snd_pcm_writei gives the ALSA path for free.
            self.pcm
                .send(bytes)
                .map_err(|_| "render thread is gone".to_string())
        }

        pub(super) fn info(&self) -> &WasapiOpenInfo {
            &self.info
        }

        pub(super) fn underruns(&self) -> u64 {
            self.underruns.load(Ordering::Relaxed)
        }

        pub(super) fn timeouts(&self) -> u64 {
            self.timeouts.load(Ordering::Relaxed)
        }

        pub(super) fn drain_blocking(&self) -> Result<(), String> {
            self.check_alive()?;
            let (tx, rx) = channel::<bool>();
            self.send_ctl(Ctl::Drain(tx))?;
            match rx.recv_timeout(DRAIN_DEADLINE + Duration::from_secs(2)) {
                Ok(true) => Ok(()),
                Ok(false) => {
                    // A Stop threw the queue away while this drain waited. The
                    // audio did NOT play out, and the caller's writer thread
                    // treats a successful drain as "the source finished", so
                    // saying Ok here would report a track as played that was
                    // discarded mid-flight.
                    Err("drain cancelled: the stream was stopped".to_string())
                }
                Err(_) => {
                    // Not an error to the caller: a stuck drain must not wedge
                    // playback, and the stream is still usable.
                    log::warn!("[WASAPI Direct] drain did not complete within its deadline");
                    Ok(())
                }
            }
        }

        /// Halt, discard, and leave the stream ready for the next track --
        /// BLOCKING until the render thread confirms it has done so.
        ///
        /// The blocking is the contract, not politeness. The player sends this
        /// on a track change and then immediately queues the next track's PCM;
        /// if this returned early, the render thread's purge would run after
        /// that audio had been handed over and would eat it.
        pub(super) fn stop_blocking(&self) -> Result<(), String> {
            self.check_alive()?;
            let (tx, rx) = channel::<()>();
            self.send_ctl(Ctl::Stop(tx))?;
            match rx.recv_timeout(STOP_DEADLINE) {
                Ok(()) => Ok(()),
                Err(_) => Err("stop was not acknowledged by the render thread".to_string()),
            }
        }

        pub(super) fn send_ctl(&self, c: Ctl) -> Result<(), String> {
            self.ctl
                .send(c)
                .map_err(|_| "render thread is gone".to_string())
        }
    }

    impl Drop for Inner {
        fn drop(&mut self) {
            // MUST be Shutdown, not Stop. `Stop` means "prepare for the next
            // track" and the thread deliberately survives it -- sending it here
            // parked the thread waiting for audio that would never come and
            // hung `join()` forever, at process exit, every time.
            let _ = self.ctl.send(Ctl::Shutdown);
            if let Some(join) = self.join.take() {
                // Joining without a deadline is safe because every wait inside
                // the loop is bounded: the event wait by `event_timeout_ms`,
                // the re-arm wait by 20 ms, both looping back to this check.
                let _ = join.join();
            }
        }
    }
}

// ===========================================================================
// The public API, one body per platform
// ===========================================================================

#[cfg(windows)]
impl WasapiDirectStream {
    /// Open `endpoint_id` in EXCLUSIVE mode at `sample_rate`.
    ///
    /// `endpoint_id` is the WASAPI endpoint id (`{0.0.0...}.{guid}`), never a
    /// friendly name: Windows gives several endpoints the same name.
    pub fn new(
        endpoint_id: &str,
        sample_rate: u32,
        channels: u16,
        timing: WasapiTiming,
    ) -> Result<Self, String> {
        Ok(Self {
            inner: imp::Inner::new(endpoint_id, sample_rate, channels, timing)?,
        })
    }

    pub fn open_info(&self) -> &WasapiOpenInfo {
        self.inner.info()
    }

    /// Periods the feeder could not fill in time. Zero is the pass condition.
    pub fn underruns(&self) -> u64 {
        self.inner.underruns()
    }

    /// Periods where the device's own event never arrived. A different fault
    /// from an underrun, and counted separately so they stop hiding each other.
    pub fn event_timeouts(&self) -> u64 {
        self.inner.timeouts()
    }

    pub fn write_f32(&self, samples: &[f32]) -> Result<(), String> {
        let rung = self.inner.info().rung;
        let mut out = Vec::with_capacity(samples.len() * rung.container_bits() as usize / 8);
        pack_f32(samples, &rung, &mut out);
        self.inner.write_bytes(out)
    }

    pub fn write(&self, samples: &[i16]) -> Result<(), String> {
        let f: Vec<f32> = samples.iter().map(|&s| s as f32 / 32768.0).collect();
        self.write_f32(&f)
    }

    pub fn pause(&self) -> Result<(), String> {
        self.inner.send_ctl(imp::Ctl::Pause)
    }

    pub fn resume(&self) -> Result<(), String> {
        self.inner.send_ctl(imp::Ctl::Resume)
    }

    /// Play out what is queued, then return - BLOCKING, like the ALSA drain
    /// this shares a writer thread with. Returning early would tell the
    /// player's writer thread the source had finished while it was still
    /// playing.
    ///
    /// Bounded: a device that stopped consuming must not hang the writer, so a
    /// drain that overruns its deadline returns anyway and says so.
    pub fn drain(&self) -> Result<(), String> {
        self.inner.drain_blocking()
    }

    pub fn stop(&self) -> Result<(), String> {
        self.inner.stop_blocking()
    }

    pub fn sample_rate(&self) -> u32 {
        self.inner.info().rate
    }

    pub fn channels(&self) -> u16 {
        self.inner.info().channels
    }

    pub fn bit_perfect_mode(&self) -> BitPerfectMode {
        self.inner.info().rung.bit_perfect_mode()
    }
}

/// Off Windows every constructor refuses, and the teardown calls succeed so a
/// caller written against the Windows arm still compiles and unwinds cleanly.
/// Mirrors `alsa_direct.rs`'s non-Linux stub.
#[cfg(not(windows))]
impl WasapiDirectStream {
    const ONLY: &'static str = "WASAPI is only available on Windows";

    pub fn new(
        _endpoint_id: &str,
        _sample_rate: u32,
        _channels: u16,
        _timing: WasapiTiming,
    ) -> Result<Self, String> {
        Err(Self::ONLY.to_string())
    }

    pub fn underruns(&self) -> u64 {
        0
    }

    pub fn event_timeouts(&self) -> u64 {
        0
    }

    pub fn write_f32(&self, _samples: &[f32]) -> Result<(), String> {
        Err(Self::ONLY.to_string())
    }

    pub fn write(&self, _samples: &[i16]) -> Result<(), String> {
        Err(Self::ONLY.to_string())
    }

    pub fn pause(&self) -> Result<(), String> {
        Ok(())
    }

    pub fn resume(&self) -> Result<(), String> {
        Ok(())
    }

    pub fn drain(&self) -> Result<(), String> {
        Ok(())
    }

    pub fn stop(&self) -> Result<(), String> {
        Ok(())
    }

    pub fn sample_rate(&self) -> u32 {
        44100
    }

    pub fn channels(&self) -> u16 {
        2
    }

    pub fn bit_perfect_mode(&self) -> BitPerfectMode {
        BitPerfectMode::Disabled
    }
}

/// Same forwarding as the ALSA side. Both platforms' constructors exist on
/// every host (one of them refusing), so this impl is unconditional too.
impl crate::backend::DirectSink for WasapiDirectStream {
    fn write_f32(&self, samples: &[f32]) -> Result<(), String> {
        WasapiDirectStream::write_f32(self, samples)
    }
    fn drain(&self) -> Result<(), String> {
        WasapiDirectStream::drain(self)
    }
    fn stop(&self) -> Result<(), String> {
        WasapiDirectStream::stop(self)
    }
    fn sample_rate(&self) -> u32 {
        WasapiDirectStream::sample_rate(self)
    }
    fn channels(&self) -> u16 {
        WasapiDirectStream::channels(self)
    }
    fn log_label(&self) -> &'static str {
        "WASAPI Direct Engine"
    }
}
