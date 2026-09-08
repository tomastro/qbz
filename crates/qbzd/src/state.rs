// crates/qbzd/src/state.rs — shared in-memory daemon state (one
// `Arc<Mutex<DaemonShared>>` shared by the playback driver + the HTTP API).
// Fields land now; real sources wire in as each producing task lands
// (T3 driver/audio, T6 HTTP server, T7 transport, T9/T10 QConnect).
#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct LatchedErrors {
    // 01 §9.4 — drain-once channels become latches
    pub stream: Option<String>,
    pub auth: Option<String>,
    pub transport: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthState {
    NeedsAuth,
    Restoring,
    LoggedIn,
} // 01 §6.2 machine

pub struct DaemonShared {
    // one Arc<Mutex<...>> shared by driver + API
    pub auth: AuthState,
    pub user_id: Option<u64>,
    pub subscription: Option<String>,
    pub last_errors: LatchedErrors,
    pub driver_last_tick: Option<std::time::Instant>,
    pub muted: bool,
    pub premute_volume: f32,
    pub started_at: std::time::Instant,
    pub startup_warnings: u32,
    pub qconnect: QconnectStatus,
    /// T11 (`POST /api/settings/reload`, 02 §3.3.17): a fingerprint of the
    /// credential-file token currently applied to the live session, so reload
    /// can tell "new token on disk" (re-login) from "same token, unrelated
    /// nudge" (no-op) without keeping a second copy of the secret in memory.
    /// `None` whenever the daemon is not LoggedIn against a known token
    /// (cleared alongside every `set_needs_auth`).
    pub credential_fingerprint: Option<u64>,
    /// Coarse network-reachability signal for `/api/status`'s `network.online`
    /// (01 §9.3). Latched ONLY from real network-class outcomes — never active
    /// probing: false on an auth-retry/credential-reload network-class failure
    /// or a QConnect reconnect-exhausted, true on a successful login/restore
    /// (`restore_activate`, which reload's own credential validation also
    /// funnels through) or a successful QConnect (re)connect. Defaults true
    /// (optimistic) until the first outcome latches it.
    pub network_online: std::sync::atomic::AtomicBool,
    /// CoreEvent bus handle so state mutations here can publish matching bus
    /// events (`emit_qconnect_session_changed`). None until `daemon::run`
    /// attaches it right after boot; tests leave it None.
    pub bus: Option<tokio::sync::broadcast::Sender<qbz_models::CoreEvent>>,
}

impl DaemonShared {
    /// Read the latched network-reachability signal (§9.3). `Relaxed` is
    /// sufficient — this is a coarse status flag read under the same
    /// `Mutex<DaemonShared>` guard as every other field here, not a
    /// synchronization primitive of its own.
    pub fn network_online(&self) -> bool {
        self.network_online
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Latch the network-reachability signal. See the field doc for exactly
    /// which call sites are allowed to call this.
    pub fn set_network_online(&self, online: bool) {
        self.network_online
            .store(online, std::sync::atomic::Ordering::Relaxed);
    }

    /// Publish the CURRENT `qconnect` block as a `QconnectSessionChanged` bus
    /// event (SSE `/api/events`, `qbzd watch`, the event hook). Call AFTER
    /// mutating `self.qconnect`; a best-effort no-op before the bus attaches
    /// or when no receiver is subscribed.
    pub fn emit_qconnect_session_changed(&self) {
        if let Some(bus) = &self.bus {
            let _ = bus.send(qbz_models::CoreEvent::QconnectSessionChanged {
                state: self.qconnect.state.clone(),
                device_name: (!self.qconnect.device_name.is_empty())
                    .then(|| self.qconnect.device_name.clone()),
                session_active: self.qconnect.session_active,
            });
        }
    }
}

/// A non-reversible-in-practice fingerprint of a credential token (SipHash via
/// the stdlib default hasher) — used ONLY to detect "the file changed", never
/// to reconstruct the token. Keeps `DaemonShared` from holding a second live
/// copy of the secret alongside the credential file + the Qobuz client.
pub fn token_fingerprint(token: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    token.hash(&mut hasher);
    hasher.finish()
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct QconnectStatus {
    pub enabled: bool,
    pub state: String, // "off"|"connecting"|"connected"|"retrying"|"exhausted"
    pub session_active: bool,
    pub device_name: String,
    pub last_transport_reconnect: Option<String>,
    /// LAN receiver lifecycle. Contract values are
    /// off|binding|listening|validating|delegated|restoring|error.
    pub lan_state: String,
    /// Bound receiver port, published only while the LAN service is live.
    pub lan_port: Option<u16>,
    /// Credential authority currently driving QConnect: owner|delegated.
    pub credential_origin: String,
    /// Latest candidate under validation/commit, never a session identifier.
    pub candidate_generation: Option<u64>,
    /// Sanitized LAN failure code. Never contains an endpoint, JWT, body, or IP.
    pub last_lan_error: Option<String>,
}

impl Default for QconnectStatus {
    fn default() -> Self {
        Self {
            enabled: false,
            state: String::new(),
            session_active: false,
            device_name: String::new(),
            last_transport_reconnect: None,
            lan_state: "off".to_string(),
            lan_port: None,
            credential_origin: "owner".to_string(),
            candidate_generation: None,
            last_lan_error: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latched_errors_default_all_none() {
        let e = LatchedErrors::default();
        assert!(e.stream.is_none());
        assert!(e.auth.is_none());
        assert!(e.transport.is_none());
    }

    #[test]
    fn qconnect_status_default_is_off_and_inactive() {
        let q = QconnectStatus::default();
        assert!(!q.enabled);
        assert_eq!(q.state, "");
        assert!(!q.session_active);
        assert_eq!(q.device_name, "");
        assert!(q.last_transport_reconnect.is_none());
        assert_eq!(q.lan_state, "off");
        assert!(q.lan_port.is_none());
        assert_eq!(q.credential_origin, "owner");
        assert!(q.candidate_generation.is_none());
        assert!(q.last_lan_error.is_none());
    }

    #[test]
    fn qconnect_lan_observability_serializes_with_contract_field_names() {
        let mut q = QconnectStatus::default();
        q.lan_state = "validating".to_string();
        q.lan_port = Some(49_152);
        q.credential_origin = "delegated".to_string();
        q.candidate_generation = Some(7);
        q.last_lan_error = Some("qws_auth_rejected".to_string());

        let value = serde_json::to_value(q).expect("serialize QConnect status");
        assert_eq!(value["lan_state"], "validating");
        assert_eq!(value["lan_port"], 49_152);
        assert_eq!(value["credential_origin"], "delegated");
        assert_eq!(value["candidate_generation"], 7);
        assert_eq!(value["last_lan_error"], "qws_auth_rejected");
    }

    #[test]
    fn auth_state_serializes_to_contract_strings() {
        // 02-cli-and-api.md §3.3.3: auth.state ∈ logged_in|needs_auth|restoring
        assert_eq!(
            serde_json::to_string(&AuthState::NeedsAuth).unwrap(),
            "\"needs_auth\""
        );
        assert_eq!(
            serde_json::to_string(&AuthState::Restoring).unwrap(),
            "\"restoring\""
        );
        assert_eq!(
            serde_json::to_string(&AuthState::LoggedIn).unwrap(),
            "\"logged_in\""
        );
    }

    #[test]
    fn daemon_shared_holds_the_fields_the_status_route_needs() {
        // Construction smoke test: DaemonShared has no derive (Instant isn't
        // Serialize) so this is the only compile-time guard that the field
        // set/types stay what api::status::assemble expects.
        let shared = DaemonShared {
            auth: AuthState::LoggedIn,
            user_id: Some(1234567),
            subscription: Some("studio".into()),
            last_errors: LatchedErrors::default(),
            driver_last_tick: None,
            muted: false,
            premute_volume: 1.0,
            started_at: std::time::Instant::now(),
            startup_warnings: 0,
            qconnect: QconnectStatus::default(),
            credential_fingerprint: None,
            network_online: std::sync::atomic::AtomicBool::new(true),
            bus: None,
        };
        assert_eq!(shared.auth, AuthState::LoggedIn);
        assert_eq!(shared.user_id, Some(1234567));
    }

    #[test]
    fn network_online_latches_false_then_true() {
        // Pure latch semantics (01 §9.3): a real network-class failure flips
        // it false, a real success flips it back true — the exact two
        // transitions every call site above drives. Defaults true.
        let shared = DaemonShared {
            auth: AuthState::Restoring,
            user_id: None,
            subscription: None,
            last_errors: LatchedErrors::default(),
            driver_last_tick: None,
            muted: false,
            premute_volume: 1.0,
            started_at: std::time::Instant::now(),
            startup_warnings: 0,
            qconnect: QconnectStatus::default(),
            credential_fingerprint: None,
            network_online: std::sync::atomic::AtomicBool::new(true),
            bus: None,
        };
        assert!(shared.network_online(), "defaults true (optimistic)");

        shared.set_network_online(false);
        assert!(!shared.network_online(), "set false -> reads back false");

        shared.set_network_online(true);
        assert!(shared.network_online(), "set true -> reads back true");
    }

    #[test]
    fn token_fingerprint_is_stable_and_distinguishes_tokens() {
        let a = token_fingerprint("token-a");
        let a_again = token_fingerprint("token-a");
        let b = token_fingerprint("token-b");
        assert_eq!(a, a_again);
        assert_ne!(a, b);
    }
}
