// crates/qbzd/src/tui/screens/wizard.rs — the HiFi/DAC Wizard (FB4), the setup
// TUI's SEVENTH section (owner-sanctioned cap break). A six-step content-frame
// flow: Welcome → Check → Select DACs → Review → Test → Done.
//
// The heavy, frontend-agnostic logic is COPIED into `tui/wizard_core.rs` (from
// the Slint `qbz-dac-wizard` crate, which the slint-free daemon must not link).
// This screen owns the transient step state + rendering and asks the App to run
// the blocking probes on a worker (NEVER on the render thread, §5.5). The owner
// emphasis — copyable generated config blocks — is the Review step: one bordered
// block per DAC, `c`/`C` copy, `w` saves under ~/qbzd-wizard/ (never a system
// path). Clipboard tiers live in `tui/clipboard.rs` (SSH-first OSC 52).

use std::time::{Duration, Instant};

use qbz_audio::{
    detect_distro, detect_init, detect_sandbox, AudioStackHealth, Distro, InitSystem,
    NegotiatedRate, Sandbox,
};
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::tui::app::{DrawCtx, ScreenAction};
use crate::tui::clipboard::{self, ClipEnv};
use crate::tui::strings as s;
use crate::tui::theme;
use crate::tui::widgets::{self, InputOutcome, SelectOutcome, SelectPopup, TextInput};
use crate::tui::wizard_core::{self, DacCandidateData, DacConfigData};

/// How long a per-block `copied ✓` / save flash stays lit.
const FLASH: Duration = Duration::from_secs(2);
/// The status-line flash (copy-all / save-path) lingers a touch longer.
const STATUS_FLASH: Duration = Duration::from_secs(4);

// ============================ step transition table ============================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WStep {
    Welcome,
    Check,
    SelectDacs,
    Review,
    Test,
    Done,
}

/// The linear order of the six steps (drives next/prev + the breadcrumb).
pub const STEP_ORDER: [WStep; 6] = [
    WStep::Welcome,
    WStep::Check,
    WStep::SelectDacs,
    WStep::Review,
    WStep::Test,
    WStep::Done,
];

fn step_index(step: WStep) -> usize {
    STEP_ORDER.iter().position(|s| *s == step).unwrap_or(0)
}

/// The next step (None at Done). Pure — the step-transition table test pins it.
pub fn next_step(step: WStep) -> Option<WStep> {
    STEP_ORDER.get(step_index(step) + 1).copied()
}

/// The previous step (None at Welcome).
pub fn prev_step(step: WStep) -> Option<WStep> {
    let i = step_index(step);
    if i == 0 {
        None
    } else {
        STEP_ORDER.get(i - 1).copied()
    }
}

impl WStep {
    /// The breadcrumb / step-name (`Wizard › <this>`).
    pub fn title(self) -> &'static str {
        match self {
            WStep::Welcome => s::WIZ_STEP_WELCOME,
            WStep::Check => s::WIZ_STEP_CHECK,
            WStep::SelectDacs => s::WIZ_STEP_SELECT,
            WStep::Review => s::WIZ_STEP_REVIEW,
            WStep::Test => s::WIZ_STEP_TEST,
            WStep::Done => s::WIZ_STEP_DONE,
        }
    }
}

// ============================ per-step sub-state ============================

/// Which Check-step override the select popup is editing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CheckField {
    Distro,
    Init,
}

/// A DAC row on the Select-DACs step (checkbox + probed rates).
#[derive(Debug, Clone)]
struct Candidate {
    id: String,
    description: String,
    bus: String,
    is_default: bool,
    looks_like_dac: bool,
    rates_label: String,
    checked: bool,
}

impl Candidate {
    fn from_data(d: DacCandidateData) -> Self {
        Candidate {
            checked: d.looks_like_dac, // pre-select the likely DACs
            id: d.id,
            description: d.description,
            bus: d.bus,
            is_default: d.is_default,
            looks_like_dac: d.looks_like_dac,
            rates_label: d.rates_label,
        }
    }
}

/// A generated config + its per-block copy flash. The flash stores which
/// `Tier` won so the render can pick the right wording (OSC 52's is
/// deliberately not "copied ✓" — see `clipboard::Tier::short_label`).
struct ConfigBlock {
    data: DacConfigData,
    flash: Option<(clipboard::Tier, Instant)>,
}

// ============================ screen state ============================

pub struct WizardState {
    step: WStep,
    clip_env: ClipEnv,

    // Check step.
    health: Option<AudioStackHealth>,
    distro_index: usize,
    init_index: usize,
    sandbox: Sandbox,
    check_focus: usize, // 0 = distro override, 1 = init override
    check_editor: Option<(CheckField, SelectPopup)>,

    // Select-DACs step.
    detecting: bool,
    detected: bool,
    candidates: Vec<Candidate>,
    dac_focus: usize,
    manual: Option<TextInput>,
    manual_node: Option<String>, // last accepted, validated manual node.name
    gate_note: Option<(String, Instant)>,

    // Review step.
    configs: Vec<ConfigBlock>,
    review_focus: usize,
    review_scroll: u16,
    status_flash: Option<(String, Instant)>,

    // Test step.
    tested: bool,
    test_requested: Option<(u32, u32)>, // (rate_hz, bit_depth)
    test_negotiated: Option<NegotiatedRate>,
    test_note: Option<String>,
}

impl WizardState {
    pub fn new() -> Self {
        WizardState {
            step: WStep::Welcome,
            clip_env: ClipEnv::from_env(),
            health: None,
            distro_index: 0,
            init_index: 0,
            sandbox: Sandbox::None,
            check_focus: 0,
            check_editor: None,
            detecting: false,
            detected: false,
            candidates: Vec::new(),
            dac_focus: 0,
            manual: None,
            manual_node: None,
            gate_note: None,
            configs: Vec::new(),
            review_focus: 0,
            review_scroll: 0,
            status_flash: None,
            tested: false,
            test_requested: None,
            test_negotiated: None,
            test_note: None,
        }
    }

    // -------------------------- App-facing interface --------------------------
    //
    // The wizard never edits a persistent store — it is never "dirty" (03: the
    // dirty/save model does not apply; the App treats Wizard as clean, and Esc
    // mid-flow confirms abandon instead).

    /// A field editor (Check override select, or the manual node input) owns the
    /// keyboard — the shell must not steal number keys / focus moves.
    pub fn is_editing(&self) -> bool {
        self.check_editor.is_some() || self.manual.is_some()
    }

    /// The breadcrumb's level-2 node is always the current STEP (`Wizard › …`),
    /// so the operator can see where they are in the flow.
    pub fn editing_label(&self) -> Option<&'static str> {
        Some(self.step.title())
    }

    /// Whether the current step consumes ←/→ itself (step back/next) — true
    /// for every step except Welcome, where nothing in the content claims ←
    /// (the CTA only listens for Enter/Space): there, ← should behave like
    /// every other section and drop focus back to the sidebar instead of
    /// being silently swallowed by a no-op `retreat()`.
    pub fn claims_horizontal(&self) -> bool {
        self.step != WStep::Welcome
    }

    /// Step-specific help bar.
    pub fn help_text(&self) -> &'static str {
        if self.check_editor.is_some() {
            return s::HELP_SELECT;
        }
        if self.manual.is_some() {
            return s::HELP_INPUT;
        }
        match self.step {
            WStep::Welcome => s::WIZ_HELP_WELCOME,
            WStep::Check => s::WIZ_HELP_CHECK,
            WStep::SelectDacs => s::WIZ_HELP_SELECT,
            WStep::Review => s::WIZ_HELP_REVIEW,
            WStep::Test => s::WIZ_HELP_TEST,
            WStep::Done => s::WIZ_HELP_DONE,
        }
    }

    // -------------------------- worker results --------------------------

    /// Apply a completed audio-stack probe (Check step) + detect distro/init.
    pub fn set_health(&mut self, health: AudioStackHealth) {
        self.health = Some(health);
    }

    /// Synchronously sample the cheap host descriptors when entering Check (file
    /// stats only — not the heavy shell-out probe, which runs on a worker).
    fn sample_host(&mut self) {
        self.distro_index = detect_distro().index();
        self.init_index = detect_init().index();
        self.sandbox = detect_sandbox();
    }

    /// Apply the enumerated DAC candidates (Select-DACs step).
    pub fn set_candidates(&mut self, data: Vec<DacCandidateData>) {
        self.candidates = data.into_iter().map(Candidate::from_data).collect();
        self.detecting = false;
        self.detected = true;
        self.dac_focus = 0;
    }

    /// Apply the generated per-DAC configs (Review step).
    pub fn set_configs(&mut self, data: Vec<DacConfigData>) {
        self.configs = data
            .into_iter()
            .map(|d| ConfigBlock { data: d, flash: None })
            .collect();
        self.review_focus = 0;
        self.review_scroll = 0;
    }

    /// Apply one test read-back (Test step).
    pub fn set_test_result(
        &mut self,
        requested: Option<(u32, u32)>,
        negotiated: Option<NegotiatedRate>,
        note: Option<String>,
    ) {
        self.tested = true;
        self.test_requested = requested;
        self.test_negotiated = negotiated;
        self.test_note = note;
    }

    /// The (node_name, display_name) pairs to generate configs for: every checked
    /// candidate, or the manual node when nothing enumerated (1:1 with the Slint
    /// `checked_dacs`).
    fn checked_dacs(&self) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = self
            .candidates
            .iter()
            .filter(|c| c.checked)
            .map(|c| (c.id.clone(), c.description.clone()))
            .collect();
        if out.is_empty() {
            if let Some(m) = &self.manual_node {
                out.push((m.clone(), m.clone()));
            }
        }
        out
    }

    /// Whether the Select-DACs step can advance (≥1 checked or a valid manual).
    fn has_selection(&self) -> bool {
        self.candidates.iter().any(|c| c.checked) || self.manual_node.is_some()
    }

    // -------------------------- input --------------------------

    pub fn handle_key(&mut self, key: KeyEvent) -> ScreenAction {
        // Open editors own the keyboard.
        if self.check_editor.is_some() {
            return self.handle_check_editor(key);
        }
        if self.manual.is_some() {
            return self.handle_manual_input(key);
        }

        // Horizontal step navigation is uniform across steps (the shell routes
        // ←/→ to the wizard). Right advances (per-step gated); Left goes back.
        match key.code {
            KeyCode::Right => return self.advance(),
            KeyCode::Left => return self.retreat(),
            KeyCode::Esc => return self.on_escape(),
            _ => {}
        }

        match self.step {
            WStep::Welcome => self.keys_welcome(key),
            WStep::Check => self.keys_check(key),
            WStep::SelectDacs => self.keys_select(key),
            WStep::Review => self.keys_review(key),
            WStep::Test => self.keys_test(key),
            WStep::Done => self.keys_done(key),
        }
    }

    /// Esc: leave outright on the terminal steps (nothing staged), else ask to
    /// abandon (the middle steps hold transient selections).
    fn on_escape(&mut self) -> ScreenAction {
        match self.step {
            WStep::Welcome | WStep::Done => ScreenAction::Back,
            _ => ScreenAction::WizardAbandon,
        }
    }

    /// Advance to the next step, kicking the worker the new step needs. Gated on
    /// Select-DACs (needs a selection).
    fn advance(&mut self) -> ScreenAction {
        match self.step {
            WStep::Welcome => {
                self.step = WStep::Check;
                self.sample_host();
                ScreenAction::WizardProbeHealth
            }
            WStep::Check => {
                self.step = WStep::SelectDacs;
                if self.detected {
                    ScreenAction::Consumed // already enumerated once — keep it
                } else {
                    self.detecting = true;
                    ScreenAction::WizardDetect
                }
            }
            WStep::SelectDacs => {
                if !self.has_selection() {
                    self.gate_note = Some((s::WIZ_SELECT_GATE.to_string(), Instant::now()));
                    return ScreenAction::Consumed;
                }
                self.step = WStep::Review;
                ScreenAction::WizardGenConfigs(self.checked_dacs())
            }
            // Review → Test and Test → Done are plain linear advances (no worker),
            // so they follow the pure step-transition table directly.
            WStep::Review | WStep::Test => {
                if let Some(next) = next_step(self.step) {
                    self.step = next;
                }
                ScreenAction::Consumed
            }
            WStep::Done => ScreenAction::Back,
        }
    }

    /// Back to the previous step (no re-fetch — the state is kept).
    fn retreat(&mut self) -> ScreenAction {
        if let Some(prev) = prev_step(self.step) {
            self.step = prev;
        }
        ScreenAction::Consumed
    }

    fn keys_welcome(&mut self, key: KeyEvent) -> ScreenAction {
        if matches!(key.code, KeyCode::Enter | KeyCode::Char(' ')) {
            self.advance()
        } else {
            ScreenAction::Consumed
        }
    }

    fn keys_check(&mut self, key: KeyEvent) -> ScreenAction {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.check_focus = self.check_focus.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.check_focus = (self.check_focus + 1).min(1);
            }
            KeyCode::Enter | KeyCode::Char(' ') => self.open_check_editor(),
            _ => {}
        }
        ScreenAction::Consumed
    }

    fn open_check_editor(&mut self) {
        if self.check_focus == 0 {
            let opts: Vec<String> = Distro::ALL.iter().map(|d| d.label().to_string()).collect();
            self.check_editor = Some((
                CheckField::Distro,
                SelectPopup::new(s::WIZ_DISTRO, opts, self.distro_index, false),
            ));
        } else {
            let opts: Vec<String> = InitSystem::ALL.iter().map(|i| i.label().to_string()).collect();
            self.check_editor = Some((
                CheckField::Init,
                SelectPopup::new(s::WIZ_INIT, opts, self.init_index, false),
            ));
        }
    }

    fn handle_check_editor(&mut self, key: KeyEvent) -> ScreenAction {
        let (field, mut popup) = self.check_editor.take().unwrap();
        match popup.handle_key(key) {
            SelectOutcome::Chosen(i) => match field {
                CheckField::Distro => self.distro_index = i,
                CheckField::Init => self.init_index = i,
            },
            SelectOutcome::Cancelled => {}
            SelectOutcome::Pending => self.check_editor = Some((field, popup)),
        }
        ScreenAction::Consumed
    }

    fn keys_select(&mut self, key: KeyEvent) -> ScreenAction {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                if !self.candidates.is_empty() {
                    self.dac_focus = self.dac_focus.saturating_sub(1);
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if !self.candidates.is_empty() {
                    self.dac_focus = (self.dac_focus + 1).min(self.candidates.len() - 1);
                }
            }
            KeyCode::Enter | KeyCode::Char(' ') => {
                if let Some(c) = self.candidates.get_mut(self.dac_focus) {
                    c.checked = !c.checked;
                }
            }
            KeyCode::Char('m') => {
                self.manual = Some(TextInput::new(
                    self.manual_node.as_deref().unwrap_or(""),
                    false,
                ));
            }
            _ => {}
        }
        ScreenAction::Consumed
    }

    fn handle_manual_input(&mut self, key: KeyEvent) -> ScreenAction {
        let mut input = self.manual.take().unwrap();
        match input.handle_key(key) {
            InputOutcome::Accepted => {
                let text = input.buf.trim().to_string();
                if wizard_core::validate_node_name(&text) {
                    self.manual_node = Some(text);
                } else if !text.is_empty() {
                    self.gate_note = Some((s::WIZ_MANUAL_INVALID.to_string(), Instant::now()));
                    self.manual = Some(input); // keep it open to fix
                }
            }
            InputOutcome::Cancelled => {}
            InputOutcome::Pending => self.manual = Some(input),
        }
        ScreenAction::Consumed
    }

    fn keys_review(&mut self, key: KeyEvent) -> ScreenAction {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.review_focus = self.review_focus.saturating_sub(1);
                self.follow_focus();
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if !self.configs.is_empty() {
                    self.review_focus = (self.review_focus + 1).min(self.configs.len() - 1);
                }
                self.follow_focus();
            }
            KeyCode::PageUp => self.review_scroll = self.review_scroll.saturating_sub(8),
            KeyCode::PageDown => {
                self.review_scroll = self.review_scroll.saturating_add(8).min(self.max_review_scroll());
            }
            KeyCode::Char('c') => self.copy_focused_block(),
            KeyCode::Char('C') => self.copy_all_blocks(),
            KeyCode::Char('w') => self.write_focused_block(),
            _ => {}
        }
        ScreenAction::Consumed
    }

    /// Scroll so the focused block's header is at the top of the viewport.
    fn follow_focus(&mut self) {
        let mut line: u16 = 0;
        for (i, cfg) in self.configs.iter().enumerate() {
            if i == self.review_focus {
                break;
            }
            line = line.saturating_add(block_line_count(&cfg.data));
        }
        self.review_scroll = line.min(self.max_review_scroll());
    }

    /// Total rendered lines in the Review body (the backup-hint line + every
    /// block) — the ceiling `review_scroll` is clamped against so PgDn/↓ can
    /// never scroll past the last block into blank space.
    fn review_content_lines(&self) -> u16 {
        let mut total: u16 = 1; // WIZ_BACKUP_HINT
        for cfg in &self.configs {
            total = total.saturating_add(block_line_count(&cfg.data));
        }
        total
    }

    /// The highest `review_scroll` that still shows the last content line.
    fn max_review_scroll(&self) -> u16 {
        self.review_content_lines().saturating_sub(1)
    }

    fn copy_focused_block(&mut self) {
        let env = self.clip_env;
        if let Some(block) = self.configs.get_mut(self.review_focus) {
            let text = block.data.full_block();
            let report = clipboard::copy(&text, &block.data.short(), &env);
            block.flash = Some((report.tier, Instant::now()));
            self.status_flash = Some((report.detail, Instant::now()));
        }
    }

    /// `C` — copy every block at once. Always ALSO lands a durable file
    /// artifact at a fixed path, independent of which clipboard tier won:
    /// OSC 52 is one-way/unverifiable, so the batch copy must never leave the
    /// operator with nothing to fall back to if the paste silently failed.
    fn copy_all_blocks(&mut self) {
        if self.configs.is_empty() {
            return;
        }
        // Prepend the backup command — "copy all" gives the operator a back-up +
        // every DAC's config in one paste.
        let mut parts = vec![format!("# ── back up first ──\n{}", wizard_core::BACKUP_CMD)];
        parts.extend(
            self.configs
                .iter()
                .map(|c| format!("# ── {} ──\n{}", c.data.name, c.data.full_block())),
        );
        let all = parts.join("\n\n");

        let report = clipboard::copy(&all, "all-blocks", &self.clip_env);
        let save = clipboard::write_wizard_file("all-blocks", &all);
        let outcome = match (report.tier, save) {
            // The clipboard chain already fell back to this exact file —
            // don't say "saved" twice.
            (clipboard::Tier::File, Ok(_)) => report.detail,
            (_, Ok(path)) => format!("{} + saved {}", report.detail, path.display()),
            (_, Err(e)) => format!("{} (file save also failed: {e})", report.detail),
        };
        self.status_flash = Some((
            format!("{} ({outcome})", s::wiz_copied_all(self.configs.len())),
            Instant::now(),
        ));
    }

    fn write_focused_block(&mut self) {
        if let Some(block) = self.configs.get_mut(self.review_focus) {
            let text = block.data.full_block();
            match clipboard::write_wizard_file(&block.data.short(), &text) {
                Ok(path) => {
                    block.flash = Some((clipboard::Tier::File, Instant::now()));
                    self.status_flash =
                        Some((format!("{} {}", s::WIZ_SAVED_TO, path.display()), Instant::now()));
                }
                Err(e) => {
                    self.status_flash = Some((format!("{}: {e}", s::WIZ_SAVE_FAILED), Instant::now()));
                }
            }
        }
    }

    fn keys_test(&mut self, key: KeyEvent) -> ScreenAction {
        match key.code {
            KeyCode::Enter | KeyCode::Char('t') => ScreenAction::WizardTestStart,
            KeyCode::Char('r') => {
                if self.tested {
                    ScreenAction::WizardTestPoll
                } else {
                    ScreenAction::WizardTestStart
                }
            }
            _ => ScreenAction::Consumed,
        }
    }

    fn keys_done(&mut self, key: KeyEvent) -> ScreenAction {
        if matches!(key.code, KeyCode::Enter) {
            ScreenAction::Back
        } else {
            ScreenAction::Consumed
        }
    }

    // -------------------------- render --------------------------

    pub fn draw(&self, f: &mut Frame, area: Rect, _ctx: &DrawCtx) {
        match self.step {
            WStep::Welcome => self.draw_welcome(f, area),
            WStep::Check => self.draw_check(f, area),
            WStep::SelectDacs => self.draw_select(f, area),
            WStep::Review => self.draw_review(f, area),
            WStep::Test => self.draw_test(f, area),
            WStep::Done => self.draw_done(f, area),
        }

        // Overlays (Check override select, manual node input) on top.
        if let Some((_, popup)) = &self.check_editor {
            popup.draw(f, area);
        }
        if let Some(input) = &self.manual {
            let body = format!("{}\n\n> {}", s::WIZ_MANUAL_BODY, input.display());
            widgets::modal(f, area, s::WIZ_MANUAL_TITLE, &body, s::HELP_INPUT);
        }
    }

    fn draw_welcome(&self, f: &mut Frame, area: Rect) {
        let mut lines = vec![
            Line::from(Span::styled(s::WIZ_WELCOME_TITLE, theme::accent_bold())),
            widgets::blank(),
        ];
        // Word-wrap the body, preserving intentional paragraph breaks (FB5).
        for l in s::WIZ_WELCOME_BODY.split('\n') {
            if l.trim().is_empty() {
                lines.push(widgets::blank());
            } else {
                for wl in widgets::wrap(l, area.width.max(1) as usize) {
                    lines.push(Line::from(wl));
                }
            }
        }
        lines.push(widgets::blank());
        lines.push(Line::from(widgets::help_spans(s::WIZ_WELCOME_CTA)));
        f.render_widget(Paragraph::new(lines), area);
    }

    fn draw_check(&self, f: &mut Frame, area: Rect) {
        let distro = Distro::ALL.get(self.distro_index).copied().unwrap_or(Distro::Other);
        let init = InitSystem::ALL.get(self.init_index).copied().unwrap_or(InitSystem::Unknown);
        let width = area.width;
        let mut lines: Vec<Line> = Vec::new();

        // Health verdict (blind inside a sandbox — show reference commands only).
        if self.sandbox != Sandbox::None {
            lines.extend(widgets::wrapped_note(&s::wiz_sandbox_note(sandbox_name(self.sandbox)), width, theme::warn()));
        } else if let Some(h) = &self.health {
            if h.is_ready() {
                lines.push(Line::from(Span::styled(s::WIZ_HEALTH_READY, theme::ok())));
            } else {
                lines.push(Line::from(Span::styled(s::WIZ_HEALTH_ATTENTION, theme::warn())));
            }
        } else {
            lines.push(Line::from(Span::styled(s::WIZ_HEALTH_CHECKING, theme::dim())));
        }
        lines.push(widgets::blank());

        // The two overrides (focusable rows), sharing one control column.
        let ctrl_col = widgets::control_column(&[s::WIZ_DISTRO, s::WIZ_INIT], width);
        lines.extend(self.check_block(0, s::WIZ_DISTRO, distro.label(), ctrl_col, width));
        lines.extend(self.check_block(1, s::WIZ_INIT, init.label(), ctrl_col, width));
        lines.push(widgets::blank());

        // Remediation / reference commands.
        let rows = if self.sandbox != Sandbox::None {
            wizard_core::reference_commands(distro, init)
        } else if let Some(h) = &self.health {
            wizard_core::remediations(*h, distro, init)
        } else {
            Vec::new()
        };
        if rows.is_empty() && self.sandbox == Sandbox::None && self.health.is_some() {
            lines.extend(widgets::wrapped_note(s::WIZ_NO_REMEDIATION, width, theme::dim()));
        }
        for (caption, command) in &rows {
            // Captions are prose → wrap; commands are copy-paste → never wrap.
            for cl in widgets::wrap(caption, width.saturating_sub(4).max(1) as usize) {
                lines.push(Line::from(Span::styled(format!("  • {cl}"), Style::default())));
            }
            for cmd_line in command.lines() {
                lines.push(Line::from(Span::styled(format!("      {cmd_line}"), theme::dim())));
            }
        }
        f.render_widget(Paragraph::new(lines), area);
    }

    fn check_block(&self, idx: usize, label: &'static str, value: &str, ctrl_col: u16, width: u16) -> Vec<Line<'static>> {
        let focused = self.check_focus == idx && self.check_editor.is_none();
        widgets::field_block(
            &widgets::Field {
                label,
                value: value.to_string(),
                widget: "[select]",
                focused,
                enabled: true,
                reason: None,
                description: None,
            },
            ctrl_col,
            width,
        )
    }

    fn draw_select(&self, f: &mut Frame, area: Rect) {
        let mut lines: Vec<Line> = Vec::new();
        if self.detecting {
            lines.push(Line::from(Span::styled(s::WIZ_DETECTING, theme::dim())));
        } else if self.candidates.is_empty() {
            lines.extend(widgets::wrapped_note(s::WIZ_NO_DACS, area.width, theme::warn()));
        } else {
            lines.extend(widgets::wrapped_note(s::WIZ_SELECT_INTRO, area.width, theme::dim()));
            lines.push(widgets::blank());
            for (i, c) in self.candidates.iter().enumerate() {
                let mark = if c.checked { "[x]" } else { "[ ]" };
                let badge = if c.looks_like_dac { s::WIZ_DAC_BADGE } else { "" };
                let deflt = if c.is_default { s::WIZ_DEFAULT_BADGE } else { "" };
                let bus = if c.bus.is_empty() { String::new() } else { format!(" ({})", c.bus) };
                let text = format!("{mark} {}{bus}{badge}{deflt}", c.description);
                let focused = i == self.dac_focus;
                let style = if focused { theme::selection() } else { Style::default() };
                lines.push(Line::from(Span::styled(format!("  {text}"), style)));
                if !c.rates_label.is_empty() {
                    lines.push(widgets::note_line(&format!("supports {}", c.rates_label)));
                }
            }
        }
        // Manual escape hatch + accepted node.
        lines.push(widgets::blank());
        if let Some(m) = &self.manual_node {
            lines.push(widgets::note_line(&format!(
                "{} {m} ({})",
                s::WIZ_MANUAL_ACCEPTED,
                wizard_core::detect_dac_type(m)
            )));
        }
        lines.push(widgets::note_line(s::WIZ_MANUAL_HINT));
        if let Some((note, at)) = &self.gate_note {
            if at.elapsed() < STATUS_FLASH {
                lines.push(widgets::warn_line(note));
            }
        }
        f.render_widget(Paragraph::new(lines), area);
    }

    fn draw_review(&self, f: &mut Frame, area: Rect) {
        if self.configs.is_empty() {
            let l = vec![Line::from(Span::styled(s::WIZ_GENERATING, theme::dim()))];
            f.render_widget(Paragraph::new(l), area);
            return;
        }

        // Reserve the bottom row for the status flash + a fixed footer line.
        let body_h = area.height.saturating_sub(1);
        let body = Rect { height: body_h, ..area };

        let mut lines: Vec<Line> = Vec::new();
        // A backup reminder above the blocks (dim).
        lines.push(widgets::note_line(s::WIZ_BACKUP_HINT));
        for (i, block) in self.configs.iter().enumerate() {
            let focused = i == self.review_focus;
            append_block_lines(&mut lines, block, focused);
        }
        f.render_widget(Paragraph::new(lines).scroll((self.review_scroll, 0)), body);

        // Footer: transient status flash (copy/save result) else the safety note.
        let footer = Rect { y: area.y + area.height.saturating_sub(1), height: 1, ..area };
        let footer_line = match &self.status_flash {
            Some((msg, at)) if at.elapsed() < STATUS_FLASH => {
                Line::from(Span::styled(format!(" {msg}"), theme::ok()))
            }
            _ => Line::from(Span::styled(format!(" {}", s::WIZ_REVIEW_FOOTER), theme::dim())),
        };
        f.render_widget(Paragraph::new(footer_line), footer);
    }

    fn draw_test(&self, f: &mut Frame, area: Rect) {
        let mut lines: Vec<Line> = Vec::new();
        lines.extend(widgets::wrapped_note(s::WIZ_TEST_INTRO, area.width, theme::dim()));
        lines.push(widgets::blank());

        if let Some(note) = &self.test_note {
            lines.push(widgets::warn_line(note));
        }
        if self.tested {
            // Requested (what QBZ asked the daemon for) vs negotiated (the DAC's
            // real hardware clock) — the bit-perfect proof.
            let req = match self.test_requested {
                Some((rate, bits)) if rate > 0 => {
                    format!("QBZ requesting {} · {}-bit", wizard_core::khz(rate), bits)
                }
                _ => s::WIZ_TEST_NOTHING.to_string(),
            };
            lines.push(Line::from(Span::styled(format!("  {req}"), Style::default())));
            match &self.test_negotiated {
                Some(n) => {
                    let matched = self
                        .test_requested
                        .map(|(r, _)| r > 0 && n.sample_rate == r)
                        .unwrap_or(false);
                    let style = if matched { theme::ok() } else { theme::warn() };
                    lines.push(Line::from(Span::styled(
                        format!("  {}", wizard_core::negotiated_label(n)),
                        style,
                    )));
                    if matched {
                        lines.push(widgets::note_line(s::WIZ_TEST_MATCHED));
                    }
                    // Label a known reference track if the rate/depth lines up.
                    if let Some((rate, bits)) = self.test_requested {
                        if let Some(seed) = wizard_core::seed_for_rate_depth(rate, bits) {
                            lines.push(widgets::note_line(&format!(
                                "{} {} — {}",
                                s::WIZ_TEST_REFERENCE,
                                seed.artist,
                                seed.title
                            )));
                        }
                    }
                }
                None => lines.push(widgets::note_line(s::WIZ_TEST_WAITING)),
            }
        }

        lines.push(widgets::blank());
        // Reference seed tracks the operator can cast to verify each rate.
        lines.push(widgets::note_line(s::WIZ_TEST_SEEDS_HEADER));
        for seed in wizard_core::TEST_SEEDS.iter() {
            lines.push(widgets::note_line(&format!(
                "{}-bit/{} — {} — {} (Qobuz id {})",
                seed.depth,
                wizard_core::khz(seed.rate as u32),
                seed.artist,
                seed.title,
                seed.id_hint
            )));
        }
        f.render_widget(Paragraph::new(lines), area);
    }

    fn draw_done(&self, f: &mut Frame, area: Rect) {
        let mut lines = vec![
            Line::from(Span::styled(s::WIZ_DONE_TITLE, theme::accent_bold())),
            widgets::blank(),
        ];
        let selected = self.configs.len();
        for wl in widgets::wrap(&s::wiz_done_summary(selected), area.width.max(1) as usize) {
            lines.push(Line::from(wl));
        }
        lines.push(widgets::blank());
        lines.extend(widgets::wrapped_note(s::WIZ_DONE_REMINDER, area.width, theme::warn()));
        // The init-aware "(re)start the audio services" command for this box.
        let init = InitSystem::ALL.get(self.init_index).copied().unwrap_or(InitSystem::Unknown);
        lines.push(widgets::blank());
        lines.push(widgets::note_line(s::WIZ_DONE_RESTART));
        for cmd_line in wizard_core::restart_cmd(init).lines() {
            lines.push(widgets::note_line(cmd_line));
        }
        lines.push(widgets::blank());
        lines.push(Line::from(widgets::help_spans(s::WIZ_DONE_CTA)));
        f.render_widget(Paragraph::new(lines), area);
    }
}

impl Default for WizardState {
    fn default() -> Self {
        Self::new()
    }
}

// ============================ render helpers ============================

fn sandbox_name(sb: Sandbox) -> &'static str {
    match sb {
        Sandbox::Flatpak => "Flatpak",
        Sandbox::Snap => "Snap",
        Sandbox::None => "",
    }
}

/// Number of rendered lines one Review block occupies (header + paths + config
/// body + separator) — used to bring the focused block to the viewport top.
fn block_line_count(data: &DacConfigData) -> u16 {
    // header(1) + paths(3) + config body + blank(1).
    let body = data.full_block().lines().count() as u16;
    1 + 3 + body + 1
}

/// One Review block, left-ruled with an accent (focused) / dim rail so it reads
/// as a bordered box while long config lines can run past the frame edge (the
/// FULL verbatim text is what `c`/`w` copy, not the clipped preview).
fn append_block_lines(lines: &mut Vec<Line<'static>>, block: &ConfigBlock, focused: bool) {
    let rail_style = if focused { theme::accent() } else { theme::dim() };
    let flashing = block
        .flash
        .as_ref()
        .map(|(_, at)| at.elapsed() < FLASH)
        .unwrap_or(false);

    // Header: rail + DAC name (+ the copied ✓ flash).
    let mut header = vec![
        Span::styled("│ ".to_string(), rail_style),
        Span::styled(
            block.data.name.clone(),
            if focused { theme::accent_bold() } else { Style::default() },
        ),
    ];
    if flashing {
        if let Some((tier, _)) = &block.flash {
            header.push(Span::styled(format!("   {}", tier.short_label()), theme::ok()));
        }
    }
    lines.push(Line::from(header));

    // Target files (dim).
    for path in block.data.target_paths() {
        lines.push(Line::from(vec![
            Span::styled("│ ".to_string(), rail_style),
            Span::styled(format!("→ {path}"), theme::dim()),
        ]));
    }
    // Config body.
    for l in block.data.full_block().lines() {
        lines.push(Line::from(vec![
            Span::styled("│ ".to_string(), rail_style),
            Span::styled(l.to_string(), Style::default()),
        ]));
    }
    lines.push(widgets::blank());
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::KeyModifiers;

    #[test]
    fn step_transition_table_is_linear_and_bounded() {
        assert_eq!(next_step(WStep::Welcome), Some(WStep::Check));
        assert_eq!(next_step(WStep::Check), Some(WStep::SelectDacs));
        assert_eq!(next_step(WStep::SelectDacs), Some(WStep::Review));
        assert_eq!(next_step(WStep::Review), Some(WStep::Test));
        assert_eq!(next_step(WStep::Test), Some(WStep::Done));
        assert_eq!(next_step(WStep::Done), None); // terminal
        assert_eq!(prev_step(WStep::Welcome), None); // initial
        assert_eq!(prev_step(WStep::Done), Some(WStep::Test));
        // Round trip through every step.
        for s in STEP_ORDER {
            if let Some(n) = next_step(s) {
                assert_eq!(prev_step(n), Some(s));
            }
        }
    }

    #[test]
    fn select_gate_blocks_advance_without_a_selection() {
        let mut w = WizardState::new();
        w.step = WStep::SelectDacs;
        w.detected = true;
        // No candidate checked, no manual → advance is refused, step unchanged.
        let action = w.advance();
        assert!(matches!(action, ScreenAction::Consumed));
        assert_eq!(w.step, WStep::SelectDacs);
        assert!(w.gate_note.is_some());

        // A manual node satisfies the gate.
        w.manual_node = Some("alsa_output.usb-x.analog-stereo".to_string());
        let action = w.advance();
        assert!(matches!(action, ScreenAction::WizardGenConfigs(_)));
        assert_eq!(w.step, WStep::Review);
    }

    #[test]
    fn checked_dacs_prefers_candidates_then_manual() {
        let mut w = WizardState::new();
        w.set_candidates(vec![
            DacCandidateData {
                id: "node-a".into(),
                description: "DAC A".into(),
                bus: "usb".into(),
                is_default: false,
                looks_like_dac: true,
                rates_label: "44.1 / 192 kHz".into(),
            },
            DacCandidateData {
                id: "node-b".into(),
                description: "Monitor".into(),
                bus: "".into(),
                is_default: false,
                looks_like_dac: false,
                rates_label: "".into(),
            },
        ]);
        // Only the likely DAC is pre-checked.
        assert_eq!(w.checked_dacs(), vec![("node-a".to_string(), "DAC A".to_string())]);
        // With nothing checked, the manual node is the fallback.
        for c in &mut w.candidates {
            c.checked = false;
        }
        w.manual_node = Some("alsa_output.usb-y".to_string());
        assert_eq!(
            w.checked_dacs(),
            vec![("alsa_output.usb-y".to_string(), "alsa_output.usb-y".to_string())]
        );
    }

    #[test]
    fn esc_confirms_abandon_only_on_middle_steps() {
        let mut w = WizardState::new();
        assert!(matches!(w.on_escape(), ScreenAction::Back)); // Welcome
        w.step = WStep::Review;
        assert!(matches!(w.on_escape(), ScreenAction::WizardAbandon));
        w.step = WStep::Done;
        assert!(matches!(w.on_escape(), ScreenAction::Back));
    }

    #[test]
    fn only_welcome_declines_to_claim_horizontal() {
        // Welcome has nothing that consumes ← (the CTA only listens for
        // Enter/Space), so it must NOT claim ←/→ — otherwise ← is silently
        // swallowed by a no-op retreat() instead of dropping focus to the
        // sidebar like every other section.
        let mut w = WizardState::new();
        for step in STEP_ORDER {
            w.step = step;
            assert_eq!(
                w.claims_horizontal(),
                step != WStep::Welcome,
                "step {step:?} claims_horizontal mismatch"
            );
        }
    }

    #[test]
    fn review_scroll_page_down_never_overscrolls_past_the_last_block() {
        let mut w = populated(); // one config block
        w.step = WStep::Review;
        let max = w.max_review_scroll();
        assert!(max > 0, "the populated fixture should have real content to clamp against");

        let page_down = KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE);
        // Mash PageDown well past the content height.
        for _ in 0..20 {
            w.handle_key(page_down);
        }
        assert_eq!(w.review_scroll, max);
    }

    #[test]
    fn breadcrumb_reflects_the_current_step() {
        let mut w = WizardState::new();
        assert_eq!(w.editing_label(), Some(s::WIZ_STEP_WELCOME));
        w.step = WStep::Review;
        assert_eq!(w.editing_label(), Some(s::WIZ_STEP_REVIEW));
    }

    // ---- 80×24 render of every wizard step (no panic + step content) ----

    fn populated() -> WizardState {
        let mut w = WizardState::new();
        w.set_health(qbz_audio::AudioStackHealth {
            wireplumber_active: true,
            has_pw_dump: true,
            cpal_sees_pipewire: true,
            has_pactl: true,
            any_devices: true,
        });
        w.set_candidates(vec![DacCandidateData {
            id: "alsa_output.usb-Cambridge-00.analog-stereo".into(),
            description: "Cambridge DacMagic".into(),
            bus: "usb".into(),
            is_default: true,
            looks_like_dac: true,
            rates_label: "44.1 / 96 / 192 kHz".into(),
        }]);
        w.set_configs(vec![DacConfigData {
            name: "Cambridge DacMagic".into(),
            node_name: "alsa_output.usb-Cambridge-00.analog-stereo".into(),
            pipewire_conf: "context.properties = { default.clock.allowed-rates = [ 44100 192000 ] }".into(),
            pulse_conf: "stream.rules = [ ... ]".into(),
            wireplumber_conf: "monitor.alsa.rules = [ ... ]".into(),
        }]);
        w.set_test_result(Some((192000, 24)), Some(NegotiatedRate {
            sample_rate: 192000,
            format: "S32_LE".into(),
            channels: 2,
        }), None);
        w
    }

    fn render_step(w: &WizardState) -> String {
        render_step_sized(w, 80, 24)
    }

    fn render_step_sized(w: &WizardState, width: u16, height: u16) -> String {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        let ctx = DrawCtx { status: None };
        terminal
            .draw(|f| {
                let area = f.area();
                w.draw(f, area, &ctx);
            })
            .unwrap();
        let buf = terminal.backend().buffer().clone();
        let mut out = String::new();
        for y in 0..height {
            for x in 0..width {
                out.push_str(buf[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    #[test]
    fn every_wizard_step_renders_on_the_80x24_floor() {
        let mut w = populated();
        let expect: [(WStep, &str); 6] = [
            (WStep::Welcome, "HiFi"),
            (WStep::Check, "ready"),
            (WStep::SelectDacs, "Cambridge"),
            (WStep::Review, "Cambridge"),
            (WStep::Test, "DAC:"),
            (WStep::Done, "All set"),
        ];
        for (step, needle) in expect {
            w.step = step;
            let out = render_step(&w);
            assert!(out.contains(needle), "step {step:?} should render {needle:?}");
        }
        // The Review step exposes the copy affordance + the never-writes footer.
        w.step = WStep::Review;
        let review = render_step(&w);
        assert!(review.contains("→ ~/.config"), "review shows the target file paths");
        assert!(review.contains("NEVER"), "review footer states the wizard never writes files");
    }

    #[test]
    fn every_wizard_step_renders_at_120x30_wide() {
        // The wizard body draws into the content frame (no sidebar of its own);
        // this mirrors it at a comfortable wide size and asserts each step's
        // content survives the word-wrap rework without a panic (FB5).
        let mut w = populated();
        let expect: [(WStep, &str); 6] = [
            (WStep::Welcome, "HiFi"),
            (WStep::Check, "ready"),
            (WStep::SelectDacs, "Cambridge"),
            (WStep::Review, "Cambridge"),
            (WStep::Test, "DAC:"),
            (WStep::Done, "All set"),
        ];
        for (step, needle) in expect {
            w.step = step;
            let out = render_step_sized(&w, 120, 30);
            assert!(out.contains(needle), "wide step {step:?} should render {needle:?}");
        }
    }
}
