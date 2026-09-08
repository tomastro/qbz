// crates/qbzd/src/tui/screens/bundle.rs — Import / Export (03 §3.6).
//
// A pure RENDERER of the qbz-app::settings::bundle engine's plan — zero
// classification logic of its own (in-band classification was the D9 safety
// flaw). Import: path → the App plans on a worker → this screen shows the three
// buckets; an absent device opens the §3.2.2 device picker for a re-pick
// (replan is pure, done here); one confirm applies. The auth domain has its own
// dedicated, default-OFF gate. Export: destination + include-auth toggle
// (default off, warning while on); the source is ALWAYS this box's daemon
// profile.

use qbz_app::settings::bundle::{
    self, Bundle, DeviceChoice, ImportOptions, ImportPlan, LiveSystem, PlanLine, ProfilePaths,
};
use qbz_audio::{AudioBackendType, AudioDevice};
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::Frame;

use super::audio::{group_devices, DeviceEntry};
use crate::tui::app::{DrawCtx, ScreenAction};
use crate::tui::strings as s;
use crate::tui::theme;
use crate::tui::widgets::{self, InputOutcome, SelectOutcome, SelectPopup, TextInput};

/// Everything a planned import carries between the plan worker, the re-pick, and
/// the apply worker. The `live` snapshot is captured once so a re-pick replans
/// without touching hardware again.
pub struct PendingImport {
    pub bundle: Bundle,
    pub plan: ImportPlan,
    pub live: LiveSystem,
    pub opts: ImportOptions,
    pub target: ProfilePaths,
    pub backend: AudioBackendType,
    pub devices: Vec<AudioDevice>,
    pub device_choice: Option<DeviceChoice>,
    pub has_auth: bool,
    pub apply_with_auth: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BField {
    ImportPath,
    Review,
    ExportDest,
    IncludeAuth,
    Export,
}

const FIELDS: [BField; 5] = [
    BField::ImportPath,
    BField::Review,
    BField::ExportDest,
    BField::IncludeAuth,
    BField::Export,
];

enum Editor {
    ImportPath(TextInput),
    ExportDest(TextInput),
}

pub struct BundleState {
    focus: usize,
    editor: Option<Editor>,
    import_path: String,
    export_dest: String,
    include_auth: bool,
    has_desktop: bool,
    // review mode:
    pending: Option<PendingImport>,
    device_picker: Option<SelectPopup>,
    picker_entries: Vec<DeviceEntry>,
    auth_confirm: bool,
    scroll: u16,
}

impl BundleState {
    pub fn new(has_desktop: bool) -> Self {
        Self {
            focus: 0,
            editor: None,
            import_path: String::new(),
            export_dest: format!("~/{}", bundle::default_filename()),
            include_auth: false,
            has_desktop,
            pending: None,
            device_picker: None,
            picker_entries: Vec::new(),
            auth_confirm: false,
            scroll: 0,
        }
    }

    // Bundle is all immediate actions — never dirty (the App short-circuits it).
    pub fn is_editing(&self) -> bool {
        self.editor.is_some()
            || self.pending.is_some()
            || self.device_picker.is_some()
            || self.auth_confirm
    }

    /// The breadcrumb's level-2 node when an inline path/dest editor is active.
    /// The review panel, device picker and auth confirm are third-level overlays
    /// — the breadcrumb underneath stays `Setup › Import / Export`.
    pub fn editing_label(&self) -> Option<&'static str> {
        match &self.editor {
            Some(Editor::ImportPath(_)) => Some(s::B_IMPORT_PATH),
            Some(Editor::ExportDest(_)) => Some(s::B_EXPORT_DEST),
            None => None,
        }
    }

    /// Store a fresh plan from the App's worker (§3.6 step 3).
    pub fn set_plan(&mut self, planned: PendingImport) {
        self.scroll = 0;
        self.pending = Some(planned);
    }

    /// The data the App's apply worker needs; None when nothing is pending.
    pub fn apply_context(
        &self,
    ) -> Option<(Bundle, ProfilePaths, LiveSystem, ImportOptions, Option<DeviceChoice>, bool)> {
        self.pending.as_ref().map(|p| {
            (
                p.bundle.clone(),
                p.target.clone(),
                p.live.clone(),
                p.opts.clone(),
                p.device_choice.clone(),
                p.apply_with_auth,
            )
        })
    }

    pub fn clear_pending(&mut self) {
        self.pending = None;
        self.device_picker = None;
        self.auth_confirm = false;
        self.import_path.clear();
    }

    // -------------------------- input --------------------------

    pub fn handle_key(&mut self, key: KeyEvent) -> ScreenAction {
        // Overlays first.
        if self.device_picker.is_some() {
            return self.handle_picker_key(key);
        }
        if self.auth_confirm {
            return self.handle_auth_confirm(key);
        }
        if self.pending.is_some() {
            return self.handle_review_key(key);
        }
        if let Some(editor) = self.editor.take() {
            return self.handle_editor_key(editor, key);
        }

        match key.code {
            KeyCode::Up | KeyCode::Char('k') | KeyCode::BackTab => {
                self.focus = if self.focus == 0 { FIELDS.len() - 1 } else { self.focus - 1 };
                ScreenAction::Consumed
            }
            KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab => {
                self.focus = (self.focus + 1) % FIELDS.len();
                ScreenAction::Consumed
            }
            KeyCode::Enter | KeyCode::Char(' ') => self.activate(),
            KeyCode::Esc => ScreenAction::Back,
            _ => ScreenAction::Consumed,
        }
    }

    fn activate(&mut self) -> ScreenAction {
        match FIELDS[self.focus] {
            BField::ImportPath => {
                self.editor = Some(Editor::ImportPath(TextInput::new(&self.import_path, false)));
                ScreenAction::Consumed
            }
            BField::Review => {
                if self.import_path.trim().is_empty() {
                    ScreenAction::Consumed
                } else {
                    ScreenAction::ImportPlan(self.import_path.trim().to_string())
                }
            }
            BField::ExportDest => {
                self.editor = Some(Editor::ExportDest(TextInput::new(&self.export_dest, false)));
                ScreenAction::Consumed
            }
            BField::IncludeAuth => {
                self.include_auth ^= true;
                ScreenAction::Consumed
            }
            BField::Export => {
                ScreenAction::Export {
                    dest: self.export_dest.clone(),
                    include_auth: self.include_auth,
                }
            }
        }
    }

    fn handle_editor_key(&mut self, editor: Editor, key: KeyEvent) -> ScreenAction {
        match editor {
            Editor::ImportPath(mut input) => match input.handle_key(key) {
                InputOutcome::Accepted => {
                    self.import_path = input.buf.trim().to_string();
                    ScreenAction::Consumed
                }
                InputOutcome::Cancelled => ScreenAction::Consumed,
                InputOutcome::Pending => {
                    self.editor = Some(Editor::ImportPath(input));
                    ScreenAction::Consumed
                }
            },
            Editor::ExportDest(mut input) => match input.handle_key(key) {
                InputOutcome::Accepted => {
                    self.export_dest = input.buf.trim().to_string();
                    ScreenAction::Consumed
                }
                InputOutcome::Cancelled => ScreenAction::Consumed,
                InputOutcome::Pending => {
                    self.editor = Some(Editor::ExportDest(input));
                    ScreenAction::Consumed
                }
            },
        }
    }

    fn handle_review_key(&mut self, key: KeyEvent) -> ScreenAction {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.scroll = self.scroll.saturating_sub(1);
                ScreenAction::Consumed
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.scroll = self.scroll.saturating_add(1);
                ScreenAction::Consumed
            }
            KeyCode::Char('p') => {
                self.open_device_picker();
                ScreenAction::Consumed
            }
            KeyCode::Enter => {
                let has_auth = self.pending.as_ref().map(|p| p.has_auth).unwrap_or(false);
                if has_auth {
                    self.auth_confirm = true;
                    ScreenAction::Consumed
                } else {
                    if let Some(p) = self.pending.as_mut() {
                        p.apply_with_auth = false;
                    }
                    ScreenAction::ImportApply
                }
            }
            KeyCode::Esc => {
                self.pending = None;
                ScreenAction::Consumed
            }
            _ => ScreenAction::Consumed,
        }
    }

    fn handle_auth_confirm(&mut self, key: KeyEvent) -> ScreenAction {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                if let Some(p) = self.pending.as_mut() {
                    p.apply_with_auth = true;
                }
                self.auth_confirm = false;
                ScreenAction::ImportApply
            }
            KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') => {
                if let Some(p) = self.pending.as_mut() {
                    p.apply_with_auth = false;
                }
                self.auth_confirm = false;
                ScreenAction::ImportApply
            }
            _ => ScreenAction::Consumed,
        }
    }

    fn open_device_picker(&mut self) {
        let Some(pending) = &self.pending else { return };
        // Only meaningful when the plan flagged a device re-pick.
        if pending.plan.device_pick.is_none() {
            return;
        }
        self.picker_entries = group_devices(pending.backend, pending.devices.clone());
        let options: Vec<String> = self
            .picker_entries
            .iter()
            .map(|d| if d.bp { format!("{} {}", d.label, s::BP_BADGE) } else { d.label.clone() })
            .collect();
        let headers: Vec<Option<String>> = self.picker_entries.iter().map(|d| d.header.clone()).collect();
        self.device_picker =
            Some(SelectPopup::new(s::DEVICE_PICKER_TITLE, options, 0, true).with_headers(headers));
    }

    fn handle_picker_key(&mut self, key: KeyEvent) -> ScreenAction {
        let mut picker = self.device_picker.take().unwrap();
        match picker.handle_key(key) {
            SelectOutcome::Chosen(i) => {
                if let Some(entry) = self.picker_entries.get(i).cloned() {
                    let choice = if entry.id.is_empty() {
                        DeviceChoice::SystemDefault
                    } else {
                        DeviceChoice::Device { id: entry.id, label: entry.label }
                    };
                    self.replan_with(choice);
                }
                ScreenAction::Consumed
            }
            SelectOutcome::Cancelled => ScreenAction::Consumed,
            SelectOutcome::Pending => {
                self.device_picker = Some(picker);
                ScreenAction::Consumed
            }
        }
    }

    /// Re-run the plan with the operator's device choice (pure — no I/O; the
    /// `live` snapshot was captured at plan time).
    fn replan_with(&mut self, choice: DeviceChoice) {
        let Some(pending) = self.pending.as_mut() else { return };
        match bundle::replan_with_device(
            &pending.bundle,
            &pending.target,
            &pending.opts,
            &pending.live,
            choice.clone(),
        ) {
            Ok(plan) => {
                pending.plan = plan;
                pending.device_choice = Some(choice);
                self.scroll = 0;
            }
            Err(_) => {}
        }
    }

    // -------------------------- render --------------------------

    pub fn draw(&self, f: &mut Frame, area: Rect, _ctx: &DrawCtx) {
        if self.pending.is_some() {
            self.draw_review(f, area);
            return;
        }

        let width = area.width.saturating_sub(2); // section inner width
        let cur = FIELDS[self.focus];
        let ed = self.editor.is_none();
        // ONE control column across BOTH boxes (consistent for the whole screen).
        let ctrl_col = widgets::control_column(
            &[s::B_IMPORT_PATH, s::B_EXPORT_DEST, s::B_EXPORT_INCLUDE_AUTH],
            width,
        );

        let mut secs: Vec<widgets::Section> = Vec::new();
        let mut anchor: Option<widgets::FocusAnchor> = None;

        // IMPORT box.
        let path_val = if self.import_path.is_empty() {
            s::B_IMPORT_PATH_HINT.to_string()
        } else {
            self.import_path.clone()
        };
        let mut import_lines: Vec<Line> = Vec::new();
        let mut import_within: Option<(u16, u16)> = None;
        self.push_field(&mut import_lines, &mut import_within, cur, ed, BField::ImportPath, s::B_IMPORT_PATH, path_val, "[input]", ctrl_col, width);
        self.push_action(&mut import_lines, &mut import_within, cur, ed, BField::Review, s::B_IMPORT_ACTION);
        let import_active = matches!(cur, BField::ImportPath | BField::Review);
        widgets::push_section(&mut secs, &mut anchor, s::BUNDLE_IMPORT_HEADER, import_active, import_lines, import_within);

        // EXPORT box.
        let mut export_lines: Vec<Line> = Vec::new();
        let mut export_within: Option<(u16, u16)> = None;
        self.push_field(&mut export_lines, &mut export_within, cur, ed, BField::ExportDest, s::B_EXPORT_DEST, self.export_dest.clone(), "[input]", ctrl_col, width);
        self.push_field(&mut export_lines, &mut export_within, cur, ed, BField::IncludeAuth, s::B_EXPORT_INCLUDE_AUTH, if self.include_auth { "on" } else { "off" }.to_string(), "[toggle]", ctrl_col, width);
        if self.include_auth {
            export_lines.extend(widgets::wrapped_note(s::B_EXPORT_AUTH_WARNING, width, theme::warn()));
        }
        self.push_action(&mut export_lines, &mut export_within, cur, ed, BField::Export, s::B_EXPORT_ACTION);
        if self.has_desktop {
            export_lines.push(widgets::blank());
            export_lines.extend(widgets::wrapped_note(s::B_DESKTOP_HINT, width, theme::dim()));
        }
        let export_active = matches!(cur, BField::ExportDest | BField::IncludeAuth | BField::Export);
        widgets::push_section(&mut secs, &mut anchor, s::BUNDLE_EXPORT_HEADER, export_active, export_lines, export_within);

        widgets::sections_scroll(f, area, &secs, anchor);

        match &self.editor {
            Some(Editor::ImportPath(input)) => {
                widgets::modal(f, area, s::B_IMPORT_PATH, &input.display(), s::HELP_INPUT)
            }
            Some(Editor::ExportDest(input)) => {
                widgets::modal(f, area, s::B_EXPORT_DEST, &input.display(), s::HELP_INPUT)
            }
            None => {}
        }
    }

    /// Append a field block to `lines`, recording the focus anchor within the box.
    #[allow(clippy::too_many_arguments)]
    fn push_field(
        &self,
        lines: &mut Vec<Line<'static>>,
        within: &mut Option<(u16, u16)>,
        cur: BField,
        ed: bool,
        field: BField,
        label: &'static str,
        value: String,
        widget: &'static str,
        ctrl_col: u16,
        width: u16,
    ) {
        let focused = cur == field && ed;
        let start = lines.len() as u16;
        let block = widgets::field_block(
            &widgets::Field { label, value, widget, focused, enabled: true, reason: None, description: None },
            ctrl_col,
            width,
        );
        if focused {
            *within = Some((start, block.len() as u16));
        }
        lines.extend(block);
    }

    /// Append a single-line action row, recording the focus anchor within the box.
    #[allow(clippy::too_many_arguments)]
    fn push_action(
        &self,
        lines: &mut Vec<Line<'static>>,
        within: &mut Option<(u16, u16)>,
        cur: BField,
        ed: bool,
        field: BField,
        label: &str,
    ) {
        let focused = cur == field && ed;
        if focused {
            *within = Some((lines.len() as u16, 1));
        }
        lines.push(widgets::action_line(&format!("> {label}"), focused, true));
    }

    fn draw_review(&self, f: &mut Frame, area: Rect) {
        let Some(p) = &self.pending else { return };
        let mut lines: Vec<Line> = Vec::new();

        bucket(&mut lines, s::B_BUCKET_APPLIED, &p.plan.applied, BucketKind::Applied);
        lines.push(widgets::blank());
        bucket(&mut lines, s::B_BUCKET_ADAPTED, &p.plan.adapted, BucketKind::Adapted);
        if p.plan.device_pick.is_some() {
            lines.push(widgets::note_line("press p to pick a local device for the missing one"));
        }
        lines.push(widgets::blank());
        bucket(&mut lines, s::B_BUCKET_SKIPPED, &p.plan.skipped, BucketKind::Skipped);

        widgets::panel(f, area, s::BUNDLE_TITLE, lines, self.scroll);

        if self.device_picker.is_some() {
            self.device_picker.as_ref().unwrap().draw(f, area);
        } else if self.auth_confirm {
            widgets::modal(
                f,
                area,
                s::B_IMPORT_AUTH_TITLE,
                s::B_IMPORT_AUTH_BODY,
                s::B_IMPORT_AUTH_HINT,
            );
        }
    }
}

enum BucketKind {
    Applied,
    Adapted,
    Skipped,
}

fn bucket(lines: &mut Vec<Line<'static>>, title: &str, rows: &[PlanLine], kind: BucketKind) {
    // Bucket headers carry a semantic tint (applies=ok, adapted=warn, skipped=dim);
    // the count and label stand on their own without it.
    let head_style = match kind {
        BucketKind::Applied => theme::ok().add_modifier(Modifier::BOLD),
        BucketKind::Adapted => theme::warn().add_modifier(Modifier::BOLD),
        BucketKind::Skipped => theme::dim().add_modifier(Modifier::BOLD),
    };
    lines.push(Line::from(Span::styled(
        format!("{title} ({})", rows.len()),
        head_style,
    )));
    for l in rows {
        let text = match kind {
            BucketKind::Applied => format!("  {} = {}", l.key, l.new),
            BucketKind::Adapted => format!(
                "  {} {} -> {} ({})",
                l.key,
                l.old.as_deref().unwrap_or(""),
                l.new,
                l.why
            ),
            BucketKind::Skipped => format!("  {}  {}", l.key, l.why),
        };
        lines.push(Line::from(text));
    }
}
