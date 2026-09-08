// crates/qbzd/src/tui/screens/scrobbler.rs — the Scrobbler setup screen
// (CONSOLE ext, owner-sanctioned 8th section). Shows Last.fm / ListenBrainz
// connection state and runs the connect flows with the SAME methodology as the
// Account login: the App suspends the alt-screen and runs the CLI auth flow on
// the plain terminal (Last.fm prints an authorize URL; ListenBrainz prompts for
// a pasted token), then resumes. No inline save — the auth flows write the
// canonical ScrobblerSettingsStore directly; enable/disable is a CLI verb.
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use qbz_app::settings::scrobblers::{ScrobblerSettings, ScrobblerSettingsStore};

use crate::paths::ProfileRoots;
use crate::tui::app::{DrawCtx, ScreenAction};
use crate::tui::theme;

pub struct ScrobblerState {
    settings: ScrobblerSettings,
}

impl ScrobblerState {
    pub fn new(roots: &ProfileRoots) -> Self {
        let settings = ScrobblerSettingsStore::new_at(&roots.data)
            .and_then(|s| s.get_settings())
            .unwrap_or_default();
        Self { settings }
    }

    /// This screen has no inline field editor (its actions suspend to the plain
    /// terminal), so it never captures the keyboard for editing.
    pub fn is_editing(&self) -> bool {
        false
    }

    pub fn editing_label(&self) -> Option<&'static str> {
        None
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> ScreenAction {
        match key.code {
            KeyCode::Char('l') | KeyCode::Char('L') => ScreenAction::ScrobbleConnectLastfm,
            KeyCode::Char('b') | KeyCode::Char('B') => ScreenAction::ScrobbleConnectListenbrainz,
            KeyCode::Esc => ScreenAction::Back,
            _ => ScreenAction::Consumed,
        }
    }

    pub fn draw(&self, f: &mut Frame, area: Rect, _ctx: &DrawCtx) {
        let s = &self.settings;
        let lines: Vec<Line> = vec![
            Line::from(Span::styled("Scrobbling", theme::accent_bold())),
            Line::from(""),
            provider_line("Last.fm", s.lastfm_is_authed(), s.lastfm_active(), &s.lastfm_username),
            provider_line(
                "ListenBrainz",
                s.listenbrainz_is_authed(),
                s.listenbrainz_active(),
                &s.listenbrainz_username,
            ),
            Line::from(""),
            Line::from(Span::styled(
                "  L  connect Last.fm      B  connect ListenBrainz",
                theme::dim(),
            )),
            Line::from(Span::styled(
                "  enable/disable: qbzd scrobble enable|disable <provider>",
                theme::dim(),
            )),
        ];
        f.render_widget(Paragraph::new(lines), area);
    }
}

fn provider_line(name: &str, authed: bool, active: bool, user: &str) -> Line<'static> {
    let status = match (authed, active) {
        (false, _) => "not connected".to_string(),
        (true, true) => format!("on · {user}"),
        (true, false) => format!("off · connected as {user}"),
    };
    Line::from(vec![
        Span::styled(format!("  {name:<14}"), theme::accent()),
        Span::raw(status),
    ])
}
