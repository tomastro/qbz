// crates/qbzd/src/tui/theme.rs — the setup TUI's one small palette.
//
// QBZ's visual identity is restrained: ONE accent (a calm cyan) plus a handful
// of semantic slots. Named colors only — they degrade cleanly on 16-, 8- and
// monochrome terminals and over serial. Color NEVER carries meaning on its own:
// the `*` dirty marker, the on/off labels, the [BP] badge and every glyph stay
// as text (serial-console safety, 03-setup-tui.md §1.2). Color only reinforces
// state a reader can already see without it.

use ratatui::style::{Color, Modifier, Style};

/// The single accent — screen/section titles, active borders, selected rows,
/// key glyphs, the ▸ menu marker.
pub const ACCENT: Color = Color::Cyan;
/// OK / on / running.
pub const OK: Color = Color::Green;
/// Attention — the dirty `*`, needs-auth, exposure and safety warnings.
pub const WARN: Color = Color::Yellow;
/// Error — rejected input, failures.
pub const ERR: Color = Color::Red;

/// Accent text (titles, focused labels, key glyphs).
pub fn accent() -> Style {
    Style::default().fg(ACCENT)
}

/// Accent, emphasized — section/screen titles.
pub fn accent_bold() -> Style {
    Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
}

/// Dim — help descriptions, inactive borders, secondary/summary text, notes.
pub fn dim() -> Style {
    Style::default().add_modifier(Modifier::DIM)
}

/// The selected/focused row: accent reversed. Terminal-theme agnostic (reverse
/// survives even where the accent foreground is invisible), so it doubles as the
/// serial-safe selection affordance.
pub fn selection() -> Style {
    Style::default().fg(ACCENT).add_modifier(Modifier::REVERSED)
}

pub fn ok() -> Style {
    Style::default().fg(OK)
}
pub fn warn() -> Style {
    Style::default().fg(WARN)
}
pub fn err() -> Style {
    Style::default().fg(ERR)
}
