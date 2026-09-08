// crates/qbzd/src/tui/mod.rs — `qbzd setup` entry, terminal lifecycle, event loop
// (03-setup-tui.md §2). Non-tty is rejected before any terminal mutation (§2.4).
// `ratatui::init()` enables raw mode + the alternate screen AND installs a panic
// hook that restores the terminal on any panic (§2.1 — a wedged terminal over
// SSH is a support fire); `ratatui::restore()` restores on the normal exit path.
//
// The event loop runs synchronously on a dedicated blocking thread (spawned off
// the tokio runtime), holding a runtime `Handle` so the discrete I/O actions
// (§5.5: screen entry, `r`, save, immediate actions) run on workers or a
// short-lived `block_on` — never on a keystroke.

pub mod app;
pub mod clipboard;
pub mod strings;
pub mod theme;
pub mod widgets;
pub mod wizard_core;

mod screens;

use std::io::{IsTerminal, Write};
use std::time::Duration;

use ratatui::crossterm::event::{self, Event, KeyEventKind};
use ratatui::DefaultTerminal;
use tokio::runtime::Handle;

use crate::paths::ProfileRoots;
use app::{App, LoopCmd};

/// Poll cadence — also the spinner tick while a worker runs (§5.5). No I/O here;
/// the loop only redraws and drains the worker channel.
const TICK: Duration = Duration::from_millis(120);

/// `qbzd setup` entry. Exit 2 on a non-tty (§2.4); else runs the configurator and
/// returns 0.
pub async fn run(roots: ProfileRoots) -> i32 {
    // §2.4: reject a non-interactive invocation BEFORE touching the terminal.
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        eprintln!("{}", strings::NON_TTY_ERROR);
        return 2;
    }
    let handle = Handle::current();
    // The synchronous ratatui loop runs on a blocking thread so its `block_on`
    // calls are legal (a runtime worker thread cannot block_on itself).
    tokio::task::spawn_blocking(move || run_sync(roots, handle))
        .await
        .unwrap_or(1)
}

fn run_sync(roots: ProfileRoots, handle: Handle) -> i32 {
    let mut terminal = ratatui::init();
    let mut app = App::new(roots, handle.clone());
    let code = event_loop(&mut terminal, &mut app, &handle);
    ratatui::restore();
    code
}

fn event_loop(terminal: &mut DefaultTerminal, app: &mut App, handle: &Handle) -> i32 {
    loop {
        if terminal.draw(|f| app.draw(f)).is_err() {
            return 1;
        }
        app.drain_worker();
        if app.busy() {
            app.busy_tick = app.busy_tick.wrapping_add(1);
        }

        if event::poll(TICK).unwrap_or(false) {
            match event::read() {
                Ok(Event::Key(key)) if key.kind == KeyEventKind::Press => match app.on_key(key) {
                    LoopCmd::None => {}
                    LoopCmd::BrowserLogin => run_browser_login(terminal, app, handle),
                    LoopCmd::ScrobbleLastfm => {
                        run_scrobble_login(terminal, app, handle, ScrobbleProvider::Lastfm)
                    }
                    LoopCmd::ScrobbleListenbrainz => {
                        run_scrobble_login(terminal, app, handle, ScrobbleProvider::Listenbrainz)
                    }
                },
                Ok(_) => {} // resize/mouse/etc. — the next draw re-lays-out (§5.4)
                Err(_) => return 1,
            }
        }

        if app.should_quit() {
            return 0;
        }
    }
}

/// Suspend the alt-screen and run the T5 browser-login engine on the plain
/// terminal (it prints the OAuth URL, waits 300 s, and prints the SSH-forward
/// hint on failure), then resume the TUI. Deliberate divergence from the §3.1
/// in-panel countdown — see the task report.
fn run_browser_login(terminal: &mut DefaultTerminal, app: &mut App, handle: &Handle) {
    ratatui::restore();
    println!("{}\n", strings::ACCOUNT_BROWSER_HANDOFF);
    let roots = app.roots().clone();
    let result = handle.block_on(async { crate::login::login_browser(&roots, None).await });
    *terminal = ratatui::init();
    let mapped = result
        .map(|session| (session.email, Some(session.subscription_label)))
        .map_err(|e| e.to_string());
    app.after_browser_login(mapped);
}

/// Which scrobbler provider a suspended connect flow targets.
enum ScrobbleProvider {
    Lastfm,
    Listenbrainz,
}

/// Suspend the alt-screen and run the scrobbler connect flow on the plain
/// terminal — the SAME methodology as the browser login. Last.fm prints an
/// authorize URL and waits for Enter (the CLI verb owns that); ListenBrainz
/// prompts for a pasted user token here, then hands it to the CLI verb. Both
/// write the canonical ScrobblerSettingsStore, so the screen is reloaded on
/// resume.
fn run_scrobble_login(
    terminal: &mut DefaultTerminal,
    app: &mut App,
    handle: &Handle,
    provider: ScrobbleProvider,
) {
    ratatui::restore();
    let roots = app.roots().clone();
    match provider {
        ScrobbleProvider::Lastfm => {
            println!("{}", strings::SCROBBLE_LASTFM_HANDOFF);
            let _ = handle.block_on(async { crate::cli::scrobble::login_lastfm(None, &roots).await });
        }
        ScrobbleProvider::Listenbrainz => {
            println!("{}", strings::SCROBBLE_LISTENBRAINZ_HANDOFF);
            print!("token: ");
            let _ = std::io::stdout().flush();
            let mut token = String::new();
            if std::io::stdin().read_line(&mut token).is_ok() {
                let token = token.trim().to_string();
                if token.is_empty() {
                    println!("no token entered — skipped.");
                } else {
                    let _ = handle
                        .block_on(async { crate::cli::scrobble::login_listenbrainz(None, token, &roots).await });
                }
            }
        }
    }
    println!("{}", strings::SCROBBLE_RETURN_HINT);
    let mut line = String::new();
    let _ = std::io::stdin().read_line(&mut line);
    *terminal = ratatui::init();
    app.refresh_scrobbler();
}
