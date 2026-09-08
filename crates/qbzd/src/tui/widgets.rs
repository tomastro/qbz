// crates/qbzd/src/tui/widgets.rs — reusable ratatui primitives for the setup TUI.
//
// Two interactive sub-widgets (a filterable select popup and a line input) plus
// the pure render helpers every screen shares (field rows, group headers, the
// bottom help bar, centered modals, spinner). No screen logic lives here —
// screens own their staged state and drive these.

use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph, Widget, Wrap};
use ratatui::Frame;

use super::theme;
use super::strings;

/// Focused-row emphasis: accent reversed (§1.2 — reverse is terminal-theme
/// agnostic, so the selection reads even on monochrome/serial).
pub fn focus_style() -> Style {
    theme::selection()
}

/// Mask a secret for display (token/paste rows never render plaintext).
pub fn mask(s: &str) -> String {
    "•".repeat(s.chars().count())
}

// ============================ sidebar width (FB5) ============================

/// The left-nav sidebar width in columns (incl. border), a pure function of the
/// terminal width so the 80×24 floor keeps working (FB5). At ≥ 100 cols the
/// operator gets the roomy 28-col sidebar the owner asked for (labels spelled
/// out, room for a dim summary line); below that we fall back to the compact
/// 14-col rendering so the content frame never starves at the floor.
pub fn sidebar_width(term_width: u16) -> u16 {
    if term_width >= 100 {
        28
    } else {
        14
    }
}

/// True when `sidebar_width` is in its roomy tier (spelled-out labels + summary).
pub fn sidebar_is_wide(term_width: u16) -> bool {
    sidebar_width(term_width) >= 28
}

// ============================ word wrap (FB5) ============================

/// Word-boundary wrap `text` to `width` columns, no new dependency (FB5). Splits
/// on ASCII whitespace; a word longer than `width` is hard-split (the only place
/// a word is broken mid-word). Blank/whitespace-only input yields no lines. Each
/// embedded `\n` in the source is honored as a hard break first, then each
/// segment is word-wrapped, so pre-formatted copy keeps its intentional breaks.
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return text.lines().map(str::to_string).collect();
    }
    let mut out: Vec<String> = Vec::new();
    for segment in text.split('\n') {
        if segment.trim().is_empty() {
            continue;
        }
        let mut cur = String::new();
        for word in segment.split_whitespace() {
            if word.chars().count() > width {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
                let mut chunk = String::new();
                for ch in word.chars() {
                    if chunk.chars().count() == width {
                        out.push(std::mem::take(&mut chunk));
                    }
                    chunk.push(ch);
                }
                cur = chunk;
                continue;
            }
            let extra = usize::from(!cur.is_empty());
            if cur.chars().count() + extra + word.chars().count() > width {
                out.push(std::mem::take(&mut cur));
                cur.push_str(word);
            } else {
                if !cur.is_empty() {
                    cur.push(' ');
                }
                cur.push_str(word);
            }
        }
        if !cur.is_empty() {
            out.push(cur);
        }
    }
    out
}

// ============================ field blocks (FB5) ============================

/// One field to render as a block. Row 1 is `label` + the CONTROL (value +
/// right-aligned widget marker) anchored at a screen-consistent column; rows
/// 2..n are the wrapped, dim DESCRIPTION under the label — the disabled `reason`
/// takes precedence over the static `description`.
pub struct Field<'a> {
    pub label: &'a str,
    pub value: String,
    /// `[select]`/`[toggle]`/`[input]`/`[slider]`, or `""` for a plain value.
    pub widget: &'a str,
    pub focused: bool,
    pub enabled: bool,
    /// Why the field is inert (only meaningful when `!enabled`).
    pub reason: Option<&'a str>,
    /// Static one-line help; wrapped under the label when present.
    pub description: Option<&'a str>,
}

/// The column (0-based, from the section inner edge) where every field's control
/// starts — the owner's "misma área de columna". ONE mechanism, applied on every
/// screen: `2` (indent) + the longest label + `2` (gap), clamped so the control
/// still has room. Pure so each screen derives an identical column for its own
/// label set.
pub fn control_column(labels: &[&str], width: u16) -> u16 {
    let max_label = labels
        .iter()
        .map(|l| l.chars().count())
        .max()
        .unwrap_or(0) as u16;
    let ceiling = width.saturating_sub(12).max(14);
    (2 + max_label + 2).clamp(14, ceiling)
}

/// Render a field as a block of lines (FB5). `ctrl_col` is the shared control
/// column; `width` is the section inner width. The value is truncated with `…`
/// (values never wrap — the control stays one line); the widget marker is
/// right-aligned so the marker column reads cleanly too. When `focused` the whole
/// control row is one accent-reverse bar (serial-safe, §1.2).
pub fn field_block(field: &Field, ctrl_col: u16, width: u16) -> Vec<Line<'static>> {
    let width = width.max(ctrl_col + 4) as usize;
    let ctrl = ctrl_col as usize;
    let widget_len = field.widget.chars().count();
    // Reserve the widget + a one-column gap on the right; the rest is the value.
    let reserved = if widget_len == 0 { 0 } else { widget_len + 1 };
    let value_space = width.saturating_sub(ctrl + reserved);
    let value = truncate(&field.value, value_space);
    let value_len = value.chars().count();

    let label_piece = pad_to(&format!("  {}", field.label), ctrl);
    let mid_pad = width.saturating_sub(ctrl + value_len + widget_len);
    let value_piece = format!("{value}{}", " ".repeat(mid_pad));
    let widget_piece = field.widget.to_string();

    let row1 = if field.focused {
        let mut sel = focus_style();
        if !field.enabled {
            sel = sel.patch(theme::dim());
        }
        Line::from(vec![
            Span::styled(label_piece, sel),
            Span::styled(value_piece, sel),
            Span::styled(widget_piece, sel),
        ])
    } else {
        let label_style = if field.enabled { Style::default() } else { theme::dim() };
        let value_style = if !field.enabled {
            theme::dim()
        } else if field.widget == "[toggle]" {
            toggle_tone(&field.value)
        } else {
            Style::default()
        };
        Line::from(vec![
            Span::styled(label_piece, label_style),
            Span::styled(value_piece, value_style),
            Span::styled(widget_piece, theme::dim()),
        ])
    };

    let mut out = vec![row1];
    // Disabled reason wins over the static description; both wrap under the label.
    let desc = if !field.enabled {
        field.reason.or(field.description)
    } else {
        field.description
    };
    if let Some(text) = desc {
        for wl in wrap(text, width.saturating_sub(4)) {
            out.push(Line::from(Span::styled(format!("    {wl}"), theme::dim())));
        }
    }
    out
}

/// Pad `s` with spaces to `n` columns, or truncate it (with `…`) if it is longer.
fn pad_to(s: &str, n: usize) -> String {
    let len = s.chars().count();
    if len >= n {
        truncate(s, n)
    } else {
        format!("{s}{}", " ".repeat(n - len))
    }
}

/// ok for an on/enabled toggle, dim for off — the text ("on"/"off") carries the
/// meaning; the tint only reinforces it.
fn toggle_tone(value: &str) -> Style {
    if value == "off" {
        theme::dim()
    } else {
        theme::ok()
    }
}

/// A plain, non-interactive info/action line (e.g. Account's status row or an
/// action item). `focused` gives it the accent bar.
pub fn action_line(text: &str, focused: bool, enabled: bool) -> Line<'static> {
    let style = if focused {
        focus_style()
    } else if !enabled {
        theme::dim()
    } else {
        Style::default()
    };
    Line::from(Span::styled(format!("  {text}"), style))
}

pub fn blank() -> Line<'static> {
    Line::from("")
}

/// A dim, wrapped note under a field (previews, hints).
pub fn note_line(text: &str) -> Line<'static> {
    Line::from(Span::styled(format!("    {text}"), theme::dim()))
}

/// A warn-tinted note (LAN exposure, DSD/auth safety, unknown-key preservation).
/// The copy already reads as a warning; the tint is a second channel, not the
/// only one.
pub fn warn_line(text: &str) -> Line<'static> {
    Line::from(Span::styled(format!("    {text}"), theme::warn()))
}

/// An error-tinted note (rejected bind/port). Same rule: the text stands alone.
pub fn err_line(text: &str) -> Line<'static> {
    Line::from(Span::styled(format!("    {text}"), theme::err()))
}

/// A multi-line note WORD-WRAPPED to `width` (FB5) — the long LAN/auth/export
/// copy no longer clips at the frame edge. `style` picks the tone (dim note,
/// warn, err). The 4-space indent matches `note_line`.
pub fn wrapped_note(text: &str, width: u16, style: Style) -> Vec<Line<'static>> {
    let inner = (width as usize).saturating_sub(4).max(1);
    wrap(text, inner)
        .into_iter()
        .map(|l| Line::from(Span::styled(format!("    {l}"), style)))
        .collect()
}

// ============================ section boxes ============================

/// One titled, rounded section box's worth of content. `active` (the group that
/// owns the focused field) borders in accent; the rest border dim.
pub struct Section {
    pub title: String,
    pub active: bool,
    pub lines: Vec<Line<'static>>,
}

impl Section {
    pub fn new(title: impl Into<String>, active: bool, lines: Vec<Line<'static>>) -> Self {
        Self { title: title.into(), active, lines }
    }
}

/// Stack titled, rounded section boxes top-to-bottom in `area`. Each box is sized
/// to its content (+2 for the border); a trailing filler keeps them compact at
/// the top rather than stretching. The active box borders + titles in accent.
pub fn sections(f: &mut Frame, area: Rect, secs: &[Section]) {
    if secs.is_empty() {
        return;
    }
    let mut constraints: Vec<Constraint> = secs
        .iter()
        .map(|s| Constraint::Length(s.lines.len() as u16 + 2))
        .collect();
    constraints.push(Constraint::Min(0));
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(area);
    for (i, sec) in secs.iter().enumerate() {
        let (border_style, title_style) = if sec.active {
            (theme::accent(), theme::accent_bold())
        } else {
            (theme::dim(), theme::dim())
        };
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(border_style)
            .title(Line::from(Span::styled(format!(" {} ", sec.title), title_style)));
        let inner = block.inner(chunks[i]);
        f.render_widget(block, chunks[i]);
        f.render_widget(Paragraph::new(sec.lines.clone()), inner);
    }
}

// ============================ follow-focus scroll (FB5) ============================

/// The virtual-line span of the focused field block inside a stacked-sections
/// render, so the viewport can scroll to keep it fully visible.
pub struct FocusAnchor {
    /// Index into the `secs` slice of the section that owns the focused block.
    pub section: usize,
    /// Line index of the block's first row WITHIN that section's `lines`.
    pub inner_line: u16,
    /// Rows the focused block occupies (control row + wrapped description).
    pub height: u16,
}

/// The minimal vertical scroll (in rows) that keeps `[focus_top, focus_top +
/// focus_height)` inside a `viewport`-tall window over `total` rows (FB5). Pure so
/// the follow-focus math is unit-tested independent of any buffer.
pub fn follow_scroll(focus_top: u16, focus_height: u16, viewport: u16, total: u16) -> u16 {
    if total <= viewport || viewport == 0 {
        return 0;
    }
    let max_scroll = total - viewport;
    let mut scroll = 0u16;
    let focus_bottom = focus_top.saturating_add(focus_height.max(1));
    if focus_bottom > viewport {
        scroll = focus_bottom - viewport;
    }
    // A block taller than the viewport (or above the current offset): pin its top.
    if focus_top < scroll {
        scroll = focus_top;
    }
    scroll.min(max_scroll)
}

/// Push a section and, if `within` names the focused block's (line, height)
/// inside it, record the screen-wide [`FocusAnchor`] (FB5). Keeps every screen's
/// section assembly a one-liner instead of hand-tracking section indices.
pub fn push_section(
    secs: &mut Vec<Section>,
    anchor: &mut Option<FocusAnchor>,
    title: impl Into<String>,
    active: bool,
    lines: Vec<Line<'static>>,
    within: Option<(u16, u16)>,
) {
    if let Some((inner_line, height)) = within {
        *anchor = Some(FocusAnchor { section: secs.len(), inner_line, height });
    }
    secs.push(Section::new(title, active, lines));
}

/// Total rows the stacked section boxes need (each box = its lines + 2 borders).
fn sections_height(secs: &[Section]) -> u16 {
    secs.iter().map(|s| s.lines.len() as u16 + 2).sum()
}

/// Stack section boxes like [`sections`], but follow-focus SCROLL when the
/// content is taller than `area` (FB5). When everything fits, it defers to
/// `sections` verbatim (top-aligned, no indicator). When it overflows, the boxes
/// render into a virtual buffer of the full height, the window that keeps the
/// focused block visible is blitted into `area`, and dim `▲`/`▼` indicators mark
/// hidden content above/below.
pub fn sections_scroll(f: &mut Frame, area: Rect, secs: &[Section], focus: Option<FocusAnchor>) {
    if secs.is_empty() {
        return;
    }
    let total = sections_height(secs);
    if total <= area.height {
        sections(f, area, secs);
        return;
    }

    let scroll = match &focus {
        Some(a) => {
            let mut y = 0u16;
            for s in secs.iter().take(a.section) {
                y = y.saturating_add(s.lines.len() as u16 + 2);
            }
            let focus_top = y + 1 + a.inner_line; // +1 for the box top border
            follow_scroll(focus_top, a.height, area.height, total)
        }
        None => 0,
    };

    // Render the stacked boxes into a full-height off-screen buffer.
    let mut buf = Buffer::empty(Rect { x: 0, y: 0, width: area.width, height: total });
    let mut y = 0u16;
    for sec in secs {
        let h = sec.lines.len() as u16 + 2;
        let rect = Rect { x: 0, y, width: area.width, height: h };
        let (border_style, title_style) = if sec.active {
            (theme::accent(), theme::accent_bold())
        } else {
            (theme::dim(), theme::dim())
        };
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(border_style)
            .title(Line::from(Span::styled(format!(" {} ", sec.title), title_style)));
        let inner = block.inner(rect);
        Widget::render(block, rect, &mut buf);
        Widget::render(Paragraph::new(sec.lines.clone()), inner, &mut buf);
        y = y.saturating_add(h);
    }

    // Blit the visible window into the frame.
    let fb = f.buffer_mut();
    for row in 0..area.height {
        let sy = scroll + row;
        if sy >= total {
            break;
        }
        for col in 0..area.width {
            if let Some(src) = buf.cell((col, sy)) {
                let cell = src.clone();
                if let Some(dst) = fb.cell_mut((area.x + col, area.y + row)) {
                    *dst = cell;
                }
            }
        }
    }

    // Dim scroll indicators at the right edge (content hidden above / below).
    let right = area.x + area.width.saturating_sub(1);
    if scroll > 0 {
        if let Some(c) = fb.cell_mut((right, area.y)) {
            c.set_symbol("▲");
            c.set_style(theme::dim());
        }
    }
    if scroll + area.height < total {
        if let Some(c) = fb.cell_mut((right, area.y + area.height.saturating_sub(1))) {
            c.set_symbol("▼");
            c.set_style(theme::dim());
        }
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

// ============================ help bar ============================

/// True only for tokens that are real key glyphs in the hint vocabulary: a
/// single character (`s`, `r`, `/`, `?`, `q`, `y`, `d`, `p`, …) or a named key.
/// Instructional words ("type") are NOT keys — their segment stays dim.
fn is_key_glyph(token: &str) -> bool {
    token.chars().count() == 1
        || matches!(
            token,
            "Esc" | "Enter" | "Tab" | "Shift-Tab" | "up/down" | "left/right" | "up" | "down"
        )
}

/// Split a `key desc · key desc` hint into accent-key / dim-description spans.
/// Segments are separated by ` · `; a segment's leading token is accent-tinted
/// only when it is a real key glyph — otherwise the whole segment is dim.
pub fn help_spans(text: &str) -> Vec<Span<'static>> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    for (i, seg) in text.split(" · ").enumerate() {
        if i > 0 {
            spans.push(Span::styled(" · ", theme::dim()));
        }
        match seg.split_once(' ') {
            Some((key, rest)) if is_key_glyph(key) => {
                spans.push(Span::styled(key.to_string(), theme::accent()));
                spans.push(Span::styled(format!(" {rest}"), theme::dim()));
            }
            None if is_key_glyph(seg) => {
                spans.push(Span::styled(seg.to_string(), theme::accent()));
            }
            _ => spans.push(Span::styled(seg.to_string(), theme::dim())),
        }
    }
    spans
}

/// The bottom help bar (one line): accent key glyphs, dim descriptions.
pub fn help_bar(f: &mut Frame, area: Rect, text: &str) {
    let mut spans = vec![Span::raw(" ")];
    spans.extend(help_spans(text));
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

// ============================ centered modal ============================

/// Center a fixed-size rect inside `area` (clamped to fit).
pub fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    let w = width.min(area.width);
    let h = height.min(area.height);
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    Rect { x, y, width: w, height: h }
}

/// A centered, bordered modal with a title, wrapped body and a hint footer.
pub fn modal(f: &mut Frame, area: Rect, title: &str, body: &str, hint: &str) {
    let lines = body.lines().count().max(1) as u16;
    let height = lines + 4; // border(2) + body + spacer + hint
    let width = body
        .lines()
        .chain(std::iter::once(title))
        .chain(std::iter::once(hint))
        .map(|l| l.chars().count())
        .max()
        .unwrap_or(20) as u16
        + 6;
    let rect = centered_rect(width.max(28), height.max(6), area);
    f.render_widget(Clear, rect);
    let block = titled_block(title);
    let inner = block.inner(rect);
    f.render_widget(block, rect);

    let mut text: Vec<Line> = body.lines().map(|l| Line::from(l.to_string())).collect();
    text.push(blank());
    text.push(Line::from(help_spans(hint)));
    f.render_widget(Paragraph::new(text).wrap(Wrap { trim: false }), inner);
}

/// A rounded, accent-bordered block with an accent-bold title — the shared frame
/// for modals, popups and panels.
fn titled_block(title: &str) -> Block<'static> {
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(theme::accent())
        .title(Line::from(Span::styled(format!(" {title} "), theme::accent_bold())))
}

/// A centered scrollable panel (help overlay, import summary, result panel).
pub fn panel(f: &mut Frame, area: Rect, title: &str, lines: Vec<Line<'static>>, scroll: u16) {
    let rect = centered_rect(area.width.saturating_sub(6).max(40), area.height.saturating_sub(4).max(10), area);
    f.render_widget(Clear, rect);
    let block = titled_block(title);
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    f.render_widget(
        Paragraph::new(lines).scroll((scroll, 0)).wrap(Wrap { trim: false }),
        inner,
    );
}

/// Spinner glyph for the given tick (§5.5 worker spinner).
pub fn spinner_frame(tick: u64) -> char {
    const FRAMES: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
    FRAMES[(tick as usize) % FRAMES.len()]
}

/// Busy overlay: a small centered spinner + label (§5.5). The spinner glyph is
/// accent; the label plain.
pub fn busy_overlay(f: &mut Frame, area: Rect, label: &str, tick: u64) {
    // Layout parity with the pre-theme overlay: body = "<spinner> <label>"
    // (label + 2 chars) + 6 → total rect width = label + 8.
    let width = label.chars().count() as u16 + 8;
    let rect = centered_rect(width, 5, area);
    f.render_widget(Clear, rect);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(theme::accent());
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    let line = Line::from(vec![
        Span::styled(format!("{} ", spinner_frame(tick)), theme::accent()),
        Span::raw(label.to_string()),
    ]);
    f.render_widget(
        Paragraph::new(line).alignment(Alignment::Center),
        inner,
    );
}

// ============================ select popup ============================

#[derive(Debug, Clone)]
pub struct SelectPopup {
    pub title: String,
    pub options: Vec<String>,
    /// Absolute index into `options` of the current selection.
    pub idx: usize,
    /// When true, printable keys filter the list (device picker, §3.2.2).
    pub filterable: bool,
    pub filter: String,
    /// Parallel to `options`: a bold section header rendered ABOVE that option
    /// when set (device-picker grouping, §3.2.2). Shown only when unfiltered.
    pub headers: Vec<Option<String>>,
}

pub enum SelectOutcome {
    Pending,
    Chosen(usize),
    Cancelled,
}

impl SelectPopup {
    pub fn new(title: &str, options: Vec<String>, selected: usize, filterable: bool) -> Self {
        let last = options.len().saturating_sub(1);
        let headers = vec![None; options.len()];
        Self {
            title: title.to_string(),
            options,
            idx: selected.min(last),
            filterable,
            filter: String::new(),
            headers,
        }
    }

    /// Attach parallel section headers (device picker). No-op if the length
    /// mismatches the option count.
    pub fn with_headers(mut self, headers: Vec<Option<String>>) -> Self {
        if headers.len() == self.options.len() {
            self.headers = headers;
        }
        self
    }

    /// Indices of options currently visible under the filter.
    pub fn visible(&self) -> Vec<usize> {
        if self.filter.is_empty() {
            return (0..self.options.len()).collect();
        }
        let needle = self.filter.to_ascii_lowercase();
        (0..self.options.len())
            .filter(|i| self.options[*i].to_ascii_lowercase().contains(&needle))
            .collect()
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> SelectOutcome {
        match key.code {
            KeyCode::Up => {
                self.step(-1);
                SelectOutcome::Pending
            }
            KeyCode::Down => {
                self.step(1);
                SelectOutcome::Pending
            }
            // j/k move ONLY on non-filterable popups; on a filterable one they
            // are filter characters.
            KeyCode::Char('k') if !self.filterable => {
                self.step(-1);
                SelectOutcome::Pending
            }
            KeyCode::Char('j') if !self.filterable => {
                self.step(1);
                SelectOutcome::Pending
            }
            KeyCode::Enter => {
                if self.visible().is_empty() {
                    SelectOutcome::Cancelled
                } else {
                    SelectOutcome::Chosen(self.idx)
                }
            }
            KeyCode::Esc => {
                if self.filterable && !self.filter.is_empty() {
                    self.filter.clear();
                    self.reselect_first();
                    SelectOutcome::Pending
                } else {
                    SelectOutcome::Cancelled
                }
            }
            KeyCode::Char('/') if self.filterable => {
                self.filter.clear();
                self.reselect_first();
                SelectOutcome::Pending
            }
            KeyCode::Backspace if self.filterable => {
                self.filter.pop();
                self.reselect_first();
                SelectOutcome::Pending
            }
            KeyCode::Char(c) if self.filterable => {
                self.filter.push(c);
                self.reselect_first();
                SelectOutcome::Pending
            }
            _ => SelectOutcome::Pending,
        }
    }

    /// Move the selection by `delta` within the visible (filtered) set, wrapping.
    fn step(&mut self, delta: isize) {
        let vis = self.visible();
        if vis.is_empty() {
            return;
        }
        let cur = vis.iter().position(|i| *i == self.idx).unwrap_or(0) as isize;
        let next = (cur + delta).rem_euclid(vis.len() as isize) as usize;
        self.idx = vis[next];
    }

    fn reselect_first(&mut self) {
        if let Some(first) = self.visible().first().copied() {
            self.idx = first;
        }
    }

    pub fn draw(&self, f: &mut Frame, area: Rect) {
        let vis = self.visible();
        let show_headers = self.filter.is_empty();
        let mut lines: Vec<Line> = Vec::new();
        let mut sel_line: u16 = 0;
        for i in &vis {
            if show_headers {
                if let Some(Some(h)) = self.headers.get(*i) {
                    lines.push(Line::from(Span::styled(h.clone(), theme::accent_bold())));
                }
            }
            let style = if *i == self.idx {
                sel_line = lines.len() as u16;
                focus_style()
            } else {
                Style::default()
            };
            lines.push(Line::from(Span::styled(
                format!("  {}", self.options[*i]),
                style,
            )));
        }
        if vis.is_empty() {
            lines.push(note_line("(no matches)"));
        }
        let hint = if self.filterable {
            format!(
                "{}   [{}]",
                strings::HELP_FILTER,
                if self.filter.is_empty() { "type to filter".into() } else { self.filter.clone() }
            )
        } else {
            strings::HELP_SELECT.to_string()
        };

        let height = (lines.len() as u16 + 4)
            .min(area.height.saturating_sub(2))
            .max(6);
        let width = self
            .options
            .iter()
            .map(|o| o.chars().count())
            .chain(std::iter::once(hint.chars().count()))
            .chain(std::iter::once(self.title.chars().count()))
            .max()
            .unwrap_or(20) as u16
            + 8;
        let rect = centered_rect(width.min(area.width.saturating_sub(2)).max(24), height, area);
        f.render_widget(Clear, rect);
        let block = titled_block(&self.title);
        let inner = block.inner(rect);
        f.render_widget(block, rect);

        let list_h = inner.height.saturating_sub(1);
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(list_h), Constraint::Length(1)])
            .split(inner);
        // Scroll so the selected line (accounting for header rows) stays visible.
        let scroll = sel_line.saturating_sub(list_h.saturating_sub(1));
        f.render_widget(Paragraph::new(lines).scroll((scroll, 0)), chunks[0]);
        f.render_widget(Paragraph::new(Line::from(help_spans(&hint))), chunks[1]);
    }
}

// ============================ line input ============================

#[derive(Debug, Clone, Default)]
pub struct TextInput {
    pub buf: String,
    pub masked: bool,
}

pub enum InputOutcome {
    Pending,
    Accepted,
    Cancelled,
}

impl TextInput {
    pub fn new(initial: &str, masked: bool) -> Self {
        Self { buf: initial.to_string(), masked }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> InputOutcome {
        match key.code {
            KeyCode::Enter => InputOutcome::Accepted,
            KeyCode::Esc => InputOutcome::Cancelled,
            KeyCode::Backspace => {
                self.buf.pop();
                InputOutcome::Pending
            }
            KeyCode::Char(c) => {
                self.buf.push(c);
                InputOutcome::Pending
            }
            _ => InputOutcome::Pending,
        }
    }

    pub fn display(&self) -> String {
        if self.masked {
            mask(&self.buf)
        } else {
            self.buf.clone()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- sidebar_width: 28 wide, 14 compact, with the 100-col boundary ----

    #[test]
    fn sidebar_width_doubles_only_at_and_above_100_cols() {
        assert_eq!(sidebar_width(80), 14, "the 80x24 floor keeps the compact sidebar");
        assert_eq!(sidebar_width(99), 14, "just below the boundary stays compact");
        assert_eq!(sidebar_width(100), 28, "at 100 the sidebar at least doubles");
        assert_eq!(sidebar_width(120), 28);
        assert!(sidebar_is_wide(120) && !sidebar_is_wide(80));
        // The wide sidebar is at least double the compact one (owner's ask).
        assert!(sidebar_width(120) >= 2 * sidebar_width(80));
    }

    // ---- wrap(): word boundaries, hard-split, edges ----

    #[test]
    fn wrap_empty_and_whitespace_yield_no_lines() {
        assert!(wrap("", 10).is_empty());
        assert!(wrap("   ", 10).is_empty());
        assert!(wrap("\n\n", 10).is_empty());
    }

    #[test]
    fn wrap_keeps_short_text_on_one_line() {
        assert_eq!(wrap("short note", 20), vec!["short note".to_string()]);
        // Exact-fit boundary: width == text length stays one line.
        assert_eq!(wrap("exactly ten", 11), vec!["exactly ten".to_string()]);
    }

    #[test]
    fn wrap_breaks_on_word_boundaries_not_mid_word() {
        // "anyone on your network" at width 12 wraps between words.
        let out = wrap("anyone on your network can control playback", 12);
        assert!(out.iter().all(|l| l.chars().count() <= 12), "no line exceeds width: {out:?}");
        // No word is split (every input word survives intact somewhere).
        for word in "anyone on your network can control playback".split(' ') {
            assert!(out.iter().any(|l| l.split(' ').any(|w| w == word)), "word {word:?} intact");
        }
    }

    #[test]
    fn wrap_hard_splits_a_word_longer_than_width() {
        // A 20-char token at width 8 is the only case a word breaks.
        let out = wrap("supercalifragilistic", 8);
        assert!(out.len() >= 3);
        assert!(out.iter().all(|l| l.chars().count() <= 8));
        assert_eq!(out.concat(), "supercalifragilistic", "no characters are lost");
    }

    #[test]
    fn wrap_honors_embedded_newlines_as_hard_breaks() {
        let out = wrap("line one\nline two", 40);
        assert_eq!(out, vec!["line one".to_string(), "line two".to_string()]);
    }

    // ---- control_column: one mechanism, consistent + clamped ----

    #[test]
    fn control_column_is_longest_label_plus_gutter_clamped() {
        // 2 indent + 17-char label + 2 gap = 21.
        assert_eq!(control_column(&["Streaming quality", "Gapless"], 62), 21);
        // A short label set still lands at the floor of 14.
        assert_eq!(control_column(&["Port"], 62), 14);
        // A pathological label is clamped so the control keeps room.
        let ceiling = 30u16.saturating_sub(12).max(14);
        assert_eq!(control_column(&["a very very very long label here"], 30), ceiling);
    }

    // ---- follow_scroll: keep the focused block visible, minimally ----

    #[test]
    fn follow_scroll_no_scroll_when_everything_fits() {
        assert_eq!(follow_scroll(0, 3, 18, 12), 0);
        assert_eq!(follow_scroll(9, 3, 18, 12), 0);
    }

    #[test]
    fn follow_scroll_brings_a_block_below_the_fold_into_view() {
        // total 30, viewport 18. A block at [20,23) needs scroll so its bottom (23)
        // sits at the viewport bottom: 23 - 18 = 5.
        assert_eq!(follow_scroll(20, 3, 18, 30), 5);
        // Clamped to max_scroll = 30 - 18 = 12.
        assert_eq!(follow_scroll(29, 3, 18, 30), 12);
    }

    #[test]
    fn follow_scroll_pins_top_of_a_block_taller_than_the_viewport() {
        // A 25-row block starting at 4, viewport 18 → pin to the block top (4).
        assert_eq!(follow_scroll(4, 25, 18, 40), 4);
    }

    #[test]
    fn sections_scroll_indicates_and_brings_the_focused_block_into_view() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;
        // Four 4-line boxes = 4 * (4 + 2) = 24 rows into a 12-row area → overflow.
        let mk = |t: &str| {
            Section::new(t, false, (0..4).map(|i| Line::from(format!("{t}-{i}"))).collect())
        };
        let secs = vec![mk("A"), mk("B"), mk("C"), mk("D")];
        // Focus the LAST box's first line — it starts well below the fold.
        let anchor = Some(FocusAnchor { section: 3, inner_line: 0, height: 1 });
        let mut term = Terminal::new(TestBackend::new(30, 12)).unwrap();
        term.draw(|f| sections_scroll(f, Rect::new(0, 0, 30, 12), &secs, anchor))
            .unwrap();
        let buf = term.backend().buffer().clone();
        let mut out = String::new();
        for y in 0..12 {
            for x in 0..30 {
                out.push_str(buf[(x, y)].symbol());
            }
            out.push('\n');
        }
        assert!(out.contains('▲'), "content hidden above → up indicator: \n{out}");
        assert!(out.contains('▼'), "content hidden below → down indicator");
        assert!(out.contains("D-0"), "the focused block is scrolled into view");
        assert!(!out.contains("A-0"), "the top box scrolled out of view");
    }

    // ---- field_block: fixed column, truncation, wrapped description ----

    fn cells(line: &Line) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn field_block_anchors_the_control_and_right_aligns_the_widget() {
        let f = Field {
            label: "Backend",
            value: "PipeWire".to_string(),
            widget: "[select]",
            focused: false,
            enabled: true,
            reason: None,
            description: None,
        };
        let block = field_block(&f, 21, 62);
        assert_eq!(block.len(), 1, "no description → just the control row");
        let row = cells(&block[0]);
        assert_eq!(row.chars().count(), 62, "the row spans the full width");
        // Label indented by 2, value starts at the control column.
        assert!(row.starts_with("  Backend"));
        assert_eq!(&row[21..29], "PipeWire", "value begins exactly at the control column");
        assert!(row.trim_end().ends_with("[select]"), "widget marker right-aligned");
    }

    #[test]
    fn field_block_truncates_a_long_value_with_ellipsis() {
        let f = Field {
            label: "Output device",
            value: "a-really-long-alsa-hardware-device-identifier-that-overflows".to_string(),
            widget: "[select]",
            focused: false,
            enabled: true,
            reason: None,
            description: None,
        };
        let block = field_block(&f, 21, 40); // narrow → must truncate
        let row = cells(&block[0]);
        assert_eq!(row.chars().count(), 40);
        assert!(row.contains('…'), "the overflowing value is truncated with an ellipsis");
    }

    #[test]
    fn field_block_wraps_the_disabled_reason_under_the_label() {
        let f = Field {
            label: "Gapless playback",
            value: "off".to_string(),
            widget: "[toggle]",
            focused: false,
            enabled: false,
            reason: Some("off while Audio > Streaming only on"),
            description: None,
        };
        let block = field_block(&f, 21, 40);
        assert!(block.len() >= 2, "the reason wraps onto its own dim row(s)");
        // Every description row is indented and within the width.
        for row in &block[1..] {
            let t = cells(row);
            assert!(t.starts_with("    "), "description indented under the label");
            assert!(t.chars().count() <= 40);
        }
    }
}
