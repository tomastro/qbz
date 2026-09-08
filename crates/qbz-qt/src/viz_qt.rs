//! Audio-visualizer glue for the Qt shell — the QML counterpart of
//! `qbz/src/visualizer.rs` (Slint).
//!
//! Spawns the frontend-agnostic FFT producer (`qbz_audio::visualizer`) against
//! the runtime's [`VisualizerTap`], latches each [`VizFrame`] into a
//! single-slot cell, and hands the latest frame to the `QbzViz` bridge
//! properties from a drain thread that hops to the Qt event loop.
//!
//! The drain is SIGNAL-DRIVEN, not polled. The producer runs its own clock
//! (`TARGET_FPS = 30`); a drain on a second, independent 33 ms clock beats
//! against it and intermittently misses a frame, which reads on screen as
//! "slow motion". Instead the sink unparks the drain right after it latches a
//! frame of the stream the visible mode consumes, and the drain parks again as
//! soon as it has published it: exactly one publish per produced frame, no
//! aliasing, and zero idle wakeups (cheaper than the old poll, not just
//! smoother).
//!
//! Cost controls (the frozen reference install() has the same three, and they are why the
//! ambient field sits under 3% CPU during playback):
//!   1. The tap starts DISABLED — nothing is captured and the producer idles
//!      until the dock's eye toggle calls `set_enabled(true)`.
//!   2. While disabled the drain thread PARKS (no timer, no locks); enabling
//!      unparks both it and the producer for an instant wake.
//!   3. `set_paused` mirrors the transport, so a paused player parks the
//!      producer instead of re-running the FFT over a stale ring buffer.
//! Plus the mode gate: only the ONE stream the visible mode renders is
//! marshalled into a `QList` (in Bars mode the 512-float waveform is latched
//! for an instant mode switch but never published) — UNLESS the immersive
//! overlay is open, which owns the legacy marshalling set and publishes bars,
//! energy and waveform every frame (§4.2 of the immersive-port contract).
//! The two scope streams are requested independently and only while their
//! panel is visible. Block A1
//! (2026-08-15 immersive-completion) extends the immersive set: `Spectral512`
//! and `Transient1` are LATCHED too (still never dock-marshalled), and the
//! drain derives the shader-scenes uniform pack from them HOST-SIDE
//! (visualizer.rs:172-269 parity — bands8 pairing, level, the level_smooth
//! EMA, the beat/transient envelopes, the phase accumulator with the 4096
//! wrap, the spectral_peak EMA) and publishes it as ONE batched JSON property
//! on `QbzShaderScene` per fresh FFT frame (spec 01 §3: a single property
//! bag, not 20 individual notifies — each notify dirties). Block A4 consumes
//! the SAME latched `Spectral512` Rust-side for the Line Bed depth ring
//! (`linebed_qt.rs` — the 512 floats never reach QML; the ready 256x200
//! ring publishes once per tick, gated on the scene being on screen).
//!
//! Protected-audio note: this lives entirely downstream of the read-only ring
//! buffer. It touches none of the device/stream init (CLAUDE.md "Audio
//! backend — PROTECTED"); the tap is a passive sample copy.

use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::Thread;

use cxx_qt_lib::QList;
use qbz_audio::visualizer::{
    spawn_visualizer_thread, VizFrame, VizSink, GONIOMETER_BIT, OSCILLOSCOPE_BIT,
};
use qbz_audio::VisualizerTap;

use crate::viz_bridge;

// A4 (Line Bed, mode 5): the scene's CPU half (LineBedState + the
// 512→256 reshape, fed from the latched Spectral512 cell). Declared HERE
// with #[path] rather than in main.rs because main.rs sits outside this
// block's file ownership (a second agent is working the tree) — the drain
// is its only consumer anyway.
#[path = "linebed_qt.rs"]
pub(crate) mod linebed_qt;

// A3 (Spectral Ribbon, mode 4): the scene's CPU half (RibbonState — the
// 512-byte row + the playback column/reset header, fed from the SAME
// latched Spectral512 cell). Same #[path] rationale as linebed_qt.
#[path = "ribbon_qt.rs"]
pub(crate) mod ribbon_qt;

/// Single-slot, latest-wins frame store shared with the FFT producer thread.
/// A stalled UI drops intermediate frames instead of growing a queue.
#[derive(Default)]
struct VizCells {
    bars: Mutex<Option<[f32; 16]>>,
    energy: Mutex<Option<[f32; 5]>>,
    waveform: Mutex<Option<Box<[f32; 512]>>>,
    // A1 (shader scenes): latched ONLY while the immersive overlay is open
    // (MARSHAL_ALL) — the dock renders neither, so the dock path keeps
    // dropping them exactly like before. Neither stream WAKES the drain:
    // they hitchhike on the bars/energy/waveform wakes, the way the Slint
    // 33 ms timer drain takes whatever the cells happen to hold
    // (visualizer.rs:146-178).
    spectral: Mutex<Option<Vec<f32>>>,
    transient: Mutex<Option<f32>>,
    goniometer: Mutex<Option<(Box<[f32; 512]>, f32)>>,
    oscilloscope: Mutex<Option<Box<[f32; 512]>>>,
}

/// Producer-side sink: latches frames into the shared cells and signals the
/// drain. Never touches Qt (this runs on the FFT thread).
struct QtVizSink {
    cells: Arc<VizCells>,
}

impl VizSink for QtVizSink {
    fn submit(&self, frame: VizFrame) {
        // The producer emits several variants per FFT frame (spectral, bars,
        // energy, transient, waveform). Waking the drain on each of them would
        // cost 3-4 wakeups per frame for one useful publish, so only the
        // stream the visible mode renders signals — UNLESS the immersive
        // overlay is open (MARSHAL_ALL, §4.2b), whose wake table covers every
        // signalled stream; A1's two scene-only streams latch WITHOUT
        // signalling (see their match arms). The other cells are still
        // latched so a mode switch has a frame ready immediately.
        let mode = ACTIVE_MODE.load(Ordering::Relaxed);
        let all = MARSHAL_ALL.load(Ordering::Relaxed);
        let wake = match frame {
            VizFrame::Viz16(b) => {
                *self.cells.bars.lock().unwrap() = Some(b);
                stream_wakes(mode, all, Stream::Bars)
            }
            VizFrame::Wave256x2(b) => {
                *self.cells.waveform.lock().unwrap() = Some(b);
                stream_wakes(mode, all, Stream::Waveform)
            }
            VizFrame::Energy5(b) => {
                *self.cells.energy.lock().unwrap() = Some(b);
                stream_wakes(mode, all, Stream::Energy)
            }
            // The dock renders neither stream, so off-immersive they are
            // still dropped for free. While the immersive overlay is open
            // (MARSHAL_ALL, A1) they are LATCHED — Transient1 feeds every
            // scene's beat punch and Spectral512 feeds the spectral-peak
            // EMA (the future mode 4) — but neither WAKES the drain: they
            // ride the next bars/energy/waveform-signalled pass, so the
            // wake/publish cadence is unchanged.
            VizFrame::Spectral512(b) => {
                if all {
                    *self.cells.spectral.lock().unwrap() = Some(b);
                }
                false
            }
            VizFrame::Transient1(x) => {
                if all {
                    *self.cells.transient.lock().unwrap() = Some(x);
                }
                false
            }
            VizFrame::Goniometer {
                points,
                correlation,
            } => {
                *self.cells.goniometer.lock().unwrap() = Some((points, correlation));
                stream_wakes(mode, all, Stream::Goniometer)
            }
            VizFrame::Oscilloscope(points) => {
                *self.cells.oscilloscope.lock().unwrap() = Some(points);
                stream_wakes(mode, all, Stream::Oscilloscope)
            }
        };
        if wake {
            wake_drain();
        }
    }
}

struct VizHandles {
    tap: VisualizerTap,
    fft_thread: Thread,
}

static HANDLES: OnceLock<VizHandles> = OnceLock::new();
/// Drain thread handle. Published BEFORE the producer is spawned so a latched
/// frame can never find itself with nowhere to signal.
static DRAIN_THREAD: OnceLock<Thread> = OnceLock::new();
/// Drives the drain loop: false parks it indefinitely, true lets it publish.
/// This is the EFFECTIVE enable — the OR of the per-source bits in
/// `ENABLED_MASK` (§4.2a); nothing writes it except `apply_effective_enabled`.
static ENABLED: AtomicBool = AtomicBool::new(false);
/// The band's render mode (0 Bars / 1 Waveform / 2 Energy / 3 Goniometer /
/// 4 Oscilloscope). The drain only
/// publishes the ONE stream that mode consumes: in Bars mode, marshalling the
/// 512-float waveform into a QList 30 times a second would be pure waste.
static ACTIVE_MODE: AtomicI32 = AtomicI32::new(0);

// ---------------------------------------------------------------------------
// §4.2 (immersive-port contract): two-source enable + immersive marshal-all
// ---------------------------------------------------------------------------

/// Enable-source bits (§4.2a): the tap runs while EITHER consumer wants it.
/// The dock's bit is driven by the existing `set_enabled` call sites
/// (`viz_bridge.rs:137`, `main.rs:1518`); the immersive bit by
/// `QbzImmersive`'s open funnel.
pub(crate) const DOCK_BIT: u32 = 0b01;
pub(crate) const IMMERSIVE_BIT: u32 = 0b10;
static ENABLED_MASK: AtomicU32 = AtomicU32::new(0);

/// §4.2b: while the immersive overlay is open it owns the marshalling set —
/// bars AND energy AND waveform publish every frame, regardless of
/// `ACTIVE_MODE`. Dock mode-cycle writes while it is set still record
/// `ACTIVE_MODE` (and the dock's pref) but have no effect on the marshalled
/// legacy set; the close edge restores `ACTIVE_MODE` from the pref anyway.
/// Scope DSP remains visibility-gated by the two scope masks below.
static MARSHAL_ALL: AtomicBool = AtomicBool::new(false);
static DOCK_SCOPE_MASK: AtomicU32 = AtomicU32::new(0);
static IMMERSIVE_PANEL_SCOPE_MASK: AtomicU32 = AtomicU32::new(0);
static IMMERSIVE_SCOPE_MASK: AtomicU32 = AtomicU32::new(0);
static IMMERSIVE_SCENE_ACTIVE: AtomicBool = AtomicBool::new(false);

/// A consumer of the FFT tap (§4.2a).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum VizSource {
    Dock,
    Immersive,
}

impl VizSource {
    fn bit(self) -> u32 {
        match self {
            VizSource::Dock => DOCK_BIT,
            VizSource::Immersive => IMMERSIVE_BIT,
        }
    }
}

fn mask_with(mask: u32, bit: u32, on: bool) -> u32 {
    if on {
        mask | bit
    } else {
        mask & !bit
    }
}

fn mask_enabled(mask: u32) -> bool {
    mask != 0
}

/// The five dock render streams (the wake table's rows).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Stream {
    Bars,
    Waveform,
    Energy,
    Goniometer,
    Oscilloscope,
}

/// Wake table for the drain signal: immersive marshal-all wakes on every
/// legacy stream; scope frames hitchhike on that cadence. Otherwise exactly
/// the ONE stream the dock's active mode renders wakes the drain.
fn stream_wakes(mode: i32, marshal_all: bool, stream: Stream) -> bool {
    if marshal_all && matches!(stream, Stream::Bars | Stream::Waveform | Stream::Energy) {
        return true;
    }
    match stream {
        Stream::Bars => mode == 0,
        Stream::Waveform => mode == 1,
        Stream::Energy => mode == 2,
        Stream::Goniometer => !marshal_all && mode == 3,
        Stream::Oscilloscope => !marshal_all && mode == 4,
    }
}

fn scope_mask_for_mode(mode: i32) -> u32 {
    match mode {
        3 | 7 => GONIOMETER_BIT,
        4 | 8 => OSCILLOSCOPE_BIT,
        _ => 0,
    }
}

fn visible_scope_mask(requested: u32, scene_active: bool) -> u32 {
    if scene_active {
        0
    } else {
        requested
    }
}

fn apply_immersive_scope_mask() {
    let mask = visible_scope_mask(
        IMMERSIVE_PANEL_SCOPE_MASK.load(Ordering::Relaxed),
        IMMERSIVE_SCENE_ACTIVE.load(Ordering::Relaxed),
    );
    IMMERSIVE_SCOPE_MASK.store(mask, Ordering::Relaxed);
    apply_scope_mask();
}

fn apply_scope_mask() {
    if let Some(handles) = HANDLES.get() {
        handles.tap.set_scope_mask(
            DOCK_SCOPE_MASK.load(Ordering::Relaxed) | IMMERSIVE_SCOPE_MASK.load(Ordering::Relaxed),
        );
    }
}

/// Signal the drain that a frame it cares about is waiting. `unpark` is a
/// single atomic swap when the drain is already awake, and the permit it
/// leaves behind is remembered by a later `park()`, so a wake can never be
/// lost to the latch/park race.
#[inline]
fn wake_drain() {
    if let Some(t) = DRAIN_THREAD.get() {
        t.unpark();
    }
}

fn to_qlist(values: &[f32]) -> QList<f32> {
    let mut list = QList::<f32>::default();
    for v in values {
        list.append(*v);
    }
    list
}

// ---------------------------------------------------------------------------
// A1: the host-side shader pack (visualizer.rs:172-269 parity)
// ---------------------------------------------------------------------------

/// Derivation state for the shader-scenes uniform pack — the Qt twin of the
/// Slint drain's `last_*` locals (qbz/src/visualizer.rs). Lives on the DRAIN
/// thread (a plain local, no locking): every field is per-tick state derived
/// from the latched cells. Energy/bars arrive ALREADY smoothed upstream
/// (qbz-audio) and are passed through raw — no double EMA
/// (shader_underlay.rs FrameAudio doc).
#[derive(Default)]
struct ShaderPackState {
    last_bars16: [f32; 16],
    last_energy: [f32; 5],
    level_smooth: f32,
    beat: f32,
    /// Slow floor the AC-coupled beat is measured against — see `beat_ac`.
    beat_floor: f32,
    transient: f32,
    phase: f32,
    spectral_peak: f32,
}

/// The 4096 phase wrap: an integer multiple of 8 so the Tunnel's `& 7` ring
/// math stays seamless across the wrap (tunnel.wgsl:66-69). The Qt port MUST
/// keep it (spec 01 §1).
const PHASE_WRAP: f32 = 4096.0;

impl ShaderPackState {
    /// Advance ONE drain tick (~30 Hz, gated on a fresh Viz16 — the cadence
    /// carrier) and return the batched JSON pack. Mirrors
    /// visualizer.rs:203-269 EXACTLY, including the ORDER: the fresh
    /// transient maxes into both envelopes first, then the envelopes decay
    /// (beat ×0.88, transient ×0.85), then level/bands/phase derive from the
    /// DECAYED values.
    fn tick(&mut self, new_transient: Option<f32>, new_spectral: Option<&[f32]>) -> String {
        if let Some(x) = new_transient {
            self.transient = x.max(self.transient);
            self.beat = x.max(self.beat);
        }
        self.transient *= 0.85;
        self.beat *= 0.88;
        // AC-COUPLED BEAT — the onset measured against its OWN density.
        //
        // `beat` alone stops being a beat on busy material, and the arithmetic
        // is why: it maxes in on a transient and decays x0.88 per 33 ms tick,
        // so it needs ~600 ms to fall back to a tenth. Portnoy's double kick
        // runs past 10 hits/s; between two of them the envelope only falls
        // 0.88^3 ~= 0.68 before the next one tops it back up, so it does not
        // saturate at 1 — it RIPPLES, shallow, around 0.7. Everything a scene
        // multiplies by it (a splat that should detonate, a rotation that
        // should jolt) turns into a constant, and the busier the music the
        // flatter the picture. That is the inversion: the most frenetic input
        // produces the least motion.
        //
        // Subtracting a slow floor restores the CONTRAST that the envelope
        // loses. On sparse material the floor sits near zero and this is the
        // plain envelope; on dense material the floor rises with it and only
        // the peaks ABOVE the local density come through. The floor tracks
        // asymmetrically — it follows the beat down quickly (x0.90) so a
        // breakdown re-arms the punch within ~a third of a second, and creeps
        // up slowly (0.02) so one loud hit does not deafen the next.
        self.beat_floor = if self.beat < self.beat_floor {
            self.beat_floor * 0.90
        } else {
            self.beat_floor * 0.98 + self.beat * 0.02
        };

        // 8 log bands paired from the 16 bars (visualizer.rs:205-208).
        let mut bands8 = [0.0f32; 8];
        for i in 0..8 {
            bands8[i] = (self.last_bars16[2 * i] + self.last_bars16[2 * i + 1]) * 0.5;
        }
        // level = mean(energy5); level_smooth = slow EMA (×0.96 + 0.04·level).
        let level = (self.last_energy[0]
            + self.last_energy[1]
            + self.last_energy[2]
            + self.last_energy[3]
            + self.last_energy[4])
            * 0.2;
        self.level_smooth = self.level_smooth * 0.96 + level * 0.04;
        // Forward-motion clock: host-side (rate is audio-dependent), wrapped
        // at an integer so fract()-based ring patterns stay continuous across
        // the wrap (visualizer.rs:219-222).
        self.phase += 0.012 + level * 0.02 + self.beat * 0.02;
        if self.phase >= PHASE_WRAP {
            self.phase -= PHASE_WRAP;
        }
        // Real-time ceiling (the future mode 4): highest band with signal,
        // EMA-smoothed so the line tracks without jitter (visualizer.rs:
        // 223-239). Derived whenever a fresh spectral frame is latched,
        // mode-agnostic — the shipped scenes ignore it.
        if let Some(bins) = new_spectral {
            let n = bins.len();
            if n > 1 {
                let mut hi = 0usize;
                for (i, &v) in bins.iter().enumerate() {
                    if v > 0.05 {
                        hi = i;
                    }
                }
                let target = hi as f32 / (n - 1) as f32;
                self.spectral_peak = self.spectral_peak * 0.85 + target * 0.15;
            }
        }

        // ONE batched publish per tick (spec 01 §3: a single property bag,
        // not 20 individual notifies — each notify dirties). The vec4 fields
        // of the uniform contract map as: energy_lo = sub/bass/mid/presence,
        // energy_hi = air/spectral_peak/0/0 (shader_underlay.rs:844-845),
        // bands_lo/hi = bands8 0..3 / 4..7. time/resolution/palette are NOT
        // in the pack: time is the QML layer's local pulse clock, resolution
        // is the item size, palette is QbzShell.ambient*.
        format!(
            "{{\"phase\":{},\"beat\":{},\"beatAc\":{},\"level\":{},\"levelSmooth\":{},\
             \"transient\":{},\
             \"energyLo\":[{},{},{},{}],\"energyHi\":[{},{},0,0],\
             \"bandsLo\":[{},{},{},{}],\"bandsHi\":[{},{},{},{}]}}",
            self.phase,
            self.beat,
            // Normalised so a scene can use it exactly where it used `beat`:
            // 0 at the local floor, 1 at a full-scale onset above it.
            ((self.beat - self.beat_floor).max(0.0) / (1.0 - self.beat_floor).max(0.15))
                .clamp(0.0, 1.0),
            level,
            self.level_smooth,
            self.transient,
            self.last_energy[0],
            self.last_energy[1],
            self.last_energy[2],
            self.last_energy[3],
            self.last_energy[4],
            self.spectral_peak,
            bands8[0],
            bands8[1],
            bands8[2],
            bands8[3],
            bands8[4],
            bands8[5],
            bands8[6],
            bands8[7],
        )
    }
}

/// Take the pending frame(s) of the stream(s) the current marshalling set
/// renders and hop them onto the Qt thread. Each cell's guard is released by
/// the end of its `let` statement — the mutex is NEVER held across the
/// `viz_bridge::ui` hop, so the producer never blocks on the drain.
fn publish_active(
    cells: &VizCells,
    pack: &mut ShaderPackState,
    linebed: &mut linebed_qt::LineBedState,
    ribbon: &mut ribbon_qt::RibbonState,
) {
    // §4.2b: immersive open -> publish ALL THREE streams each frame (the
    // immersive panels consume bars + energy + waveform simultaneously, like
    // the Slint immersive does). ACTIVE_MODE is irrelevant here.
    if MARSHAL_ALL.load(Ordering::Relaxed) {
        let bars = cells.bars.lock().unwrap().take();
        // Viz16 is emitted on EVERY producer frame, so a fresh bars cell is
        // the cadence carrier for the pack tick below: a wake that found the
        // cells already drained (the sink unparks once per stream variant,
        // the park permit coalesces) must NOT tick — the envelopes and the
        // phase accumulator are per-tick, and ticking on empty passes would
        // run the decay faster than the reference's 30 Hz drain.
        let fresh_frame = bars.is_some();
        if let Some(b) = bars {
            pack.last_bars16 = b;
            viz_bridge::ui(move |mut v| v.as_mut().set_bars(to_qlist(&b)));
        }
        let energy = cells.energy.lock().unwrap().take();
        if let Some(b) = energy {
            pack.last_energy = b;
            viz_bridge::ui(move |mut v| v.as_mut().set_energy(to_qlist(&b)));
        }
        let waveform = cells.waveform.lock().unwrap().take();
        if let Some(b) = waveform {
            viz_bridge::ui(move |mut v| v.as_mut().set_waveform(to_qlist(b.as_ref())));
        }
        let goniometer = cells.goniometer.lock().unwrap().take();
        if let Some((points, correlation)) = goniometer {
            viz_bridge::ui(move |mut v| {
                v.as_mut().set_goniometer(to_qlist(points.as_ref()));
                v.as_mut().set_stereo_correlation(correlation);
            });
        }
        let oscilloscope = cells.oscilloscope.lock().unwrap().take();
        if let Some(points) = oscilloscope {
            viz_bridge::ui(move |mut v| {
                v.as_mut().set_oscilloscope(to_qlist(points.as_ref()));
            });
        }
        // A1: the scene-only streams, latched by the sink without waking.
        let spectral = cells.spectral.lock().unwrap().take();
        let transient = cells.transient.lock().unwrap().take();
        if fresh_frame {
            crate::shader_scene_bridge::publish_pack(pack.tick(transient, spectral.as_deref()));
        }
        // A4 (Line Bed, mode 5): push the SAME latched Spectral512 frame
        // into the depth ring (fresh frames only, like the reference's
        // per-spectral-frame push — shader_underlay.rs:934-939) and publish
        // the ready 256x200 ring ONCE per tick, one QByteArray notify. Both
        // are gated on the scene being on screen (the bridge's atomic
        // mirror of scene == 5): 200 KB at 30 Hz for a scene nobody is
        // looking at is exactly what the pulse law forbids. The state
        // persists across scene switches, like Slint's thread-local.
        if crate::shader_scene_bridge::linebed_active() {
            if let Some(bins) = spectral.as_deref() {
                linebed.push(bins);
            }
            crate::shader_scene_bridge::publish_linebed(linebed.ring_bytes());
        }
        // A3 (Spectral Ribbon, mode 4): one self-describing frame per fresh
        // Spectral512 (the 512-byte row + playback column + reset header —
        // ribbon_qt.rs), gated on the scene exactly like the linebed ring
        // (517 B at 30 Hz for a scene nobody is looking at is still the
        // pulse law's "invisible writes nothing"). FRESH frames only: a
        // re-publish at the same progress would re-stamp the same columns
        // (the C++ gap-fill cursor never moves backward on its own).
        if crate::shader_scene_bridge::ribbon_active() {
            if let Some(bins) = spectral.as_deref() {
                if let Some(frame) = ribbon.frame(bins, crate::now_playing::ribbon_cursor()) {
                    crate::shader_scene_bridge::publish_ribbon(frame);
                }
            }
        }
        return;
    }
    match ACTIVE_MODE.load(Ordering::Relaxed) {
        1 => {
            let frame = cells.waveform.lock().unwrap().take();
            if let Some(b) = frame {
                viz_bridge::ui(move |mut v| v.as_mut().set_waveform(to_qlist(b.as_ref())));
            }
        }
        2 => {
            let frame = cells.energy.lock().unwrap().take();
            if let Some(b) = frame {
                viz_bridge::ui(move |mut v| v.as_mut().set_energy(to_qlist(&b)));
            }
        }
        3 => {
            let frame = cells.goniometer.lock().unwrap().take();
            if let Some((points, correlation)) = frame {
                viz_bridge::ui(move |mut v| {
                    v.as_mut().set_goniometer(to_qlist(points.as_ref()));
                    v.as_mut().set_stereo_correlation(correlation);
                });
            }
        }
        4 => {
            let frame = cells.oscilloscope.lock().unwrap().take();
            if let Some(points) = frame {
                viz_bridge::ui(move |mut v| {
                    v.as_mut().set_oscilloscope(to_qlist(points.as_ref()));
                });
            }
        }
        _ => {
            let frame = cells.bars.lock().unwrap().take();
            if let Some(b) = frame {
                viz_bridge::ui(move |mut v| v.as_mut().set_bars(to_qlist(&b)));
            }
        }
    }
}

/// Wire the visualizer. Call once at startup, after the runtime is built.
/// No-op when the runtime carries no tap (i.e. it was built with
/// [`qbz_app::shell::AppRuntime::new`] instead of `with_visualizer`).
pub fn install(tap: VisualizerTap) {
    if HANDLES.get().is_some() {
        log::warn!("[qbz-qt][viz] install called twice; ignoring");
        return;
    }

    // The startup macro is normally sufficient, but the explicit symbol
    // reference keeps the custom item linked from static archives on every
    // platform and guarantees registration before the QML module loads.
    register_scope_qml_item();

    let cells = Arc::new(VizCells::default());

    // The drain: park until the sink says a frame of the visible stream is
    // ready (or until `set_enabled` wakes it), publish exactly that one frame,
    // park again. No timer, no polling — while disabled or paused it blocks
    // outright instead of spinning.
    let drain_cells = cells.clone();
    let drain_thread = std::thread::Builder::new()
        .name("qbz-qt-viz-drain".to_string())
        .spawn(move || {
            // A1: the shader-pack derivation state lives and dies with the
            // drain thread — per-tick state, never shared, never locked.
            let mut pack = ShaderPackState::default();
            // A4: the Line Bed depth ring, same drain-local pattern.
            let mut linebed = linebed_qt::LineBedState::default();
            // A3: the Spectral Ribbon cursor/reset state, same pattern.
            let mut ribbon = ribbon_qt::RibbonState::default();
            loop {
                if !ENABLED.load(Ordering::Relaxed) {
                    // Disabled: block until `set_enabled(true)` unparks us. A
                    // leftover permit only costs one extra trip round this check.
                    std::thread::park();
                    continue;
                }
                publish_active(&drain_cells, &mut pack, &mut linebed, &mut ribbon);
                // Wait for the next produced frame. If the sink unparked us while
                // we were publishing, the permit is already set and this returns
                // immediately — no wakeup is lost, we just re-check the cell.
                std::thread::park();
            }
        })
        .expect("spawn viz drain thread")
        .thread()
        .clone();
    // Publish the handle BEFORE the producer exists: from here on, every
    // latched frame has a thread to signal.
    let _ = DRAIN_THREAD.set(drain_thread);

    let sink = Arc::new(QtVizSink {
        cells: cells.clone(),
    });
    let fft_thread = spawn_visualizer_thread(tap.clone(), sink).thread().clone();

    let _ = HANDLES.set(VizHandles { tap, fft_thread });
    log::info!("[qbz-qt][viz] producer + signal-driven drain installed (idle until enabled)");
}

/// Capture gate — the `QbzViz.setEnabled` invokable. Drives the DOCK source
/// bit (§4.2a): both existing call sites (`viz_bridge.rs:137`, `main.rs:1518`)
/// stay dock-sourced; the effective enable only flips when the OR of the bits
/// does, so a dock toggle while immersive is open (or vice versa) never parks
/// a tap the other consumer still needs.
pub fn set_enabled(on: bool) {
    set_enabled_source(VizSource::Dock, on);
    let mask = if on {
        scope_mask_for_mode(ACTIVE_MODE.load(Ordering::Relaxed))
    } else {
        0
    };
    DOCK_SCOPE_MASK.store(mask, Ordering::Relaxed);
    apply_scope_mask();
}

/// §4.2a two-source enable: set one consumer's bit; the EFFECTIVE enable is
/// the OR of all bits. Only the effective edges unpark/park the FFT thread —
/// flipping one bit while another stays on changes nothing (trap 4: the
/// immersive close edge NEVER clears the dock's bit).
pub(crate) fn set_enabled_source(source: VizSource, on: bool) {
    let bit = source.bit();
    let prev = ENABLED_MASK.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |m| {
        Some(mask_with(m, bit, on))
    });
    let Ok(prev) = prev else { return };
    let was_enabled = mask_enabled(prev);
    let now_enabled = mask_enabled(mask_with(prev, bit, on));
    if was_enabled == now_enabled {
        return;
    }
    apply_effective_enabled(now_enabled);
}

/// The effective enable flipped: toggle the tap, wake the parked
/// producer/drain, and (when turning off) leave the last frame on screen
/// frozen, exactly like the Slint handler.
fn apply_effective_enabled(on: bool) {
    let Some(h) = HANDLES.get() else {
        return;
    };
    ENABLED.store(on, Ordering::Relaxed);
    h.tap.set_enabled(on);
    if on {
        // Both park while idle; unpark for an instant wake instead of waiting
        // out the producer's 200ms IDLE_POLL. The drain re-reads ENABLED (now
        // true) and publishes whatever the last latched frame was, then parks
        // until the producer signals the next one.
        h.fft_thread.unpark();
        wake_drain();
    }
    // Turning OFF needs no unpark: the drain parks on its own at the top of
    // the loop and stays there, which is exactly the "costs nothing" state.
}

/// §4.2b immersive open hook (QbzImmersive's open funnel): the immersive bit
/// goes ON and the marshalling set switches to all-three. MARSHAL_ALL is set
/// BEFORE the enable so the first publish after the wake is already the full
/// set.
pub(crate) fn immersive_opened() {
    MARSHAL_ALL.store(true, Ordering::Relaxed);
    set_enabled_source(VizSource::Immersive, true);
    if ENABLED.load(Ordering::Relaxed) {
        // The panels should not wait a producer tick for the streams the dock
        // mode was not marshalling — same rationale as `set_mode`'s wake.
        wake_drain();
    }
}

/// §4.2b immersive close hook: the immersive bit goes OFF (the dock's bit is
/// untouched — trap 4/20), single-stream marshalling resumes, and
/// `ACTIVE_MODE` is restored from the dock's persisted pref (dock mode-cycles
/// while immersive was open already updated it via
/// `settings_qt::set_large_spectrum_mode`). The renderer-tier resolution is
/// repeated here so closing Immersive on Software cannot quietly re-enable a
/// hidden scope producer.
pub(crate) fn immersive_closed() {
    IMMERSIVE_PANEL_SCOPE_MASK.store(0, Ordering::Relaxed);
    IMMERSIVE_SCOPE_MASK.store(0, Ordering::Relaxed);
    apply_scope_mask();
    set_enabled_source(VizSource::Immersive, false);
    MARSHAL_ALL.store(false, Ordering::Relaxed);
    set_mode(crate::settings_qt::large_spectrum_mode_for_tier(
        crate::settings_qt::large_spectrum_mode(),
        crate::renderer_qt::gpu_tier(),
    ));
}

/// Point the drain at the stream the band is actually rendering. Called at
/// install time (from the persisted pref) and on every mode cycle.
pub fn set_mode(mode: i32) {
    let mode = mode.clamp(0, 4);
    ACTIVE_MODE.store(mode, Ordering::Relaxed);
    let dock_visible = ENABLED_MASK.load(Ordering::Relaxed) & DOCK_BIT != 0;
    DOCK_SCOPE_MASK.store(
        if dock_visible {
            scope_mask_for_mode(mode)
        } else {
            0
        },
        Ordering::Relaxed,
    );
    apply_scope_mask();
    // The new stream may already have a frame latched from before the switch;
    // publish it now instead of showing the old mode's last frame until the
    // producer's next tick (and it is what unblocks a switch made while the
    // player is paused).
    if ENABLED.load(Ordering::Relaxed) {
        wake_drain();
    }
}

/// Update the scope requested by the active immersive FOCUS panel. Split
/// layouts and every existing FOCUS mode leave both scope producers idle.
pub(crate) fn set_immersive_view(view_mode: i32, mode: i32) {
    let mask = if MARSHAL_ALL.load(Ordering::Relaxed) && view_mode == 0 {
        scope_mask_for_mode(mode)
    } else {
        0
    };
    IMMERSIVE_PANEL_SCOPE_MASK.store(mask, Ordering::Relaxed);
    apply_immersive_scope_mask();
}

/// A shader scene replaces every FOCUS panel. Keep the desired scope mode so
/// returning to the panel is instant, but disable its DSP and Qt publishes
/// while the scene is covering it.
pub(crate) fn set_immersive_scene_active(active: bool) {
    IMMERSIVE_SCENE_ACTIVE.store(active, Ordering::Relaxed);
    apply_immersive_scope_mask();
}

extern "C" {
    fn qbz_scope_trace_register_qml_type();
}

fn register_scope_qml_item() {
    // SAFETY: no arguments; registration is guarded and occurs before QML is
    // loaded. The explicit reference also keeps the static-library object.
    unsafe { qbz_scope_trace_register_qml_type() };
}

/// Mirror the transport onto the tap so a paused player parks the producer.
/// Called from the playback path on every playing-state flip (the Slint side
/// does this from playback.rs).
pub fn set_paused(paused: bool) {
    if let Some(h) = HANDLES.get() {
        h.tap.set_paused(paused);
        if !paused && ENABLED.load(Ordering::Relaxed) {
            // Wake the producer only — the drain is woken by the first frame
            // the resumed producer latches.
            h.fft_thread.unpark();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// §4.2a: the effective enable is the OR of the source bits.
    #[test]
    fn source_bits_or_to_the_effective_enable() {
        let mut mask = 0;
        assert!(!mask_enabled(mask));
        mask = mask_with(mask, DOCK_BIT, true);
        assert!(mask_enabled(mask));
        // Immersive on top of dock: still enabled, ONE logical state.
        mask = mask_with(mask, IMMERSIVE_BIT, true);
        assert!(mask_enabled(mask));
        assert_eq!(mask, DOCK_BIT | IMMERSIVE_BIT);
    }

    /// Trap 4/20: clearing the immersive bit NEVER clears the dock's bit, and
    /// vice versa — the tap only parks when the LAST source lets go.
    #[test]
    fn closing_one_source_keeps_the_other() {
        let both = DOCK_BIT | IMMERSIVE_BIT;
        let after_immersive_close = mask_with(both, IMMERSIVE_BIT, false);
        assert_eq!(after_immersive_close, DOCK_BIT);
        assert!(mask_enabled(after_immersive_close));
        let after_dock_off = mask_with(both, DOCK_BIT, false);
        assert_eq!(after_dock_off, IMMERSIVE_BIT);
        assert!(mask_enabled(after_dock_off));
        let none = mask_with(after_immersive_close, DOCK_BIT, false);
        assert!(!mask_enabled(none));
    }

    /// The dock wake table: exactly ONE stream per render mode.
    #[test]
    fn single_stream_wake_table() {
        let streams = [
            Stream::Bars,
            Stream::Waveform,
            Stream::Energy,
            Stream::Goniometer,
            Stream::Oscilloscope,
        ];
        for mode in 0..=4 {
            let wakes: Vec<bool> = streams
                .into_iter()
                .map(|s| stream_wakes(mode, false, s))
                .collect();
            assert_eq!(wakes.iter().filter(|w| **w).count(), 1, "mode {mode}");
        }
        assert!(stream_wakes(0, false, Stream::Bars));
        assert!(stream_wakes(1, false, Stream::Waveform));
        assert!(stream_wakes(2, false, Stream::Energy));
        assert!(stream_wakes(3, false, Stream::Goniometer));
        assert!(stream_wakes(4, false, Stream::Oscilloscope));
    }

    /// §4.2b: marshal-all wakes on every legacy stream regardless of
    /// ACTIVE_MODE. Scope frames hitchhike on that fixed cadence, avoiding
    /// extra drain wakeups while immersive is open.
    #[test]
    fn marshal_all_wakes_every_stream() {
        for mode in 0..=2 {
            for s in [Stream::Bars, Stream::Waveform, Stream::Energy] {
                assert!(stream_wakes(mode, true, s), "mode {mode} stream {s:?}");
            }
            assert!(!stream_wakes(mode, true, Stream::Goniometer));
            assert!(!stream_wakes(mode, true, Stream::Oscilloscope));
        }
    }

    #[test]
    fn shader_scene_suppresses_hidden_scope() {
        assert_eq!(visible_scope_mask(GONIOMETER_BIT, false), GONIOMETER_BIT);
        assert_eq!(
            visible_scope_mask(OSCILLOSCOPE_BIT, false),
            OSCILLOSCOPE_BIT
        );
        assert_eq!(visible_scope_mask(GONIOMETER_BIT, true), 0);
        assert_eq!(visible_scope_mask(OSCILLOSCOPE_BIT, true), 0);
    }

    /// The hooks against the REAL statics (kept in ONE test so parallel test
    /// threads cannot race the shared atomics): open sets the immersive bit +
    /// marshal-all; close clears both and restores ACTIVE_MODE from the
    /// dock's persisted pref. HANDLES is unset here, so the effective-enable
    /// application is a no-op and only the masks/flags move.
    #[test]
    fn immersive_hooks_switch_and_restore() {
        // Selecting a scope while the dock is hidden only records the mode;
        // its DSP request stays off until the dock becomes visible.
        set_mode(3);
        assert_eq!(DOCK_SCOPE_MASK.load(Ordering::Relaxed), 0);

        // Dock was on (the common case: dock drives the tap, then immersive
        // opens on top).
        set_enabled(true);
        assert_eq!(ENABLED_MASK.load(Ordering::Relaxed), DOCK_BIT);
        assert_eq!(DOCK_SCOPE_MASK.load(Ordering::Relaxed), GONIOMETER_BIT);

        immersive_opened();
        assert!(MARSHAL_ALL.load(Ordering::Relaxed));
        assert_eq!(
            ENABLED_MASK.load(Ordering::Relaxed),
            DOCK_BIT | IMMERSIVE_BIT
        );

        // A dock mode-cycle while immersive is open records ACTIVE_MODE but
        // the wake table stays all-three (marshal-all short-circuits it).
        set_mode(2);
        assert_eq!(DOCK_SCOPE_MASK.load(Ordering::Relaxed), 0);
        assert!(stream_wakes(
            2,
            MARSHAL_ALL.load(Ordering::Relaxed),
            Stream::Bars
        ));

        immersive_closed();
        assert!(!MARSHAL_ALL.load(Ordering::Relaxed));
        // The dock's bit survived the close (trap 4/20).
        assert_eq!(ENABLED_MASK.load(Ordering::Relaxed), DOCK_BIT);
        // ACTIVE_MODE was restored from the dock pref, not left at the cycle.
        assert_eq!(
            ACTIVE_MODE.load(Ordering::Relaxed),
            crate::settings_qt::large_spectrum_mode()
        );

        // Leave the statics as found for any later test in the process.
        set_enabled(false);
        assert_eq!(ENABLED_MASK.load(Ordering::Relaxed), 0);
        assert_eq!(DOCK_SCOPE_MASK.load(Ordering::Relaxed), 0);
    }

    // --- A1: the shader-pack derivation (visualizer.rs:203-269 parity) -----

    /// bands8 pairing + level/level_smooth math, checked against the JSON the
    /// QML layer parses (the pack's ONLY consumer contract).
    #[test]
    fn pack_bands_pairing_and_level_ema() {
        let mut p = ShaderPackState {
            last_bars16: [
                0.0, 0.2, 0.4, 0.6, 0.8, 1.0, 0.1, 0.3, 0.5, 0.7, 0.9, 0.2, 0.4, 0.6, 0.8, 1.0,
            ],
            // mean = (0.1+0.2+0.3+0.4+0.5)*0.2 = 0.3
            last_energy: [0.1, 0.2, 0.3, 0.4, 0.5],
            ..Default::default()
        };
        let json = p.tick(None, None);
        let doc: serde_json::Value = serde_json::from_str(&json).unwrap();
        // bands8[0] = (0.0+0.2)/2 = 0.1; bands8[7] = (0.8+1.0)/2 = 0.9.
        let bands_lo = doc["bandsLo"].as_array().unwrap();
        let bands_hi = doc["bandsHi"].as_array().unwrap();
        assert!((bands_lo[0].as_f64().unwrap() - 0.1).abs() < 1e-6);
        assert!((bands_hi[3].as_f64().unwrap() - 0.9).abs() < 1e-6);
        assert!((doc["level"].as_f64().unwrap() - 0.3).abs() < 1e-6);
        // First tick: level_smooth = 0*0.96 + 0.3*0.04 = 0.012.
        assert!((doc["levelSmooth"].as_f64().unwrap() - 0.012).abs() < 1e-6);
        // Second tick: 0.012*0.96 + 0.012 = 0.02352.
        let doc2: serde_json::Value = serde_json::from_str(&p.tick(None, None)).unwrap();
        assert!((doc2["levelSmooth"].as_f64().unwrap() - 0.02352).abs() < 1e-5);
        // energy_hi = [air, spectral_peak, 0, 0] (shader_underlay.rs:845).
        let energy_hi = doc["energyHi"].as_array().unwrap();
        assert!((energy_hi[0].as_f64().unwrap() - 0.5).abs() < 1e-6);
        assert_eq!(energy_hi[2].as_f64().unwrap(), 0.0);
    }

    /// The envelopes: a fresh transient MAXES into beat AND transient first,
    /// then both decay (×0.88 / ×0.85) — and the pack carries the DECAYED
    /// values, exactly like visualizer.rs:176-189.
    #[test]
    fn pack_envelopes_max_in_then_decay() {
        let mut p = ShaderPackState::default();
        let doc: serde_json::Value = serde_json::from_str(&p.tick(Some(0.5), None)).unwrap();
        assert!((doc["beat"].as_f64().unwrap() - 0.44).abs() < 1e-6); // 0.5*0.88
        assert!((doc["transient"].as_f64().unwrap() - 0.425).abs() < 1e-6); // 0.5*0.85
                                                                            // A weaker transient mid-decay does NOT lower the envelope (max-in).
        let doc: serde_json::Value = serde_json::from_str(&p.tick(Some(0.1), None)).unwrap();
        assert!((doc["beat"].as_f64().unwrap() - 0.44 * 0.88).abs() < 1e-5);
        assert!((doc["transient"].as_f64().unwrap() - 0.425 * 0.85).abs() < 1e-5);
    }

    /// The forward-motion clock: +0.012 + level*0.02 + beat*0.02 per tick,
    /// wrapping at 4096 (a multiple of 8, so the Tunnel's `& 7` ring math is
    /// seamless across the wrap — spec 01 §1).
    #[test]
    fn pack_phase_advances_and_wraps_at_4096() {
        let mut p = ShaderPackState {
            phase: 4096.0 - 0.005,
            ..Default::default()
        };
        // level = 0, beat = 0 -> +0.012 -> 4096.007 -> wraps to 0.007.
        // Tolerance 2e-3, not tighter: at the 4096 scale an f32 ulp is
        // ~4.9e-4, so the pre-wrap addition rounds by a few ulps — the test
        // pins the WRAP (and continuity), not the exact residue.
        let doc: serde_json::Value = serde_json::from_str(&p.tick(None, None)).unwrap();
        assert!((doc["phase"].as_f64().unwrap() - 0.007).abs() < 2e-3);
        // With level 0.5 and a decayed beat 0.44: +0.012+0.010+0.0088.
        let mut p = ShaderPackState {
            last_energy: [0.5; 5],
            ..Default::default()
        };
        let doc: serde_json::Value = serde_json::from_str(&p.tick(Some(0.5), None)).unwrap();
        let expected = 0.012 + 0.5 * 0.02 + 0.44 * 0.02;
        assert!((doc["phase"].as_f64().unwrap() - expected).abs() < 1e-4);
    }

    /// The spectral-peak EMA (the future mode 4): the highest bin above 0.05
    /// as a fraction, smoothed ×0.85 + 0.15·target (visualizer.rs:223-239).
    /// A single-bin / empty frame is ignored (the n > 1 guard).
    #[test]
    fn pack_spectral_peak_ema() {
        let mut p = ShaderPackState::default();
        let mut bins = vec![0.0f32; 101];
        bins[75] = 0.5; // highest active: 75/100 = 0.75
        let doc: serde_json::Value = serde_json::from_str(&p.tick(None, Some(&bins))).unwrap();
        assert!((doc["energyHi"][1].as_f64().unwrap() - 0.75 * 0.15).abs() < 1e-5);
        // Empty of signal: target 0, the EMA decays toward it.
        let doc: serde_json::Value =
            serde_json::from_str(&p.tick(None, Some(&vec![0.0f32; 101]))).unwrap();
        assert!((doc["energyHi"][1].as_f64().unwrap() - 0.75 * 0.15 * 0.85).abs() < 1e-4);
        // The n > 1 guard: a one-bin frame changes nothing.
        let before = p.spectral_peak;
        p.tick(None, Some(&[1.0]));
        assert_eq!(p.spectral_peak, before);
    }
}
