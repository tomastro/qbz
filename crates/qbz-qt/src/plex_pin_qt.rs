//! Plex PIN sign-in — the "Authorize" half of Settings > Local Library > Plex.
//!
//! Port of `crates/qbz/src/plex_auth.rs`'s PIN lifecycle (`generate_code`
//! :413-476, `begin_pin_poll` :478-560, `open_auth_url` :569, `copy_code`
//! :584). Nothing here re-implements the protocol: `qbz_plex` already ships
//! `plex_auth_pin_start` / `_check` / `plex_open_auth_url` (lib.rs:920, :941,
//! :971) and this port simply never called them, so the only sign-in route
//! was pasting an `X-Plex-Token` by hand.
//!
//! ## The shape, and why it is a task and not a Timer
//!
//! The reference polls with a Slint `Timer` on the event-loop thread because
//! that is the only place a Slint Timer may be driven. Qt has no such
//! constraint here — the poll lives in a plain tokio task with a 2500 ms
//! interval, the same period.
//!
//! Two guards carried over verbatim, because both exist for a reason the
//! reference paid for:
//!
//! 1. **A generation counter.** Every start bumps it; every tick and every
//!    in-flight check re-reads it and drops itself if it is stale. Without it
//!    a check that was already in flight when the user hit Authorize again
//!    can land AFTER the newer one and persist an older token.
//! 2. **Self-termination when the panel is gone.** The reference stops the
//!    poll when the settings section is no longer Local Library, so a user
//!    who wanders off does not leave a 2.5 s heartbeat against plex.tv for
//!    the rest of the session. Here the panel tells us directly
//!    ([`stop_poll`] from `QbzLocal::plex_stop_pin`), because a QML panel DOES
//!    get an unmount hook and does not need the polling workaround.
//!
//! The server url is captured ONCE, when the code is generated, and threaded
//! through the poll. The reference has a comment about this that is worth
//! keeping: its authorized branch runs on a tokio thread where the UI handle
//! is gone, so re-reading the field there yields a blank base url. The same
//! discipline applies for the same reason — the field can also have been
//! edited by the user while the browser tab was open.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use crate::local_bridge::ui;
use crate::local_plex as plex;
use cxx_qt_lib::QString;

/// Bumped on every start/stop so a stale in-flight check is ignored.
static PIN_GEN: AtomicU64 = AtomicU64::new(0);

/// The live code + its browser url, for the "Link code" row and the two
/// buttons beside it. Empty when no PIN is outstanding.
static PIN: LazyLock<Mutex<PinState>> = LazyLock::new(|| Mutex::new(PinState::default()));

#[derive(Default, Clone)]
struct PinState {
    code: String,
    auth_url: String,
}

fn publish(code: &str, auth_url: &str, busy: bool) {
    let (c, u) = (code.to_string(), auth_url.to_string());
    ui(move |mut b| {
        b.as_mut().set_pin_code(QString::from(c.as_str()));
        b.as_mut().set_pin_auth_url(QString::from(u.as_str()));
        b.as_mut().set_pin_busy(busy);
    });
}

fn set_error(msg: String) {
    ui(move |mut b| b.as_mut().set_plex_error(QString::from(msg.as_str())));
}

/// Clear the outstanding PIN and make every in-flight check a no-op.
///
/// Called when the panel unmounts, on disconnect, and on every terminal
/// outcome (authorized / expired / failed).
pub fn stop_poll() {
    PIN_GEN.fetch_add(1, Ordering::SeqCst);
    *PIN.lock().unwrap_or_else(|e| e.into_inner()) = PinState::default();
    publish("", "", false);
}

/// "Generate code": persist the address, ask plex.tv for a PIN, publish the
/// code, and start polling for the authorization.
///
/// Gated exactly like the reference (`plex_auth.rs:416`): Plex enabled, a LAN
/// address, and a resolvable base url. The gate matters — a PIN issued
/// against an address we will refuse to store later would authorize into
/// nothing.
pub async fn generate_code(server_url: String) {
    if !plex::is_enabled() {
        return;
    }
    if !plex::is_local_address(&server_url) || plex::resolve_base_url(&server_url).is_empty() {
        set_error(qbz_i18n::t("Only local network servers are supported."));
        return;
    }
    publish("", "", true);

    let client_id = plex::client_id();
    let pin = match qbz_plex::plex_auth_pin_start(client_id.clone()).await {
        Ok(p) => p,
        Err(e) => {
            log::warn!("[qbz-qt] plex pin start failed: {e}");
            publish("", "", false);
            set_error(qbz_i18n::t("Plex link failed to start"));
            crate::toast_qt::error(qbz_i18n::t("Plex link failed to start"));
            return;
        }
    };

    {
        let mut st = PIN.lock().unwrap_or_else(|e| e.into_inner());
        st.code = pin.code.clone();
        st.auth_url = pin.auth_url.clone();
    }
    publish(&pin.code, &pin.auth_url, false);
    set_error(String::new());
    crate::toast_qt::success(qbz_i18n::t_args(
        "Enter code {} at the Plex sign-in page",
        &[&pin.code],
    ));

    let my_gen = PIN_GEN.fetch_add(1, Ordering::SeqCst) + 1;
    let pin_id = pin.pin_id;
    let code = pin.code.clone();
    crate::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_millis(2500));
        // The first tick of a tokio interval fires immediately; skip it so the
        // first check lands 2.5 s in, like the reference's Repeated timer.
        ticker.tick().await;
        loop {
            ticker.tick().await;
            if PIN_GEN.load(Ordering::SeqCst) != my_gen {
                return;
            }
            let check =
                match qbz_plex::plex_auth_pin_check(client_id.clone(), pin_id, Some(code.clone()))
                    .await
                {
                    Ok(c) => c,
                    Err(e) => {
                        // A single failed poll is not terminal — plex.tv 5xxs
                        // happen and the user is mid-browser. Keep ticking;
                        // the expiry branch below is what ends it.
                        log::warn!("[qbz-qt] plex pin check failed: {e}");
                        continue;
                    }
                };
            // Re-check AFTER the await: the request was in flight for a while.
            if PIN_GEN.load(Ordering::SeqCst) != my_gen {
                return;
            }
            if check.authorized {
                let Some(token) = check.auth_token else {
                    continue;
                };
                stop_poll();
                let token = token.trim().to_string();
                let url = server_url.clone();
                let tok = token.clone();
                let base = tokio::task::spawn_blocking(move || {
                    plex::set_enabled(true);
                    plex::connect_manual(&url, &tok)
                })
                .await
                .unwrap_or_default();
                if base.is_empty() {
                    set_error(qbz_i18n::t("Enter a valid server address."));
                    return;
                }
                // The PIN path is not manual-token mode — the reference
                // clears the flag here so the panel goes back to showing the
                // Authorize row instead of the token field.
                plex::set_manual_token_mode(false);
                crate::toast_qt::success(qbz_i18n::t("Connected to Plex"));
                crate::local_bridge_ops::publish_plex_state();
                crate::local_bridge_ops::run_sync();
                return;
            }
            if check.expired {
                stop_poll();
                set_error(qbz_i18n::t("The code expired. Generate a new one."));
                crate::toast_qt::error(qbz_i18n::t("The code expired. Generate a new one."));
                return;
            }
        }
    });
}

/// Open the Plex sign-in page in the user's browser.
pub async fn open_auth_url() {
    let url = PIN
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .auth_url
        .clone();
    if url.is_empty() {
        return;
    }
    if let Err(e) = qbz_plex::plex_open_auth_url(url).await {
        log::warn!("[qbz-qt] open Plex auth url failed: {e}");
    }
}

/// The live code, for the copy button (empty when none is outstanding).
pub fn current_code() -> String {
    PIN.lock().unwrap_or_else(|e| e.into_inner()).code.clone()
}

/// "Check connection": ping the stored server and report what answered.
///
/// Two jobs in one call, and the second is the quiet one: a successful ping
/// carries the server's `machineIdentifier`, which is what stamps `server_id`
/// on every cached row. `plex_ping` had ZERO callers in this port, so the
/// reader at `local_plex.rs`'s cache layer was reading a column only the
/// Slint build ever wrote — a Qt-only install had `server_id = NULL`
/// everywhere.
pub async fn check_connection() {
    let (base_url, token) = plex::credentials();
    if base_url.is_empty() || token.is_empty() {
        set_error(qbz_i18n::t("Plex is not configured."));
        return;
    }
    ui(|mut b| b.as_mut().set_plex_syncing(true));
    let result = qbz_plex::plex_ping(base_url, token).await;
    ui(|mut b| b.as_mut().set_plex_syncing(false));

    match result {
        Ok(info) => {
            if let Some(id) = info.machine_identifier.as_deref().filter(|s| !s.is_empty()) {
                plex::set_machine_id(id);
            }
            set_error(String::new());
            let name = info
                .friendly_name
                .as_deref()
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| qbz_i18n::t("Plex"));
            crate::toast_qt::success(qbz_i18n::t_args("Connected to {}", &[&name]));
            crate::local_bridge_ops::publish_plex_state();
        }
        Err(e) => {
            log::warn!("[qbz-qt] plex ping failed: {e}");
            set_error(e);
            crate::toast_qt::error(qbz_i18n::t("Could not reach the Plex server."));
        }
    }
}
