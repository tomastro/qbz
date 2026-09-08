//! QbzKioskNav — the kiosk zone-navigation bridge.
//!
//! The Qt counterpart of Slint's `KioskNavState` global
//! (`qbz-ui/ui/state.slint:3511-3536`). The state machine itself lives in
//! `kiosk_nav_qt.rs`, where `cargo test` can reach it; this file is the thin
//! Qt surface over it. See that module's header for why the model is Rust and
//! not QML, and for the two documented departures from the Slint original.
//!
//! The module is `qbz_kiosk_nav_bridge`, not `qbz_kiosk_nav`: bridge modules
//! are never named after a workspace crate (album_bridge.rs:4-7 records the
//! `qbz_library` collision that set the rule). Flat in `src/`, like every
//! other bridge — QTBUG-93443.
//!
//! **There is no `boot()` and no `CxxQtThread`, deliberately.** Every other
//! bridge has one because Rust publishes INTO it from a worker thread, and
//! the hop is what keeps those publishes from being dropped. Nothing in Rust
//! ever writes this object: QML moves the focus, QML reads it back. A boot()
//! here would be a dead function that looks load-bearing, and Main.qml would
//! grow a line implying a hop that does not exist.

use std::pin::Pin;

use cxx_qt::CxxQtType as _;
use cxx_qt_lib::QString;

use crate::kiosk_nav_qt::NavModel;

#[cxx_qt::bridge]
pub mod qbz_kiosk_nav_bridge {
    extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
    }

    #[auto_cxx_name]
    extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qml_singleton]
        // --- The zone model, 1:1 with state.slint:3512-3535 ----------------
        /// "rail" | "content" | "player".
        #[qproperty(QString, zone)]
        /// The focused item inside the zone.
        #[qproperty(i32, index)]
        /// Grid width of the content index space (1 = list or shelves).
        #[qproperty(i32, columns)]
        /// tabs + the mounted view's item count.
        #[qproperty(i32, count)]
        /// Latches true on the first nav key; gates EVERY focus ring, so a
        /// touch-only session never shows one.
        #[qproperty(bool, nav_active)]
        /// Leading tab-strip entries inside the content index space.
        #[qproperty(i32, tabs)]
        /// Shelf mode: content rows are horizontal strips with their own
        /// cursor, so Left/Right pulse `hmove` instead of moving `index`.
        #[qproperty(bool, shelves)]
        /// Pending shelf move, -1 | +1. The focused shelf consumes it.
        #[qproperty(i32, hmove)]
        /// Last vertical direction (-1 up / +1 down / 0 none). Shelf views
        /// skip non-navigable rows by nudging one more step this way.
        #[qproperty(i32, dir)]
        /// Enter pulse for conditionally-mounted content views.
        #[qproperty(i32, activate_seq)]
        type QbzKioskNav = super::QbzKioskNavRust;

        /// Arrow keys, from the kiosk shell's root key handler.
        #[qinvokable]
        fn nav_left(self: Pin<&mut QbzKioskNav>);
        #[qinvokable]
        fn nav_right(self: Pin<&mut QbzKioskNav>);
        #[qinvokable]
        fn nav_up(self: Pin<&mut QbzKioskNav>);
        #[qinvokable]
        fn nav_down(self: Pin<&mut QbzKioskNav>);

        /// The content arm of Enter: bump the pulse the mounted view answers.
        /// The rail and player arms stay in QML — see kiosk_nav_qt.rs.
        #[qinvokable]
        fn bump_activate(self: Pin<&mut QbzKioskNav>);
        /// Latch the ring on without moving (the rail/player Enter arms).
        #[qinvokable]
        fn mark_active(self: Pin<&mut QbzKioskNav>);

        /// Every route change: focus lands on the new view's content at item
        /// 0. `nav_active` survives — it is a session latch.
        #[qinvokable]
        fn reset_for_view(self: Pin<&mut QbzKioskNav>);

        /// What a mounted view calls when its tab or its data changes.
        #[qinvokable]
        fn publish_nav(
            self: Pin<&mut QbzKioskNav>,
            tabs: i32,
            columns: i32,
            count: i32,
            shelves: bool,
        );

        /// Read the pending shelf move and clear it in one step, so a pulse
        /// can never be applied twice.
        #[qinvokable]
        fn take_hmove(self: Pin<&mut QbzKioskNav>) -> i32;
    }
}

use qbz_kiosk_nav_bridge::QbzKioskNav;

/// Rust side. The model is the single source of truth; the qproperties are a
/// mirror pushed after every mutation, because QML can only bind to those.
pub struct QbzKioskNavRust {
    model: NavModel,
    zone: QString,
    index: i32,
    columns: i32,
    count: i32,
    nav_active: bool,
    tabs: i32,
    shelves: bool,
    hmove: i32,
    dir: i32,
    activate_seq: i32,
}

impl Default for QbzKioskNavRust {
    fn default() -> Self {
        let model = NavModel::default();
        Self {
            zone: QString::from(model.zone.as_str()),
            index: model.index,
            columns: model.columns,
            count: model.count,
            nav_active: model.nav_active,
            tabs: model.tabs,
            shelves: model.shelves,
            hmove: model.hmove,
            dir: model.dir,
            activate_seq: model.activate_seq,
            model,
        }
    }
}

impl QbzKioskNav {
    /// Copy the model onto the qproperties. cxx-qt's setters only emit their
    /// NOTIFY when the value actually changes, so calling this after every
    /// mutation costs nothing for the fields that did not move.
    ///
    /// **The ORDER is load-bearing.** Each setter emits its NOTIFY
    /// synchronously, so a QML handler on one property runs while the fields
    /// written after it still hold their PREVIOUS values.
    ///
    /// THREE properties are consumer-facing — the ones a view has a handler on:
    /// `hmove` (shelf pulse), `index` (focus moved) and `activate_seq` (Enter).
    /// Everything else is CONTEXT those handlers read: which zone, which
    /// direction, how big the space is, and **whether the ring is live at all**.
    /// So context is written first and the three last.
    ///
    /// Both halves of that rule were learned the hard way, one after the other:
    ///
    /// * With `dir` after `index`, the shelf skip-cascade in `KioskArtist.qml`
    ///   read `dir == 0` on the first Down of a session and refused to skip,
    ///   parking the ring on a shelf of zero height; a direction reversal read
    ///   the old sign and skipped back the way it came.
    /// * Then, fixing that, `nav_active` was moved BELOW `hmove` — and the
    ///   first arrow key of a session sets both in one model call, so
    ///   `hmoveChanged` fired while `nav_active` was still false, no shelf
    ///   consumed the pulse, and — because cxx-qt's generated setters are
    ///   change-gated — `hmove` then stayed at 1 and every subsequent Right
    ///   emitted NOTHING. Left/Right dead on the kiosk's landing page. Caught
    ///   by an adversarial reviewer reading the diff, not by any test.
    ///
    /// The QML side does NOT rely on this alone — the skip handlers also defer
    /// through `Qt.callLater`, which additionally covers Qt's own binding
    /// re-evaluation order (the hazard written up at `qml/Main.qml:233-247`).
    /// Two independent guards, because this class of bug is invisible to every
    /// static check and shows up as a focus ring that lands somewhere silly.
    fn sync(mut self: Pin<&mut Self>) {
        let m = self.model.clone();
        // --- context: everything a consumer's handler needs to READ ---
        let zone = QString::from(m.zone.as_str());
        if *self.zone() != zone {
            self.as_mut().set_zone(zone);
        }
        self.as_mut().set_columns(m.columns);
        self.as_mut().set_count(m.count);
        self.as_mut().set_tabs(m.tabs);
        self.as_mut().set_shelves(m.shelves);
        self.as_mut().set_dir(m.dir);
        self.as_mut().set_nav_active(m.nav_active);
        // --- the three consumers have handlers ON, last ---
        self.as_mut().set_hmove(m.hmove);
        self.as_mut().set_index(m.index);
        self.as_mut().set_activate_seq(m.activate_seq);
    }

    /// Re-arm the shelf pulse before publishing a new one.
    ///
    /// `hmove` is the one property whose VALUE is not its meaning — the
    /// meaning is the edge. It is cleared only by a consumer calling
    /// `takeHmove()`, so if nobody consumed the last one (no shelf focused,
    /// the ring not live yet, a skipped shelf) the model still holds it. The
    /// next Right would then write the same `1` over the same `1`, and
    /// cxx-qt's generated setter is CHANGE-GATED — it returns without emitting
    /// — so the key would be silently dead until the user happened to press
    /// the other direction.
    ///
    /// Forcing the property to 0 first guarantees a real 0 → ±1 transition
    /// every time, whatever the consumers did or did not do.
    fn rearm_hmove(mut self: Pin<&mut Self>) {
        self.as_mut().set_hmove(0);
    }

    pub fn nav_left(mut self: Pin<&mut Self>) {
        self.as_mut().rust_mut().model.left();
        self.as_mut().rearm_hmove();
        self.sync();
    }

    pub fn nav_right(mut self: Pin<&mut Self>) {
        self.as_mut().rust_mut().model.right();
        self.as_mut().rearm_hmove();
        self.sync();
    }

    pub fn nav_up(mut self: Pin<&mut Self>) {
        self.as_mut().rust_mut().model.up();
        self.sync();
    }

    pub fn nav_down(mut self: Pin<&mut Self>) {
        self.as_mut().rust_mut().model.down();
        self.sync();
    }

    pub fn bump_activate(mut self: Pin<&mut Self>) {
        self.as_mut().rust_mut().model.bump_activate();
        self.sync();
    }

    pub fn mark_active(mut self: Pin<&mut Self>) {
        self.as_mut().rust_mut().model.mark_active();
        self.sync();
    }

    pub fn reset_for_view(mut self: Pin<&mut Self>) {
        self.as_mut().rust_mut().model.reset_for_view();
        self.sync();
    }

    pub fn publish_nav(
        mut self: Pin<&mut Self>,
        tabs: i32,
        columns: i32,
        count: i32,
        shelves: bool,
    ) {
        self.as_mut()
            .rust_mut()
            .model
            .publish(tabs, columns, count, shelves);
        self.sync();
    }

    pub fn take_hmove(mut self: Pin<&mut Self>) -> i32 {
        let taken = self.as_mut().rust_mut().model.take_hmove();
        self.sync();
        taken
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kiosk_nav_qt::Zone;

    #[test]
    fn zone_strings_match_the_slint_comparisons() {
        // KioskShell.slint:98,113,129 compare against these exact literals.
        assert_eq!(Zone::Rail.as_str(), "rail");
        assert_eq!(Zone::Content.as_str(), "content");
        assert_eq!(Zone::Player.as_str(), "player");
    }

    #[test]
    fn the_bridge_defaults_mirror_the_model_defaults() {
        let r = QbzKioskNavRust::default();
        assert_eq!(r.zone, QString::from("content"));
        assert_eq!(r.index, 0);
        assert_eq!(r.columns, 1);
        assert_eq!(r.count, 0);
        assert!(!r.nav_active);
        assert_eq!(r.tabs, 0);
        assert!(!r.shelves);
        assert_eq!(r.hmove, 0);
        assert_eq!(r.dir, 0);
        assert_eq!(r.activate_seq, 0);
    }
}
