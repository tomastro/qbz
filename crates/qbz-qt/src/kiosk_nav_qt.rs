//! Kiosk zone-navigation model — the Qt counterpart of Slint's
//! `KioskNavState` global (`qbz-ui/ui/state.slint:3511-3536`) plus the five
//! movement functions that live on the kiosk shell
//! (`qbz-ui/ui/shell/KioskShell.slint:95-200`).
//!
//! # Why this is Rust and not QML
//!
//! The kiosk contract (§7, §12.1) names this model as "the piece a lazy port
//! silently degrades", and it is invisible to every static check the gate
//! runs: it has no types to resolve, no bridge members to cross-reference,
//! and clicks never exercise it (a click does not set `nav_active`, so the
//! whole model can be dead while every screenshot looks right).
//!
//! A QML `pragma Singleton` was not an option regardless —
//! `qml/theme/QbzTheme.qml:12-13` records that "the cxx-qt-build generated
//! qmldir cannot mark singletons", which is why the theme object is
//! instantiated rather than shared. An object instantiated at the kiosk
//! shell's root cannot be reached by the views either: they mount inside a
//! `Loader`, so they would have to walk `parent.parent…` — the documented
//! parent-chain binding trap, and invisible to qmllint (LESSONS L3).
//!
//! So the model is a bridge. Putting the STATE MACHINE here rather than in
//! the bridge's QML surface is what buys the thing screenshots cannot give:
//! the transition table below is covered by `cargo test`, which is step 2 of
//! the gate.
//!
//! # Fidelity
//!
//! Every transition mirrors `KioskShell.slint:95-174` line for line. Two
//! deliberate departures, both recorded in the contract's §10 divergence log:
//!
//! * **`columns` is clamped to >= 1.** Slint evaluates `Math.mod(x, 0)` to
//!   NaN and the comparison quietly fails; Rust's `%` panics. A view that
//!   published `columns = 0` would take the whole app down. No behavioural
//!   change for any legal value.
//! * **D5 — Up from the first row of a view with no tab strip.** Slint runs
//!   `index = Math.min(tabs - 1, index - tabs)`, which for `tabs == 0` and
//!   `index == 0` is `min(-1, 0)` = **-1**. No item carries index -1, so the
//!   focus ring vanishes and a second Up is needed to reach the rail. This is
//!   live on both `KioskAlbum` (`:46` publishes `tabs = 0`) and `KioskArtist`
//!   (`:24`). Here, a view with no tab strip wraps straight to the rail —
//!   the same destination the tab-strip case reaches at
//!   `KioskShell.slint:137-138`, one keypress earlier.

/// The three focus zones. `KioskShell.slint:98,113,129` compare the Slint
/// global's string, so the bridge exposes these as the same strings; the
/// model itself uses an enum so an unknown zone cannot exist.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Zone {
    Rail,
    Content,
    Player,
}

impl Zone {
    pub fn as_str(self) -> &'static str {
        match self {
            Zone::Rail => "rail",
            Zone::Content => "content",
            Zone::Player => "player",
        }
    }
}

/// `state.slint:3511-3536`, field for field, with the same defaults.
#[derive(Clone, Debug)]
pub struct NavModel {
    pub zone: Zone,
    pub index: i32,
    pub columns: i32,
    pub count: i32,
    pub nav_active: bool,
    pub tabs: i32,
    pub shelves: bool,
    pub hmove: i32,
    pub dir: i32,
    pub activate_seq: i32,
    /// `NavRail.slint:75` — `out property <int> tile-count: 7`. Read by
    /// `nav_right` for the rail clamp (`KioskShell.slint:114`).
    pub rail_count: i32,
}

impl Default for NavModel {
    fn default() -> Self {
        Self {
            zone: Zone::Content,
            index: 0,
            columns: 1,
            count: 0,
            nav_active: false,
            tabs: 0,
            shelves: false,
            hmove: 0,
            dir: 0,
            activate_seq: 0,
            rail_count: 7,
        }
    }
}

impl NavModel {
    /// Columns, guaranteed usable as a divisor. See the module header.
    fn cols(&self) -> i32 {
        self.columns.max(1)
    }

    /// True when the view mounted below has a leading tab strip.
    fn has_tabs(&self) -> bool {
        self.tabs > 0
    }

    /// `KioskShell.slint:95-109`.
    pub fn left(&mut self) {
        self.nav_active = true;
        self.dir = 0;
        match self.zone {
            Zone::Rail => self.index = (self.index - 1).max(0),
            Zone::Content => {
                if self.index < self.tabs {
                    self.index = (self.index - 1).max(0);
                } else if self.shelves {
                    self.hmove = -1;
                } else if (self.index - self.tabs) % self.cols() > 0 {
                    self.index -= 1;
                }
            }
            Zone::Player => {}
        }
    }

    /// `KioskShell.slint:110-125`.
    pub fn right(&mut self) {
        self.nav_active = true;
        self.dir = 0;
        match self.zone {
            Zone::Rail => self.index = (self.index + 1).min(self.rail_count - 1),
            Zone::Content => {
                if self.index < self.tabs {
                    self.index = (self.index + 1).min(self.tabs - 1);
                } else if self.shelves {
                    self.hmove = 1;
                } else if (self.index - self.tabs) % self.cols() < self.cols() - 1
                    && self.index + 1 < self.count
                {
                    self.index += 1;
                }
            }
            Zone::Player => {}
        }
    }

    /// `KioskShell.slint:126-147`, with D5 (module header).
    pub fn up(&mut self) {
        self.nav_active = true;
        self.dir = -1;
        match self.zone {
            Zone::Content => {
                if self.index >= self.tabs + self.cols() {
                    // A row up inside the grid.
                    self.index -= self.cols();
                } else if self.index >= self.tabs && self.has_tabs() {
                    // Grid row 0 -> the tab strip above it (column -> tab).
                    self.index = (self.index - self.tabs).min(self.tabs - 1);
                } else {
                    // The tab strip is the top of the cycle -> wrap to the
                    // rail. D5: a view with NO tab strip wraps from row 0 too,
                    // instead of Slint's index = -1.
                    self.zone = Zone::Rail;
                    self.index = 0;
                }
            }
            Zone::Rail => {
                self.zone = Zone::Player;
                self.index = 0;
            }
            Zone::Player => {
                self.zone = Zone::Content;
                self.index = 0;
            }
        }
    }

    /// `KioskShell.slint:148-174`.
    pub fn down(&mut self) {
        self.nav_active = true;
        self.dir = 1;
        match self.zone {
            Zone::Content => {
                if self.index < self.tabs {
                    // Tab strip -> the first item row, or the player bar when
                    // the view carries no items at all.
                    if self.count > self.tabs {
                        self.index = self.tabs;
                    } else {
                        self.zone = Zone::Player;
                        self.index = 0;
                    }
                } else if self.index + self.cols() < self.count {
                    self.index += self.cols();
                } else {
                    self.zone = Zone::Player;
                    self.index = 0;
                }
            }
            Zone::Player => {
                self.zone = Zone::Rail;
                self.index = 0;
            }
            Zone::Rail => {
                self.zone = Zone::Content;
                self.index = 0;
            }
        }
    }

    /// The content arm of `nav-activate` (`KioskShell.slint:175-190`). The
    /// rail and player arms stay on the QML side: one calls into the mounted
    /// NavRail, the other toggles playback, and both need objects Rust cannot
    /// address. Content views are conditionally mounted and cannot be
    /// addressed from anywhere, which is why the request goes out as a pulse
    /// they answer through a `changed` probe.
    pub fn bump_activate(&mut self) {
        self.nav_active = true;
        self.activate_seq = self.activate_seq.wrapping_add(1);
    }

    /// Latch the ring on without moving (the rail and player Enter arms still
    /// have to set it — `KioskShell.slint:176`).
    pub fn mark_active(&mut self) {
        self.nav_active = true;
    }

    /// The view-switch reset (`KioskShell.slint:194-200`): focus lands on the
    /// new view's content, parked at the first item. `nav_active` deliberately
    /// SURVIVES — it is a session latch, not per-view state, so a user who has
    /// started navigating by key keeps their ring across a route change.
    pub fn reset_for_view(&mut self) {
        self.zone = Zone::Content;
        self.index = 0;
        self.dir = 0;
        self.hmove = 0;
    }

    /// What a mounted view calls whenever its tab or its data changes — the
    /// Qt spelling of the `KioskNavState.tabs/columns/count/shelves = …`
    /// blocks each Kiosk view runs (e.g. `KioskLibrary.slint:67-86`).
    ///
    /// Every one of those blocks ends in the same clamp
    /// (`KioskLibrary.slint:86` and its seven siblings):
    ///
    /// ```slint
    /// KioskNavState.index = Math.max(0, Math.min(KioskNavState.index, KioskNavState.count - 1));
    /// ```
    ///
    /// It is not optional: without it, switching from a tab with 100 items to
    /// one with 3 leaves `index` at 90, no item matches, and the focus ring
    /// simply vanishes.
    ///
    /// **D7 — the clamp is scoped to the content zone.** Slint applies it
    /// unconditionally, so a publish that lands while the focus is on the RAIL
    /// (7 tiles, indices 0-6) rewrites the rail's cursor to the content view's
    /// `count - 1`. That is reachable in an ordinary session: park on the rail,
    /// let an in-flight fetch resolve, and the highlighted tile jumps. `index`
    /// is one field shared by three zones, so the clamp only makes sense for
    /// the zone it was computed for.
    pub fn publish(&mut self, tabs: i32, columns: i32, count: i32, shelves: bool) {
        self.tabs = tabs.max(0);
        self.columns = columns.max(1);
        self.count = count.max(0);
        self.shelves = shelves;
        if self.zone == Zone::Content {
            self.index = self.index.clamp(0, (self.count - 1).max(0));
        }
    }

    /// The shelf consumer's half of the `hmove` pulse
    /// (`KioskDiscover.slint:174-182`): read it and reset to 0 in one step, so
    /// a pulse can never be applied twice.
    pub fn take_hmove(&mut self) -> i32 {
        std::mem::replace(&mut self.hmove, 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A library-shaped view: 5 tabs, a 6-wide grid, 12 albums.
    fn library() -> NavModel {
        let mut m = NavModel::default();
        m.publish(5, 6, 5 + 12, false);
        m
    }

    #[test]
    fn defaults_match_the_slint_global() {
        let m = NavModel::default();
        assert_eq!(m.zone, Zone::Content);
        assert_eq!(m.index, 0);
        assert_eq!(m.columns, 1);
        assert_eq!(m.count, 0);
        assert!(!m.nav_active);
        assert_eq!(m.tabs, 0);
        assert!(!m.shelves);
        assert_eq!(m.hmove, 0);
        assert_eq!(m.dir, 0);
        assert_eq!(m.activate_seq, 0);
    }

    #[test]
    fn first_nav_key_latches_the_ring() {
        let mut m = library();
        assert!(!m.nav_active);
        m.right();
        assert!(m.nav_active, "the ring gates on this; clicks never set it");
    }

    #[test]
    fn tab_strip_moves_within_its_own_range() {
        let mut m = library();
        for _ in 0..10 {
            m.right();
        }
        assert_eq!(m.index, 4, "clamped to tabs - 1");
        for _ in 0..10 {
            m.left();
        }
        assert_eq!(m.index, 0);
    }

    #[test]
    fn down_from_tabs_lands_on_the_first_item_row() {
        let mut m = library();
        m.down();
        assert_eq!(m.zone, Zone::Content);
        assert_eq!(m.index, 5, "= tabs");
    }

    #[test]
    fn down_from_tabs_skips_to_the_player_when_the_view_is_empty() {
        let mut m = NavModel::default();
        m.publish(5, 6, 5, false); // 5 tabs, zero items
        m.down();
        assert_eq!(m.zone, Zone::Player);
        assert_eq!(m.index, 0);
    }

    #[test]
    fn grid_right_stops_at_the_row_edge_and_at_the_end_of_the_data() {
        let mut m = library();
        m.down(); // index 5, grid row 0 col 0
        for _ in 0..10 {
            m.right();
        }
        assert_eq!(m.index, 10, "column 5 of 6 — the row edge, not the count");

        // Last row is partial: 12 albums = 6 + 6, so indices 11..16.
        let mut m = NavModel::default();
        m.publish(5, 6, 5 + 9, false); // 9 albums: rows of 6 then 3
        m.down();
        for _ in 0..3 {
            m.down();
        }
        // Row 1 = indices 11,12,13 (3 items). Walk right off the end.
        let before = m.index;
        for _ in 0..10 {
            m.right();
        }
        assert!(
            m.index < m.count,
            "never past the data ({before} -> {})",
            m.index
        );
    }

    #[test]
    fn grid_up_from_row_zero_reaches_the_tab_strip() {
        let mut m = library();
        m.down(); // index 5
        m.right(); // index 6, column 1
        m.up();
        assert_eq!(m.index, 1, "column 1 -> tab 1");
        assert_eq!(m.zone, Zone::Content);
    }

    #[test]
    fn grid_up_from_a_far_column_clamps_onto_the_last_tab() {
        let mut m = library();
        m.down();
        for _ in 0..5 {
            m.right();
        } // index 10, column 5
        m.up();
        assert_eq!(
            m.index, 4,
            "min(tabs - 1, column) with 5 tabs and 6 columns"
        );
    }

    #[test]
    fn up_from_the_tab_strip_wraps_to_the_rail() {
        let mut m = library();
        m.up();
        assert_eq!(m.zone, Zone::Rail);
        assert_eq!(m.index, 0);
    }

    /// D5. Slint leaves `index` at -1 here and the ring vanishes until a
    /// second Up. `KioskAlbum.slint:46` and `KioskArtist.slint:24` both
    /// publish `tabs = 0`, so this is live on two shipped views.
    #[test]
    fn up_from_row_zero_of_a_tabless_view_reaches_the_rail_in_one_press() {
        let mut m = NavModel::default();
        m.publish(0, 1, 14, false); // KioskAlbum: 14 tracks, no tabs
        m.up();
        assert_eq!(m.zone, Zone::Rail);
        assert_eq!(m.index, 0);
        assert!(m.index >= 0, "the Slint original produces -1 here");
    }

    #[test]
    fn the_zone_cycle_is_closed_in_both_directions() {
        let mut m = library();
        // Down from content with nothing below -> player -> rail -> content.
        m.zone = Zone::Player;
        m.down();
        assert_eq!(m.zone, Zone::Rail);
        m.down();
        assert_eq!(m.zone, Zone::Content);
        // Up reverses.
        m.up();
        assert_eq!(m.zone, Zone::Rail);
        m.up();
        assert_eq!(m.zone, Zone::Player);
        m.up();
        assert_eq!(m.zone, Zone::Content);
    }

    #[test]
    fn walking_down_a_grid_ends_on_the_player() {
        let mut m = library(); // 12 items, 6 columns -> 2 rows
        m.down(); // tabs -> row 0
        m.down(); // row 0 -> row 1
        assert_eq!(m.index, 11);
        m.down(); // past the last row
        assert_eq!(m.zone, Zone::Player);
    }

    #[test]
    fn rail_index_clamps_to_the_tile_count() {
        let mut m = library();
        m.up(); // -> rail
        for _ in 0..20 {
            m.right();
        }
        assert_eq!(m.index, 6, "7 tiles, NavRail.slint:75");
        for _ in 0..20 {
            m.left();
        }
        assert_eq!(m.index, 0);
    }

    #[test]
    fn shelf_views_pulse_hmove_instead_of_moving_the_index() {
        let mut m = NavModel::default();
        m.publish(3, 1, 3 + 4, true); // KioskDiscover: 3 tabs, 4 shelves
        m.down(); // index 3, first shelf
        let at = m.index;
        m.right();
        assert_eq!(m.index, at, "index must not move between shelves");
        assert_eq!(m.hmove, 1);
        assert_eq!(m.take_hmove(), 1);
        assert_eq!(m.hmove, 0, "the consumer resets it");
        assert_eq!(m.take_hmove(), 0, "a pulse is never applied twice");
        m.left();
        assert_eq!(m.hmove, -1);
    }

    #[test]
    fn tab_strip_left_right_never_pulses_hmove_even_in_a_shelf_view() {
        let mut m = NavModel::default();
        m.publish(3, 1, 3 + 4, true);
        m.right(); // still inside the tab strip
        assert_eq!(m.index, 1);
        assert_eq!(m.hmove, 0);
    }

    #[test]
    fn dir_records_the_last_vertical_direction() {
        let mut m = library();
        m.down();
        assert_eq!(m.dir, 1);
        m.up();
        assert_eq!(m.dir, -1);
        m.right();
        assert_eq!(m.dir, 0, "horizontal moves clear it");
    }

    #[test]
    fn the_view_switch_reset_keeps_the_session_latch() {
        let mut m = library();
        m.down();
        m.right();
        m.up(); // leaves dir at -1
        m.reset_for_view();
        assert_eq!(m.zone, Zone::Content);
        assert_eq!(m.index, 0);
        assert_eq!(m.dir, 0);
        assert_eq!(m.hmove, 0);
        assert!(m.nav_active, "the ring is a session latch, not per-view");
    }

    #[test]
    fn activate_pulses_monotonically() {
        let mut m = library();
        let before = m.activate_seq;
        m.bump_activate();
        m.bump_activate();
        assert_eq!(m.activate_seq, before + 2);
        assert!(m.nav_active);
    }

    #[test]
    fn a_republish_clamps_the_index_into_the_new_data() {
        let mut m = library(); // 5 tabs, 6 columns, 12 albums -> count 17
        m.down();
        for _ in 0..4 {
            m.right();
        }
        assert_eq!(m.index, 9);
        // The user switches to a tab with 3 items.
        m.publish(5, 6, 5 + 3, false);
        assert_eq!(m.index, 7, "clamped to count - 1, so an item still owns it");
        assert!(m.index < m.count);
    }

    #[test]
    fn a_publish_with_no_items_parks_on_the_first_tab() {
        let mut m = library();
        m.down(); // into the grid
        m.publish(5, 6, 0, false);
        assert_eq!(m.index, 0, "max(0, -1)");
    }

    /// D7. Slint clamps unconditionally, so a publish arriving while the focus
    /// sits on the rail rewrites the RAIL's cursor to the content view's
    /// `count - 1`.
    #[test]
    fn a_publish_never_disturbs_the_rail_cursor() {
        let mut m = library();
        m.up(); // -> rail
        for _ in 0..3 {
            m.right();
        }
        assert_eq!(m.zone, Zone::Rail);
        assert_eq!(m.index, 3, "the MyQBZ tile");
        // An in-flight fetch resolves and the mounted view republishes.
        m.publish(5, 6, 5 + 1, false);
        assert_eq!(m.index, 3, "still the MyQBZ tile");
        assert_eq!(m.zone, Zone::Rail);
    }

    #[test]
    fn a_zero_column_publish_cannot_panic() {
        let mut m = NavModel::default();
        m.publish(0, 0, 5, false);
        assert_eq!(m.columns, 1, "clamped at the publish seam");
        m.down();
        m.right();
        m.left();
        m.up();
    }

    /// The player zone is ONE focusable unit — arrows do not move inside it
    /// (`KioskShell.slint:177-181`: Enter is the only thing it answers).
    #[test]
    fn the_player_zone_ignores_horizontal_movement() {
        let mut m = library();
        m.zone = Zone::Player;
        m.index = 0;
        m.left();
        m.right();
        assert_eq!(m.index, 0);
        assert_eq!(m.zone, Zone::Player);
    }
}
