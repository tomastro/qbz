// crates/qbzd/src/tui/screens/network.rs — the Network screen (03 §3.5).
//
// Edits ONLY [server] bind/port/token in qbzd.toml. The save is a whole-file
// parse → update → rewrite that preserves EVERY other key (known schema keys and
// unrecognized ones alike) — the J5 silent-revert guard. bind/port/token cannot
// rebind live, so the save result names the restart. The LAN-exposure warning is
// shown verbatim when bind is non-loopback.

use std::net::IpAddr;

use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::Frame;

use crate::config::QbzdConfig;
use crate::tui::app::{DrawCtx, ScreenAction};
use crate::tui::strings as s;
use crate::tui::theme;
use crate::tui::widgets::{self, InputOutcome, TextInput};

#[derive(Debug, Clone, PartialEq)]
struct Staged {
    bind: String,
    port: String,
    token: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NField {
    Bind,
    Port,
    Token,
}

const FIELDS: [NField; 3] = [NField::Bind, NField::Port, NField::Token];

pub struct NetworkState {
    baseline: Staged,
    staged: Staged,
    focus: usize,
    editor: Option<(NField, TextInput)>,
    /// Unrecognized qbzd.toml keys (named in the pre-save warning, §3.5).
    unknown_keys: Vec<String>,
}

impl NetworkState {
    pub fn new(cfg: &QbzdConfig, unknown_keys: Vec<String>) -> Self {
        let staged = Staged {
            bind: cfg.server.bind.clone(),
            port: cfg.server.port.to_string(),
            token: cfg.server.token.clone().unwrap_or_default(),
        };
        Self {
            baseline: staged.clone(),
            staged,
            focus: 0,
            editor: None,
            unknown_keys,
        }
    }

    pub fn is_dirty(&self) -> bool {
        self.staged != self.baseline
    }
    pub fn is_editing(&self) -> bool {
        self.editor.is_some()
    }
    pub fn mark_saved(&mut self) {
        self.baseline = self.staged.clone();
    }

    /// The breadcrumb's level-2 node when a field editor is active.
    pub fn editing_label(&self) -> Option<&'static str> {
        self.editor.as_ref().map(|(f, _)| match f {
            NField::Bind => s::N_BIND,
            NField::Port => s::N_PORT,
            NField::Token => s::N_TOKEN,
        })
    }

    /// Validated (bind, port, token) ready for the TOML rewrite, or field errors.
    pub fn validated(&self) -> Result<(String, u16, Option<String>), String> {
        if self.staged.bind.parse::<IpAddr>().is_err() {
            return Err(s::N_BAD_IP.to_string());
        }
        let port: u16 = match self.staged.port.parse() {
            Ok(p) if p >= 1 => p,
            _ => return Err(s::N_BAD_PORT.to_string()),
        };
        let token = if self.staged.token.trim().is_empty() {
            None
        } else {
            Some(self.staged.token.clone())
        };
        Ok((self.staged.bind.clone(), port, token))
    }

    /// A non-loopback bind is reachable beyond localhost (§3.5); 0.0.0.0
    /// (unspecified) binds every interface, so it warns too.
    fn bind_is_lan(&self) -> bool {
        self.staged
            .bind
            .parse::<IpAddr>()
            .map(|ip| !ip.is_loopback())
            .unwrap_or(false)
    }

    fn port_invalid(&self) -> bool {
        !self.staged.port.parse::<u16>().map(|p| p >= 1).unwrap_or(false)
    }

    // -------------------------- input --------------------------

    pub fn handle_key(&mut self, key: KeyEvent) -> ScreenAction {
        if let Some((field, mut input)) = self.editor.take() {
            match input.handle_key(key) {
                InputOutcome::Accepted => {
                    let v = input.buf.trim().to_string();
                    match field {
                        NField::Bind => self.staged.bind = v,
                        NField::Port => self.staged.port = v,
                        NField::Token => self.staged.token = input.buf.clone(),
                    }
                }
                InputOutcome::Cancelled => {}
                InputOutcome::Pending => self.editor = Some((field, input)),
            }
            return ScreenAction::Consumed;
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
            KeyCode::Char('s') => ScreenAction::Save,
            KeyCode::Enter => {
                let field = FIELDS[self.focus];
                let (initial, masked) = match field {
                    NField::Bind => (self.staged.bind.clone(), false),
                    NField::Port => (self.staged.port.clone(), false),
                    NField::Token => (self.staged.token.clone(), true),
                };
                self.editor = Some((field, TextInput::new(&initial, masked)));
                ScreenAction::Consumed
            }
            KeyCode::Esc => ScreenAction::Back,
            _ => ScreenAction::Consumed,
        }
    }

    // -------------------------- render --------------------------

    pub fn draw(&self, f: &mut Frame, area: Rect, _ctx: &DrawCtx) {
        let width = area.width.saturating_sub(2); // section inner width
        let labels: Vec<&str> = FIELDS
            .iter()
            .map(|f| match f {
                NField::Bind => s::N_BIND,
                NField::Port => s::N_PORT,
                NField::Token => s::N_TOKEN,
            })
            .collect();
        let ctrl_col = widgets::control_column(&labels, width);

        let mut lines: Vec<Line> = Vec::new();
        let mut within: Option<(u16, u16)> = None;
        for (i, field) in FIELDS.iter().enumerate() {
            let focused = i == self.focus && self.editor.is_none();
            let editing = self.editor.as_ref().map(|(nf, _)| *nf == *field).unwrap_or(false);
            let (label, value, widget) = match field {
                NField::Bind => (s::N_BIND, self.field_value(NField::Bind, editing), "[input]"),
                NField::Port => (s::N_PORT, self.field_value(NField::Port, editing), "[input]"),
                NField::Token => {
                    let v = if editing {
                        self.editor.as_ref().map(|(_, i)| i.display()).unwrap_or_default()
                    } else if self.staged.token.trim().is_empty() {
                        s::N_TOKEN_HINT.to_string()
                    } else {
                        widgets::mask(&self.staged.token)
                    };
                    (s::N_TOKEN, v, "[input]")
                }
            };
            let start = lines.len() as u16;
            let block = widgets::field_block(
                &widgets::Field { label, value, widget, focused, enabled: true, reason: None, description: None },
                ctrl_col,
                width,
            );
            if focused {
                within = Some((start, block.len() as u16));
            }
            lines.extend(block);
        }

        // Field-level validation notes (§4.2): the short errors stay one line; the
        // long exposure/preservation copy is now WORD-WRAPPED so it never clips.
        if self.staged.bind.parse::<IpAddr>().is_err() {
            lines.push(widgets::err_line(s::N_BAD_IP));
        } else if self.bind_is_lan() {
            lines.push(widgets::blank());
            lines.extend(widgets::wrapped_note(s::NETWORK_LAN_POSTURE, width, theme::dim()));
        }
        if self.port_invalid() {
            lines.push(widgets::err_line(s::N_BAD_PORT));
        }

        // Pre-save unknown-key warning (§3.5).
        if !self.unknown_keys.is_empty() {
            lines.push(widgets::blank());
            lines.extend(widgets::wrapped_note(s::N_DROP_UNKNOWN, width, theme::warn()));
            lines.extend(widgets::wrapped_note(&self.unknown_keys.join(", "), width, theme::warn()));
        }

        let mut secs: Vec<widgets::Section> = Vec::new();
        let mut anchor: Option<widgets::FocusAnchor> = None;
        widgets::push_section(&mut secs, &mut anchor, s::NETWORK_SECTION, true, lines, within);
        widgets::sections_scroll(f, area, &secs, anchor);

        if let Some((field, input)) = &self.editor {
            let title = match field {
                NField::Bind => s::N_BIND,
                NField::Port => s::N_PORT,
                NField::Token => s::N_TOKEN,
            };
            widgets::modal(f, area, title, &input.display(), s::HELP_INPUT);
        }
    }

    fn field_value(&self, field: NField, editing: bool) -> String {
        if editing {
            if let Some((_, input)) = &self.editor {
                return input.display();
            }
        }
        match field {
            NField::Bind => self.staged.bind.clone(),
            NField::Port => self.staged.port.clone(),
            NField::Token => self.staged.token.clone(),
        }
    }
}

/// Whole-file rewrite: parse `existing` (or start empty), update only
/// [server] bind/port/token, keep every other key verbatim (§3.5). An empty or
/// whitespace-only token REMOVES the key (open control plane).
pub fn rewrite_toml(
    existing: &str,
    bind: &str,
    port: u16,
    token: Option<&str>,
) -> Result<String, String> {
    let mut root: toml::Table = if existing.trim().is_empty() {
        toml::Table::new()
    } else {
        toml::from_str(existing).map_err(|e| format!("cannot parse qbzd.toml: {e}"))?
    };
    let server = root
        .entry("server".to_string())
        .or_insert_with(|| toml::Value::Table(toml::Table::new()));
    let st = server
        .as_table_mut()
        .ok_or_else(|| "qbzd.toml [server] is not a table".to_string())?;
    st.insert("bind".to_string(), toml::Value::String(bind.to_string()));
    st.insert("port".to_string(), toml::Value::Integer(port as i64));
    match token {
        Some(t) if !t.trim().is_empty() => {
            st.insert("token".to_string(), toml::Value::String(t.to_string()));
        }
        _ => {
            st.remove("token");
        }
    }
    toml::to_string(&root).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrite_preserves_unknown_and_known_keys() {
        // A file with a released key (data_root), a schema key we don't edit
        // (log.level) and an unrecognized key must ALL survive a server edit.
        let existing = "config_version = 1\ndata_root = \"/srv/qbzd\"\n\n[server]\nbind = \"127.0.0.1\"\nport = 8182\n\n[log]\nlevel = \"debug\"\n\n[weird]\nkey = \"value\"\n";
        let out = rewrite_toml(existing, "0.0.0.0", 9000, Some("secret")).unwrap();
        let parsed: toml::Table = toml::from_str(&out).unwrap();
        assert_eq!(parsed["config_version"].as_integer(), Some(1));
        assert_eq!(parsed["data_root"].as_str(), Some("/srv/qbzd"));
        assert_eq!(parsed["log"]["level"].as_str(), Some("debug"));
        assert_eq!(parsed["weird"]["key"].as_str(), Some("value"));
        assert_eq!(parsed["server"]["bind"].as_str(), Some("0.0.0.0"));
        assert_eq!(parsed["server"]["port"].as_integer(), Some(9000));
        assert_eq!(parsed["server"]["token"].as_str(), Some("secret"));
    }

    #[test]
    fn empty_token_clears_the_key() {
        let existing = "[server]\nport = 8182\ntoken = \"old\"\n";
        let out = rewrite_toml(existing, "127.0.0.1", 8182, None).unwrap();
        let parsed: toml::Table = toml::from_str(&out).unwrap();
        assert!(parsed["server"].get("token").is_none(), "empty token removes the key");
    }

    #[test]
    fn bad_ip_and_port_are_rejected() {
        let cfg = QbzdConfig::default();
        let mut st = NetworkState::new(&cfg, Vec::new());
        st.staged.bind = "not-an-ip".to_string();
        assert!(st.validated().is_err());
        st.staged.bind = "127.0.0.1".to_string();
        st.staged.port = "70000".to_string();
        assert!(st.validated().is_err());
        st.staged.port = "8182".to_string();
        assert!(st.validated().is_ok());
    }
}
