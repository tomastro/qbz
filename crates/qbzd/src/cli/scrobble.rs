// crates/qbzd/src/cli/scrobble.rs — the `qbzd scrobble …` verbs (CONSOLE ext).
// Connect Last.fm / ListenBrainz and manage scrobbling, using the SAME
// methodology as `qbzd login`: Last.fm prints an authorize URL and exchanges
// the token after the user approves; ListenBrainz takes a pasted user token
// (like `login --token`).
//
// Credentials land in the CANONICAL scrobbler store
// (`qbz_app::settings::scrobblers::ScrobblerSettingsStore`, SQLite at the daemon
// data root) — the SAME store the desktop uses and the one the settings bundle
// export/import already carries. A running daemon is nudged to reload so the
// scrobble-on-play driver picks up the new credentials. These are LOCAL,
// daemon-down-capable operations, like `login`/`settings set`.
use std::io::{BufRead, Write};

use qbz_app::settings::scrobblers::ScrobblerSettingsStore;

use crate::cli::client::ApiClient;
use crate::paths::ProfileRoots;

/// `qbzd scrobble login lastfm` — the Last.fm web-auth flow (print URL →
/// user approves → exchange for a session key), mirroring `qbzd login`.
pub async fn login_lastfm(host: Option<String>, roots: &ProfileRoots) -> i32 {
    let mut client = qbz_integrations::lastfm::LastFmClient::new();
    let (token, auth_url) = match client.get_token().await {
        Ok(v) => v,
        Err(e) => {
            eprintln!("error: Last.fm token request failed: {e}");
            eprintln!("  → check your connection and retry");
            return 1;
        }
    };
    println!("Authorize QBZ on Last.fm, then come back here:");
    println!("  {auth_url}");
    print!("Press Enter after you've clicked \"Yes, allow access\"… ");
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    let _ = std::io::stdin().lock().read_line(&mut line);

    let session = match client.get_session(&token).await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: Last.fm authorization not completed: {e}");
            eprintln!("  → approve access on the page first, then run this again");
            return 1;
        }
    };
    qbz_log::register_secret(session.key.clone());

    let store = match open_store(roots) {
        Ok(s) => s,
        Err(code) => return code,
    };
    if let Err(e) = store
        .set_lastfm_session(&session.key, &session.name)
        .and_then(|_| store.set_lastfm_enabled(true))
        .and_then(|_| store.set_enabled(true))
    {
        eprintln!("error: {e}");
        return 1;
    }
    nudge_reload(host).await;
    println!("Last.fm connected as {} — scrobbling enabled", session.name);
    0
}

/// `qbzd scrobble login listenbrainz --token <TOKEN>` — validate and store a
/// ListenBrainz user token (from listenbrainz.org/settings).
pub async fn login_listenbrainz(host: Option<String>, token: String, roots: &ProfileRoots) -> i32 {
    let client = qbz_integrations::listenbrainz::ListenBrainzClient::new();
    let info = match client.set_token(&token).await {
        Ok(i) => i,
        Err(e) => {
            eprintln!("error: ListenBrainz token rejected: {e}");
            eprintln!("  → get a token at https://listenbrainz.org/settings/");
            return 1;
        }
    };
    qbz_log::register_secret(token.clone());

    let store = match open_store(roots) {
        Ok(s) => s,
        Err(code) => return code,
    };
    if let Err(e) = store
        .set_listenbrainz_token(&token, &info.user_name)
        .and_then(|_| store.set_listenbrainz_enabled(true))
        .and_then(|_| store.set_enabled(true))
    {
        eprintln!("error: {e}");
        return 1;
    }
    nudge_reload(host).await;
    println!("ListenBrainz connected as {} — scrobbling enabled", info.user_name);
    0
}

/// `qbzd scrobble status` — per-provider connection + enabled state.
pub fn status(roots: &ProfileRoots) -> i32 {
    let store = match open_store(roots) {
        Ok(s) => s,
        Err(code) => return code,
    };
    let s = match store.get_settings() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: {e}");
            return 1;
        }
    };
    println!(
        "last.fm       : {}",
        provider_line(s.lastfm_is_authed(), s.lastfm_active(), &s.lastfm_username)
    );
    println!(
        "listenbrainz  : {}",
        provider_line(s.listenbrainz_is_authed(), s.listenbrainz_active(), &s.listenbrainz_username)
    );
    0
}

/// `qbzd scrobble enable|disable <lastfm|listenbrainz>` — keep the credentials
/// but start/stop scrobbling to that provider.
pub async fn set_enabled(host: Option<String>, provider: String, enabled: bool, roots: &ProfileRoots) -> i32 {
    let store = match open_store(roots) {
        Ok(s) => s,
        Err(code) => return code,
    };
    let s = match store.get_settings() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: {e}");
            return 1;
        }
    };
    let result = match provider.as_str() {
        "lastfm" if !s.lastfm_is_authed() => return not_connected(&provider),
        "listenbrainz" if !s.listenbrainz_is_authed() => return not_connected(&provider),
        "lastfm" => store.set_lastfm_enabled(enabled),
        "listenbrainz" => store.set_listenbrainz_enabled(enabled),
        other => {
            eprintln!("error: unknown provider '{other}'");
            eprintln!("  → lastfm | listenbrainz");
            return 2;
        }
    };
    if let Err(e) = result {
        eprintln!("error: {e}");
        return 1;
    }
    // Enabling a provider also lifts the master toggle (off = nothing scrobbles).
    if enabled {
        let _ = store.set_enabled(true);
    }
    nudge_reload(host).await;
    println!("{provider} scrobbling {}", if enabled { "enabled" } else { "disabled" });
    0
}

// ============================ internals ============================

fn open_store(roots: &ProfileRoots) -> Result<ScrobblerSettingsStore, i32> {
    ScrobblerSettingsStore::new_at(&roots.data).map_err(|e| {
        eprintln!("error: cannot open the scrobbler store: {e}");
        1
    })
}

fn not_connected(provider: &str) -> i32 {
    eprintln!("error: {provider} is not connected");
    eprintln!("  → connect it first: qbzd scrobble login {provider}");
    1
}

fn provider_line(authed: bool, active: bool, name: &str) -> String {
    match (authed, active) {
        (false, _) => "not connected".to_string(),
        (true, true) => format!("on as {name}"),
        (true, false) => format!("off (connected as {name})"),
    }
}

/// Best-effort: tell a running daemon to reload so the scrobble-on-play driver
/// picks up new credentials. Silent if the daemon is down — the store write is
/// what matters.
async fn nudge_reload(host: Option<String>) {
    let roots = crate::paths::ProfileRoots::resolve(None, None);
    let client = ApiClient::new(host, &roots);
    let _ = client.post("/api/settings/reload", serde_json::Value::Null).await;
}
