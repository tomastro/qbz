// crates/qbzd/src/tui/screens/account.rs — the Account screen (03 §3.1).
//
// Status + inline auth. All auth work reuses the T5 engine (login.rs) — the TUI
// adds zero auth logic. Token paste is fully inline (masked input →
// login_with_token_arg, which validates via validate_token BEFORE persisting).
// Browser login is a suspend-and-run handoff to the engine (see the task report):
// the TUI leaves the alternate screen, the engine prints the URL + 300 s wait +
// the SSH-forward hint on failure, then the TUI resumes. The Status row NEVER
// fabricates a name offline — it shows only "credential file present".

use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::Frame;

use crate::tui::app::{DrawCtx, ScreenAction};
use crate::tui::strings as s;
use crate::tui::widgets::{self, InputOutcome, TextInput};

/// Snapshot of the auth state the App resolves (from `GET /api/status` when the
/// daemon runs, else from credential-file presence).
#[derive(Debug, Clone, Default)]
pub struct AuthSnapshot {
    pub logged_in: bool,
    pub email: Option<String>,
    pub plan: Option<String>,
    /// Offline + daemon-down: a credential file exists but is unvalidated.
    pub cred_file_present: bool,
}

pub struct AccountState {
    auth: AuthSnapshot,
    focus: usize,
    token_input: Option<TextInput>,
    confirm_logout: bool,
}

impl AccountState {
    pub fn new(auth: AuthSnapshot) -> Self {
        Self {
            auth,
            focus: 0,
            token_input: None,
            confirm_logout: false,
        }
    }

    pub fn set_auth(&mut self, auth: AuthSnapshot) {
        self.auth = auth;
    }

    // Account is all immediate actions — it never participates in dirty-state,
    // so the App's `active_is_dirty` short-circuits it (no `is_dirty` here).
    pub fn is_editing(&self) -> bool {
        self.token_input.is_some() || self.confirm_logout
    }

    /// The breadcrumb's level-2 node when an INLINE field edit is active. The
    /// logout confirm is a third-level modal (an overlay) — the breadcrumb
    /// underneath stays `Setup › Account`, so it returns None.
    pub fn editing_label(&self) -> Option<&'static str> {
        if self.token_input.is_some() {
            Some(s::ACCOUNT_PASTE_TOKEN)
        } else {
            None
        }
    }

    /// Action rows in focus order, depending on login state.
    fn actions(&self) -> Vec<&'static str> {
        if self.auth.logged_in {
            vec![s::ACCOUNT_LOGOUT, s::ACCOUNT_PASTE_TOKEN]
        } else {
            vec![s::ACCOUNT_LOGIN_BROWSER, s::ACCOUNT_PASTE_TOKEN]
        }
    }

    // -------------------------- input --------------------------

    pub fn handle_key(&mut self, key: KeyEvent) -> ScreenAction {
        if let Some(mut input) = self.token_input.take() {
            match input.handle_key(key) {
                InputOutcome::Accepted => {
                    let token = input.buf.trim().to_string();
                    if token.is_empty() {
                        return ScreenAction::Consumed;
                    }
                    return ScreenAction::LoginToken(token);
                }
                InputOutcome::Cancelled => return ScreenAction::Consumed,
                InputOutcome::Pending => {
                    self.token_input = Some(input);
                    return ScreenAction::Consumed;
                }
            }
        }
        if self.confirm_logout {
            match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    self.confirm_logout = false;
                    return ScreenAction::Logout;
                }
                KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') => {
                    self.confirm_logout = false;
                    return ScreenAction::Consumed;
                }
                _ => return ScreenAction::Consumed,
            }
        }

        let actions = self.actions();
        match key.code {
            KeyCode::Up | KeyCode::Char('k') | KeyCode::BackTab => {
                self.focus = if self.focus == 0 { actions.len() - 1 } else { self.focus - 1 };
                ScreenAction::Consumed
            }
            KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab => {
                self.focus = (self.focus + 1) % actions.len();
                ScreenAction::Consumed
            }
            KeyCode::Enter | KeyCode::Char(' ') => match actions[self.focus] {
                s::ACCOUNT_LOGIN_BROWSER => ScreenAction::LoginBrowser,
                s::ACCOUNT_PASTE_TOKEN => {
                    self.token_input = Some(TextInput::new("", true));
                    ScreenAction::Consumed
                }
                s::ACCOUNT_LOGOUT => {
                    self.confirm_logout = true;
                    ScreenAction::Consumed
                }
                _ => ScreenAction::Consumed,
            },
            KeyCode::Esc => ScreenAction::Back,
            _ => ScreenAction::Consumed,
        }
    }

    // -------------------------- render --------------------------

    pub fn draw(&self, f: &mut Frame, area: Rect, _ctx: &DrawCtx) {
        let width = area.width.saturating_sub(2); // section inner width
        let ctrl_col = widgets::control_column(&[s::ACCOUNT_STATUS], width);
        let mut lines: Vec<Line> = Vec::new();

        // Status row — never fabricates a name (§3.1).
        let status = if self.auth.logged_in {
            match (&self.auth.email, &self.auth.plan) {
                (Some(e), Some(p)) => s::account_logged_in_plan(e, p),
                (Some(e), None) => s::account_logged_in(e),
                _ => "logged in".to_string(),
            }
        } else if self.auth.cred_file_present {
            s::ACCOUNT_CRED_PRESENT.to_string()
        } else {
            s::ACCOUNT_NOT_LOGGED_IN.to_string()
        };
        lines.extend(widgets::field_block(
            &widgets::Field {
                label: s::ACCOUNT_STATUS,
                value: status,
                widget: "",
                focused: false,
                enabled: true,
                reason: None,
                description: None,
            },
            ctrl_col,
            width,
        ));
        lines.push(widgets::blank());

        let mut anchor: Option<widgets::FocusAnchor> = None;
        for (i, action) in self.actions().iter().enumerate() {
            let focused = i == self.focus && !self.is_editing();
            if focused {
                anchor = Some(widgets::FocusAnchor { section: 0, inner_line: lines.len() as u16, height: 1 });
            }
            lines.push(widgets::action_line(&format!("> {action}"), focused, true));
        }

        let secs = [widgets::Section::new(s::ACCOUNT_SECTION, true, lines)];
        widgets::sections_scroll(f, area, &secs, anchor);

        if let Some(input) = &self.token_input {
            widgets::modal(f, area, s::ACCOUNT_PASTE_TOKEN, &input.display(), s::HELP_INPUT);
        } else if self.confirm_logout {
            widgets::modal(
                f,
                area,
                s::ACCOUNT_LOGOUT_CONFIRM_TITLE,
                s::ACCOUNT_LOGOUT_CONFIRM_BODY,
                s::CONFIRM_YN,
            );
        }
    }
}
