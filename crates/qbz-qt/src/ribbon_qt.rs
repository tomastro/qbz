//! Spectral Ribbon (immersive shader scene, mode 4) — the CPU half.
//!
//! Block A3 of the 2026-08-15 immersive-completion contract (spec 01 §2.4).
//! Ports the publisher side of the Slint spectrogram feed
//! (`crates/qbz/src/visualizer.rs:240-256` + `shader_underlay.rs:855-929`):
//! per fresh Spectral512 frame, one 512-byte row (u8 per band,
//! `clamp(0,1)*255`), written at the playback-time column
//! `progress * (COLS-1)`, with a reset (full clear) on track change / seek.
//!
//! The GPU half is the C++ `RibbonItem` (`cxx/ribbon_item.{h,cpp}`), which
//! owns the persistent R8 texture, the gap-fill (the Slint SPECTRO_LAST_COL
//! logic lives renderer-side here, next to the texture it describes) and
//! the one-pass colorizer draw. This module only builds the self-describing
//! frame the item consumes (the layout is pinned between the two files and
//! `shader_scene_bridge.rs`):
//!
//! ```text
//! bytes 0..4   column (u32 LE) — playback-time column, 0..2047
//! byte  4      reset flag (0/1) — full clear, applied BEFORE the row write
//! bytes 5..517 the 512-band row
//! ```

/// Pinned with `cxx/ribbon_item.cpp` and the Slint reference
/// (`shader_underlay.rs:44-45`).
pub(crate) const SPECTRO_BANDS: usize = 512;
pub(crate) const SPECTRO_COLS: u32 = 2048;
pub(crate) const FRAME_BYTES: usize = 4 + 1 + SPECTRO_BANDS;

/// Drain-local ribbon state (the viz drain thread — the linebed_qt.rs
/// pattern: per-tick state, never shared, never locked). The reference keeps
/// `last_track_id`/`last_progress` as drain-closure locals
/// (visualizer.rs:114-116); the Slint thread-local SPECTRO_LAST_COL moves
/// renderer-side with the texture (see the header).
#[derive(Default)]
pub(crate) struct RibbonState {
    last_track_id: u64,
    last_progress: f32,
}

impl RibbonState {
    /// Build the frame for one fresh Spectral512 row, or `None` when the row
    /// is empty. `cursor` is `(track_id, progress 0..1)` from
    /// `now_playing::ribbon_cursor` (second-granular, like the reference's
    /// NowPlayingState read).
    ///
    /// Reset rule VERBATIM from visualizer.rs:248-250: track change, a
    /// backward step past 0.01, or a forward JUMP past 0.15 (a seek — the
    /// ~1 Hz progress granularity means normal playback never moves more
    /// than that between ticks except on VERY short tracks... exactly as the
    /// reference has it).
    pub(crate) fn frame(&mut self, bins: &[f32], cursor: (u64, f32)) -> Option<Vec<u8>> {
        if bins.is_empty() {
            return None;
        }
        let (track_id, progress) = cursor;
        let reset = track_id != self.last_track_id
            || progress + 0.01 < self.last_progress
            || progress > self.last_progress + 0.15;
        self.last_track_id = track_id;
        self.last_progress = progress;

        let col = (progress.clamp(0.0, 1.0) * (SPECTRO_COLS - 1) as f32) as u32;
        let mut out = Vec::with_capacity(FRAME_BYTES);
        out.extend_from_slice(&col.to_le_bytes());
        out.push(u8::from(reset));
        for i in 0..SPECTRO_BANDS {
            let v = if i < bins.len() {
                (bins[i].clamp(0.0, 1.0) * 255.0) as u8
            } else {
                0
            };
            out.push(v);
        }
        Some(out)
    }
}

// ---------------------------------------------------------------------------
// QML type registration anchor
// ---------------------------------------------------------------------------

extern "C" {
    /// Defined in `cxx/ribbon_item.cpp`. Registers `RibbonItem` with the
    /// QML type system (idempotent — guarded C++ side).
    fn qbz_ribbon_register_qml_type();
}

/// Call the C++ registration — the linebed_qt.rs idiom: the C++ lives in a
/// STATIC LIB, so the linker only pulls its object file to resolve an
/// undefined symbol; the call from `QbzShaderScene::boot` is that reference
/// (the registration itself already ran at QGuiApplication construction via
/// Q_COREAPP_STARTUP_FUNC).
pub(crate) fn register_qml_item() {
    // SAFETY: no arguments, touches only Qt's global type registry (we are
    // on the GUI thread — boot() runs from QML), idempotent.
    unsafe { qbz_ribbon_register_qml_type() };
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The frame layout is pinned with cxx/ribbon_item.cpp: 4-byte col,
    /// 1-byte reset, 512-byte row.
    #[test]
    fn frame_layout_is_pinned() {
        let mut st = RibbonState::default();
        let bins = vec![0.5f32; 512];
        let f = st.frame(&bins, (7, 0.25)).expect("non-empty row");
        assert_eq!(f.len(), FRAME_BYTES);
        let col = u32::from_le_bytes([f[0], f[1], f[2], f[3]]);
        assert_eq!(col, (0.25 * (SPECTRO_COLS - 1) as f32) as u32);
        assert_eq!(f[4], 1, "first frame for a track resets");
        assert!(f[5..].iter().all(|&b| b == 127)); // 0.5 * 255 = 127.5 -> 127
    }

    /// The reset rule (visualizer.rs:248-250 verbatim): same track + small
    /// forward progress does NOT reset; a backward step, a forward jump past
    /// 0.15, or a track change DO.
    #[test]
    fn reset_rule_matches_the_reference() {
        let mut st = RibbonState::default();
        let bins = vec![0.1f32; 8]; // shorter than 512 — zero-padded
        let f = st.frame(&bins, (1, 0.0)).unwrap();
        assert_eq!(f[4], 1, "first ever frame resets (track 0 -> 1)");
        let f = st.frame(&bins, (1, 0.05)).unwrap();
        assert_eq!(f[4], 0, "normal progress keeps the texture");
        let f = st.frame(&bins, (1, 0.045)).unwrap();
        assert_eq!(f[4], 0, "a sub-0.01 backward nudge is NOT a seek");
        let f = st.frame(&bins, (1, 0.02)).unwrap();
        assert_eq!(f[4], 1, "a backward step past 0.01 is a seek");
        let f = st.frame(&bins, (1, 0.20)).unwrap();
        assert_eq!(f[4], 1, "a forward jump past 0.15 is a seek");
        let f = st.frame(&bins, (2, 0.20)).unwrap();
        assert_eq!(f[4], 1, "a track change resets");
        let f = st.frame(&bins, (2, 0.21)).unwrap();
        assert_eq!(f[4], 0);
        // Rows shorter than 512 pad with zeros.
        assert!(f[5 + 8..].iter().all(|&b| b == 0));
        assert!(st.frame(&[], (2, 0.22)).is_none(), "an empty row skips");
    }
}
