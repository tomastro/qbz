// crates/qbzd/src/api/mod.rs — the HTTP control plane (02-cli-and-api.md §3).
//
// tiny_http 0.12, single listener, thread-per-connection collapsed to ONE
// serving thread (requests are handled inline, serialized — a control plane
// sees a handful of clients, never a load). Two mechanisms qualify the
// otherwise-open surface (§3.1.2):
//   - Origin shield (ALWAYS on): any request carrying an `Origin` header is
//     refused `403 origin_forbidden` before routing — CLI/curl/scripts send no
//     `Origin`, browsers do (CSRF / DNS-rebinding guard at zero UX cost).
//   - Opt-in `[server] token`: when set, `Authorization: Bearer <token>` is
//     required on every route EXCEPT `GET /api/ping`; a mismatch is
//     `401 invalid_token`. Unset = no auth machinery exists at runtime.
//
// The two-call split — `bind` at boot step 5 (stateless, so the foreign-occupant
// diagnosis runs BEFORE the stores/runtime exist), `serve` at boot step 11 — is
// what keeps the 01-architecture.md §8.1 boot order honest.
pub mod browse;
pub mod discover;
pub mod fav;
pub mod lyrics;
pub mod play;
pub mod playback;
pub mod playlist;
pub mod queue;
pub mod radio;
pub mod reco;
pub mod search;
pub mod artwork;
pub mod settings;
pub mod sse;
pub mod status;

use std::io::Cursor;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use tiny_http::{Header, Method, Request, Response};
use tokio::sync::broadcast;

use crate::adapter::DaemonAdapter;
use crate::paths::ProfileRoots;
use crate::state::DaemonShared;
use qbz_app::shell::AppRuntime;
use qbz_audio::settings::AudioSettingsStore;
use qbz_models::CoreEvent;

/// The counted P0 route table (02 §3.2) — 17, FINAL, this test now guards the
/// budget forever. A route exists only iff a shipped client calls it (§3.1.4).
/// T1-T6 landed the first 3; T7 added the 9 playback + now-playing routes
/// (rows 4-12); T8 added the 4 queue routes (rows 13-16); T11 (this task)
/// adds `/api/settings/reload` (row 17) — the inline
/// `route_table_matches_spec_count` test pins the number so the 68-routes
/// failure shape (a route with no caller) cannot creep back in.
pub const P0_ROUTES: &[(&str, &str)] = &[
    ("GET", "/api/ping"),
    ("GET", "/api/info"),
    ("GET", "/api/status"),
    ("GET", "/api/now-playing"),
    ("POST", "/api/playback/play"),
    ("POST", "/api/playback/pause"),
    ("POST", "/api/playback/toggle"),
    ("POST", "/api/playback/stop"),
    ("POST", "/api/playback/next"),
    ("POST", "/api/playback/previous"),
    ("POST", "/api/playback/seek"),
    ("POST", "/api/playback/volume"),
    ("GET", "/api/queue"),
    ("POST", "/api/queue/add"),
    ("POST", "/api/queue/remove"),
    ("POST", "/api/queue/clear"),
    ("POST", "/api/settings/reload"),
];

/// The P1 route table (02 §3.4) — the content-verb surface. SAME HARD RULE as
/// P0 (§3.1.4): a route exists only iff a shipped CLI/TUI client calls it, and
/// this table grows one row per verb+route pair in the SAME change as the verb.
/// Row 19 (this change): `GET /api/search` — caller: `qbzd search`. The
/// `p1_route_table_grows_only_with_a_shipped_caller` test pins the count and
/// forbids overlap with P0, so the 68-routes failure shape cannot creep in
/// through the P1 door either.
pub const P1_ROUTES: &[(&str, &str)] = &[
    ("GET", "/api/search"),
    ("POST", "/api/play"),
    ("GET", "/api/album"),
    ("GET", "/api/artist"),
    ("GET", "/api/similar"),
    ("GET", "/api/suggest"),
    ("GET", "/api/discover"),
    ("GET", "/api/lyrics"),
    ("POST", "/api/radio"),
    ("POST", "/api/reco/playlist"),
    ("POST", "/api/playback/shuffle"),
    ("POST", "/api/playback/repeat"),
    ("POST", "/api/queue/move"),
    ("POST", "/api/queue/jump"),
    ("POST", "/api/queue/stop-after"),
    ("GET", "/api/favorites"),
    ("POST", "/api/favorites/add"),
    ("POST", "/api/favorites/remove"),
    ("GET", "/api/playlists"),
    ("GET", "/api/playlist"),
    ("POST", "/api/playlist/create"),
    ("POST", "/api/playlist/update"),
    ("POST", "/api/playlist/delete"),
    ("POST", "/api/playlist/tracks/add"),
    ("POST", "/api/playlist/tracks/remove"),
    ("GET", "/api/events"),
    ("GET", "/api/artwork/current"),
];

/// A socket bound at boot step 5, not yet serving. Wraps the tiny_http server
/// in an `Arc` so the serving thread and the shutdown handle can both hold it
/// (`unblock` from the handle terminates the thread's `incoming_requests`).
pub struct BoundServer {
    server: Arc<tiny_http::Server>,
}

/// Everything the route handlers read. Owned by the single serving thread
/// (moved into it by [`serve`]), so it only needs `Send`, never `Sync` — which
/// is why a plain `AudioSettingsStore` (rusqlite `Connection`: Send, not Sync)
/// can live here directly. `token` is the opt-in `[server] token`, read once at
/// boot (`None` = open).
pub struct ApiState {
    pub runtime: Arc<AppRuntime<DaemonAdapter>>,
    pub shared: Arc<Mutex<DaemonShared>>,
    /// The CoreEvent bus (DaemonAdapter sender). `/api/events` subscribes a
    /// receiver per SSE connection; no other route touches it.
    pub bus: broadcast::Sender<CoreEvent>,
    pub roots: ProfileRoots,
    pub token: Option<String>,
    /// The bound address, echoed verbatim by `/api/info`.
    pub bind: String,
    /// Handle to the daemon's tokio runtime — the serving thread is a plain
    /// `std::thread`, so async core calls (`get_queue_state`) run via
    /// `Handle::block_on` (never called from a runtime worker → no panic).
    pub rt: tokio::runtime::Handle,
    /// Second read-only connection to the daemon-root audio settings DB (WAL
    /// allows it alongside the Player's). Supplies `configured_device`/`backend`.
    pub audio: AudioSettingsStore,
    /// Cached device enumeration for `device_present` (refreshed on a TTL so a
    /// `status` poll never re-enumerates CPAL on every call).
    pub devices: Mutex<DeviceCache>,
    /// T11: the `AudioSettings` last applied to the `Player`, so
    /// `POST /api/settings/reload` can tell whether a routing-critical field
    /// changed since the previous reload (`daemon::audio_routing_changed`) —
    /// reinit only when it did, never on every unrelated nudge.
    pub audio_snapshot: Mutex<qbz_audio::settings::AudioSettings>,
    /// T11: the live cell the playback driver's background auto-advance reads
    /// for streaming quality (`daemon.rs::run`'s `quality_cell`) — reload
    /// writes a fresh value here after re-reading `daemon_prefs`.
    pub quality: Arc<Mutex<qbz_models::Quality>>,
    /// T11: reaches the running QConnect service (`connect`/`disconnect`/
    /// device-name refresh) once `qconnect::start` (boot step 12, AFTER the
    /// API starts serving at step 11) publishes it — empty only in the brief
    /// window between the two, which the reload handler no-ops through.
    pub qconnect_control: Arc<std::sync::OnceLock<crate::qconnect::QconnectControl>>,
}

/// TTL-cached output-device names for the `device_present` check.
#[derive(Default)]
pub struct DeviceCache {
    pub at: Option<Instant>,
    pub names: Vec<String>,
}

/// Live serving handle. [`ApiHandle::shutdown`] unblocks the serving thread and
/// joins it — dropping the thread's `ApiState` (and with it the `Arc<AppRuntime>`
/// clone) BEFORE the daemon drops the runtime, preserving the §8.2 audio
/// clock-release ordering (the API thread is one more `Arc<AppRuntime>` holder,
/// exactly like the driver and auth-retry tasks).
pub struct ApiHandle {
    server: Arc<tiny_http::Server>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl ApiHandle {
    pub fn shutdown(mut self) {
        self.server.unblock();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Why a bind failed — `AddrInUse` is the case the boot step-5 diagnosis probes
/// (foreign qbzd vs another process); everything else is a generic fatal.
#[derive(Debug)]
pub enum BindError {
    AddrInUse(SocketAddr),
    Other(String),
}

/// Boot step 5 (01 §8.1): bind only — stateless, so the foreign-occupant
/// diagnosis (in `daemon.rs`) runs BEFORE stores (6) and runtime composition (7).
pub fn bind(addr: SocketAddr) -> Result<BoundServer, BindError> {
    match tiny_http::Server::http(addr) {
        Ok(server) => Ok(BoundServer {
            server: Arc::new(server),
        }),
        Err(e) => Err(classify_bind_error(e, addr)),
    }
}

fn classify_bind_error(
    e: Box<dyn std::error::Error + Send + Sync + 'static>,
    addr: SocketAddr,
) -> BindError {
    if let Some(io) = e.downcast_ref::<std::io::Error>() {
        if io.kind() == std::io::ErrorKind::AddrInUse {
            return BindError::AddrInUse(addr);
        }
    }
    BindError::Other(e.to_string())
}

/// Best-effort occupant probe for the step-5 diagnosis: `GET /api/ping` and
/// check the response identifies as qbzd (`"app":"qbzd"`). Loopback, short
/// timeout, no dependency on reqwest (the CLI's async client is not built here).
pub fn probe_is_qbzd(addr: SocketAddr) -> bool {
    use std::io::{Read, Write};
    use std::net::TcpStream;
    use std::time::Duration;
    let mut stream = match TcpStream::connect_timeout(&addr, Duration::from_millis(500)) {
        Ok(s) => s,
        Err(_) => return false,
    };
    let _ = stream.set_read_timeout(Some(Duration::from_secs(1)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(1)));
    let req = format!(
        "GET /api/ping HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n"
    );
    if stream.write_all(req.as_bytes()).is_err() {
        return false;
    }
    let _ = stream.flush();
    let mut buf = Vec::new();
    let _ = stream.take(4096).read_to_end(&mut buf);
    let text = String::from_utf8_lossy(&buf);
    text.contains("\"app\":\"qbzd\"")
}

/// Boot step 11 (01 §8.1): start serving on the already-bound socket. Requests
/// are handled inline on one thread (serialized). `unblock` (from the handle)
/// ends `incoming_requests` for graceful shutdown.
pub fn serve(server: BoundServer, state: ApiState) -> ApiHandle {
    // The counted P0 surface — logged at boot so an operator sees exactly how
    // many routes this build exposes (and it anchors the const to production).
    log::info!(
        "control plane serving {} route(s) ({} P0 + {} P1)",
        P0_ROUTES.len() + P1_ROUTES.len(),
        P0_ROUTES.len(),
        P1_ROUTES.len()
    );
    let srv = server.server;
    let srv_handle = srv.clone();
    let thread = std::thread::Builder::new()
        .name("qbzd-api".into())
        .spawn(move || {
            for mut req in srv.incoming_requests() {
                // `/api/events` is a long-lived SSE stream: it would block this
                // single serving thread forever. Move it onto its OWN thread
                // (Request is Send) so the control plane keeps answering. The
                // origin/token gate is applied first, identically to `route`.
                let is_events = *req.method() == Method::Get
                    && req.url().split('?').next() == Some("/api/events");
                if is_events {
                    let has_origin = req.headers().iter().any(|h| h.field.equiv("Origin"));
                    let auth = req
                        .headers()
                        .iter()
                        .find(|h| h.field.equiv("Authorization"))
                        .map(|h| h.value.as_str().to_owned());
                    if let Some(reject) =
                        access_gate(has_origin, "GET", "/api/events", auth.as_deref(), state.token.as_deref())
                    {
                        let _ = req.respond(reject.response());
                        continue;
                    }
                    let rx = state.bus.subscribe();
                    std::thread::Builder::new()
                        .name("qbzd-sse".into())
                        .spawn(move || sse::stream(req, rx))
                        .ok();
                    continue;
                }
                let resp = route(&state, &mut req);
                let _ = req.respond(resp);
            }
        })
        .expect("failed to spawn qbzd api thread");
    ApiHandle {
        server: srv_handle,
        thread: Some(thread),
    }
}

fn route(state: &ApiState, req: &mut Request) -> Response<Cursor<Vec<u8>>> {
    let method = req.method().as_str().to_owned();
    let url = req.url().to_owned();
    let mut url_parts = url.splitn(2, '?');
    let path = url_parts.next().unwrap_or("").to_owned();
    let query = url_parts.next().unwrap_or("").to_owned();
    let has_origin = req.headers().iter().any(|h| h.field.equiv("Origin"));
    let auth_header = req
        .headers()
        .iter()
        .find(|h| h.field.equiv("Authorization"))
        .map(|h| h.value.as_str());

    // Origin shield (ALWAYS on) + opt-in [server] token — one pre-routing gate,
    // /api/ping exempt from the token only (§3.1.2).
    if let Some(reject) = access_gate(has_origin, &method, &path, auth_header, state.token.as_deref())
    {
        return reject.response();
    }

    match (method.as_str(), path.as_str()) {
        ("GET", "/api/ping") => json(
            200,
            serde_json::json!({"ok": true, "app": "qbzd", "api_version": crate::API_VERSION}),
        ),
        ("GET", "/api/info") => status::info(state),
        ("GET", "/api/status") => status::status(state),
        ("GET", "/api/now-playing") => playback::now_playing(state),
        ("POST", "/api/playback/play") => playback::play(state),
        ("POST", "/api/playback/pause") => playback::pause(state),
        ("POST", "/api/playback/toggle") => playback::toggle(state),
        ("POST", "/api/playback/stop") => playback::stop(state),
        ("POST", "/api/playback/next") => playback::next(state),
        ("POST", "/api/playback/previous") => playback::previous(state),
        ("POST", "/api/playback/seek") => {
            let body = read_json_body(req);
            playback::seek(state, &body)
        }
        ("POST", "/api/playback/volume") => {
            let body = read_json_body(req);
            playback::volume(state, &body)
        }
        ("POST", "/api/playback/shuffle") => {
            let body = read_json_body(req);
            playback::shuffle(state, &body)
        }
        ("POST", "/api/playback/repeat") => {
            let body = read_json_body(req);
            playback::repeat(state, &body)
        }
        ("GET", "/api/search") => search::search(state, &query),
        ("POST", "/api/play") => {
            let body = read_json_body(req);
            play::play(state, &body)
        }
        ("GET", "/api/album") => browse::album(state, &query),
        ("GET", "/api/artist") => browse::artist(state, &query),
        ("GET", "/api/similar") => browse::similar(state, &query),
        ("GET", "/api/suggest") => browse::suggest(state, &query),
        ("GET", "/api/discover") => discover::discover(state, &query),
        ("GET", "/api/lyrics") => lyrics::lyrics(state, &query),
        ("GET", "/api/artwork/current") => artwork::current(state),
        ("POST", "/api/radio") => {
            let body = read_json_body(req);
            radio::radio(state, &body)
        }
        ("POST", "/api/reco/playlist") => {
            let body = read_json_body(req);
            reco::playlist(state, &body)
        }
        ("GET", "/api/favorites") => fav::list(state, &query),
        ("POST", "/api/favorites/add") => {
            let body = read_json_body(req);
            fav::add(state, &body)
        }
        ("POST", "/api/favorites/remove") => {
            let body = read_json_body(req);
            fav::remove(state, &body)
        }
        ("GET", "/api/playlists") => playlist::list(state),
        ("GET", "/api/playlist") => playlist::show(state, &query),
        ("POST", "/api/playlist/create") => {
            let body = read_json_body(req);
            playlist::create(state, &body)
        }
        ("POST", "/api/playlist/update") => {
            let body = read_json_body(req);
            playlist::update(state, &body)
        }
        ("POST", "/api/playlist/delete") => {
            let body = read_json_body(req);
            playlist::delete(state, &body)
        }
        ("POST", "/api/playlist/tracks/add") => {
            let body = read_json_body(req);
            playlist::tracks_add(state, &body)
        }
        ("POST", "/api/playlist/tracks/remove") => {
            let body = read_json_body(req);
            playlist::tracks_remove(state, &body)
        }
        ("GET", "/api/queue") => queue::list(state, &query),
        ("POST", "/api/queue/add") => {
            let body = read_json_body(req);
            queue::add(state, &body)
        }
        ("POST", "/api/queue/remove") => {
            let body = read_json_body(req);
            queue::remove(state, &body)
        }
        ("POST", "/api/queue/clear") => {
            let body = read_json_body(req);
            queue::clear(state, &body)
        }
        ("POST", "/api/queue/move") => {
            let body = read_json_body(req);
            queue::reorder(state, &body)
        }
        ("POST", "/api/queue/jump") => {
            let body = read_json_body(req);
            queue::jump(state, &body)
        }
        ("POST", "/api/queue/stop-after") => {
            let body = read_json_body(req);
            queue::stop_after(state, &body)
        }
        ("POST", "/api/settings/reload") => settings::reload(state),
        _ => err_json(404, "not_found", "unknown route", "see qbzd --help"),
    }
}

/// Read and parse a request body as JSON (T7's seek/volume POST bodies).
/// An unreadable or absent body parses to `Value::Null` — the route handlers
/// treat a missing expected field as `400 bad_request`, never a panic.
fn read_json_body(req: &mut Request) -> serde_json::Value {
    let mut buf = String::new();
    let _ = req.as_reader().read_to_string(&mut buf);
    serde_json::from_str(&buf).unwrap_or(serde_json::Value::Null)
}

/// A pre-routing rejection. Carried as a small enum so [`access_gate`] stays a
/// pure decision (unit-testable without a tiny_http `Request`, which has no
/// public constructor) while `route` renders the normative envelope.
enum GateReject {
    OriginForbidden,
    InvalidToken,
}

impl GateReject {
    fn response(&self) -> Response<Cursor<Vec<u8>>> {
        match self {
            GateReject::OriginForbidden => err_json(
                403,
                "origin_forbidden",
                "requests with an Origin header are refused",
                "the control plane is not a browser API",
            ),
            GateReject::InvalidToken => err_json(
                401,
                "invalid_token",
                "missing or wrong bearer token",
                "set QBZD_TOKEN or check [server] token in qbzd.toml",
            ),
        }
    }
}

/// The pre-routing access decision (02 §3.1.2): Origin shield always on; the
/// opt-in Bearer required on every route except `GET /api/ping` when `token`
/// is `Some`. `None` = open (no auth machinery). Returns `Some(_)` to reject.
fn access_gate(
    has_origin: bool,
    method: &str,
    path: &str,
    auth_header: Option<&str>,
    token: Option<&str>,
) -> Option<GateReject> {
    if has_origin {
        return Some(GateReject::OriginForbidden);
    }
    if let Some(secret) = token {
        let is_ping = method == "GET" && path == "/api/ping";
        let expected = format!("Bearer {secret}");
        let ok = auth_header
            .map(|v| constant_time_eq(v.as_bytes(), expected.as_bytes()))
            .unwrap_or(false);
        if !is_ping && !ok {
            return Some(GateReject::InvalidToken);
        }
    }
    None
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Canonical JSON number for a volume level (0.0-1.0). `serde_json::Value`
/// backs f32 via `Number::from_f32`, which stores the value as `f as f64` —
/// the f32→f64 widening turns `0.8f32` into `0.800000011920929` on the wire.
/// 02-cli-and-api.md §2.2/§3.3.4 document plain `0.8`, and `--json` is the
/// frozen machine contract ("scripts parse this"), so every volume-bearing
/// response routes through this instead of a bare `json!(v)`. 3 decimals is
/// plenty of precision for a 0.0-1.0 level.
pub(crate) fn canon_volume(v: f32) -> serde_json::Value {
    let rounded = (v as f64 * 1000.0).round() / 1000.0;
    serde_json::json!(rounded)
}

/// A 2xx JSON response. `pub(crate)` so the per-route handlers in `status.rs`
/// share the exact same envelope framing.
pub(crate) fn json(status: u16, body: serde_json::Value) -> Response<Cursor<Vec<u8>>> {
    let bytes = serde_json::to_vec(&body).unwrap_or_else(|_| b"{}".to_vec());
    let header = Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..])
        .expect("static content-type header");
    Response::from_data(bytes)
        .with_status_code(status)
        .with_header(header)
}

/// The normative error envelope (02 §3.1.3): `{"error":{"code","message","hint"}}`.
/// The CLI keys its exit code off `code` (never raw HTTP status), and every hint
/// names the fix (§1.4 error voice). The G0 addendum's shorthand
/// `{"error":"origin_forbidden"}` is this same nested envelope — the uniform
/// §3.1.3 shape the CLI's `error_from_envelope` reads via `error.code`.
pub(crate) fn err_json(
    status: u16,
    code: &str,
    message: &str,
    hint: &str,
) -> Response<Cursor<Vec<u8>>> {
    json(status, error_body(code, message, hint))
}

/// Refuse owner-only queue and track-selection actions while the QConnect
/// authority cell belongs to a delegated runtime (or is fenced during an
/// authority transition). Transport controls that are safe for either origin
/// -- pause/resume/seek/volume -- deliberately do not call this OWNER gate;
/// they use [`transport_action_lease`] so they still drain before a handoff.
pub(crate) fn owner_action_gate(state: &ApiState) -> Option<Response<Cursor<Vec<u8>>>> {
    owner_actions_blocked(state.qconnect_control.as_ref())
        .then(|| json(409, owner_action_conflict_body()))
}

/// Atomically admit one owner-only action. The returned permit must stay alive
/// through every queue/playback mutation and await in that action; authority
/// handoff closes new admission and drains these permits before snapshotting.
/// `None` inside `Ok` is the short boot window before QConnect publishes its
/// control handle, when no delegated runtime can exist yet.
pub(crate) fn owner_action_lease(
    state: &ApiState,
) -> Result<Option<crate::qconnect::authority::AuthorityActionPermit>, Response<Cursor<Vec<u8>>>> {
    let Some(qconnect) = state.qconnect_control.get() else {
        return Ok(None);
    };
    qconnect
        .try_owner_action_permit()
        .map(Some)
        .ok_or_else(|| json(409, owner_action_conflict_body()))
}

/// Atomically admit an origin-agnostic transport action. A stable owner or
/// delegated renderer may be controlled locally, but a transition fence
/// refuses the action so it cannot land on the runtime installed after the
/// handoff. As with owner leases, `None` is only the pre-QConnect boot window.
pub(crate) fn transport_action_lease(
    state: &ApiState,
) -> Result<Option<crate::qconnect::authority::AuthorityActionPermit>, Response<Cursor<Vec<u8>>>> {
    let Some(qconnect) = state.qconnect_control.get() else {
        return Ok(None);
    };
    qconnect
        .try_transport_action_permit()
        .map(Some)
        .ok_or_else(|| json(409, transport_action_conflict_body()))
}

/// Cheap form used by spawn-and-ack handlers immediately before their async
/// owner action starts. An unpublished control means QConnect has not reached
/// daemon boot step 12 yet, so there is no delegated authority to conflict
/// with.
pub(crate) fn owner_actions_blocked(control: &OnceLock<crate::qconnect::QconnectControl>) -> bool {
    control
        .get()
        .is_some_and(|qconnect| !qconnect.owner_actions_allowed())
}

fn owner_action_conflict_body() -> serde_json::Value {
    error_body(
        "qconnect_authority_conflict",
        "the owner queue is unavailable while QConnect controls playback",
        "return playback control to the QBZ owner and try again",
    )
}

fn transport_action_conflict_body() -> serde_json::Value {
    error_body(
        "qconnect_authority_transition",
        "playback control is temporarily unavailable during a QConnect handoff",
        "retry the transport command",
    )
}

pub(crate) fn error_body(code: &str, message: &str, hint: &str) -> serde_json::Value {
    serde_json::json!({"error": {"code": code, "message": message, "hint": hint}})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_table_matches_spec_count() {
        // 02-cli-and-api.md §3.2 — P0 = exactly 17 routes, FINAL; grows ONLY
        // with a shipped client. T1-T6 landed 3; T7 +9 = 12; T8 +4 = 16; T11
        // (this task) +1 = 17.
        assert_eq!(P0_ROUTES.len(), 17);
        assert!(P0_ROUTES.contains(&("GET", "/api/ping")));
        assert!(P0_ROUTES.contains(&("GET", "/api/info")));
        assert!(P0_ROUTES.contains(&("GET", "/api/status")));
        assert!(P0_ROUTES.contains(&("GET", "/api/now-playing")));
        assert!(P0_ROUTES.contains(&("POST", "/api/playback/play")));
        assert!(P0_ROUTES.contains(&("POST", "/api/playback/pause")));
        assert!(P0_ROUTES.contains(&("POST", "/api/playback/toggle")));
        assert!(P0_ROUTES.contains(&("POST", "/api/playback/stop")));
        assert!(P0_ROUTES.contains(&("POST", "/api/playback/next")));
        assert!(P0_ROUTES.contains(&("POST", "/api/playback/previous")));
        assert!(P0_ROUTES.contains(&("POST", "/api/playback/seek")));
        assert!(P0_ROUTES.contains(&("POST", "/api/playback/volume")));
        assert!(P0_ROUTES.contains(&("GET", "/api/queue")));
        assert!(P0_ROUTES.contains(&("POST", "/api/queue/add")));
        assert!(P0_ROUTES.contains(&("POST", "/api/queue/remove")));
        assert!(P0_ROUTES.contains(&("POST", "/api/queue/clear")));
        assert!(P0_ROUTES.contains(&("POST", "/api/settings/reload")));
    }

    #[test]
    fn p1_route_table_grows_only_with_a_shipped_caller() {
        // 02-cli-and-api.md §3.4 — each P1 route lands with its CLI verb (the
        // §3.1.4 HARD RULE, applied to the content-verb door). Row 19:
        // GET /api/search — caller: `qbzd search`. Count is pinned so a route
        // with no caller cannot creep in; P1 must never overlap P0.
        assert_eq!(P1_ROUTES.len(), 27);
        assert!(P1_ROUTES.contains(&("GET", "/api/events"))); // caller: `qbzd watch`
        assert!(P1_ROUTES.contains(&("GET", "/api/artwork/current"))); // caller: `qbzd art`
        assert!(P1_ROUTES.contains(&("GET", "/api/discover")));
        assert!(P1_ROUTES.contains(&("GET", "/api/lyrics")));
        assert!(P1_ROUTES.contains(&("POST", "/api/reco/playlist")));
        assert!(P1_ROUTES.contains(&("GET", "/api/favorites")));
        assert!(P1_ROUTES.contains(&("POST", "/api/favorites/add")));
        assert!(P1_ROUTES.contains(&("POST", "/api/favorites/remove")));
        assert!(P1_ROUTES.contains(&("GET", "/api/playlists")));
        assert!(P1_ROUTES.contains(&("GET", "/api/playlist")));
        assert!(P1_ROUTES.contains(&("POST", "/api/playlist/create")));
        assert!(P1_ROUTES.contains(&("POST", "/api/playlist/update")));
        assert!(P1_ROUTES.contains(&("POST", "/api/playlist/delete")));
        assert!(P1_ROUTES.contains(&("POST", "/api/playlist/tracks/add")));
        assert!(P1_ROUTES.contains(&("POST", "/api/playlist/tracks/remove")));
        assert!(P1_ROUTES.contains(&("GET", "/api/search")));
        assert!(P1_ROUTES.contains(&("POST", "/api/play")));
        assert!(P1_ROUTES.contains(&("GET", "/api/album")));
        assert!(P1_ROUTES.contains(&("GET", "/api/artist")));
        assert!(P1_ROUTES.contains(&("GET", "/api/similar")));
        assert!(P1_ROUTES.contains(&("GET", "/api/suggest")));
        assert!(P1_ROUTES.contains(&("POST", "/api/radio")));
        assert!(P1_ROUTES.contains(&("POST", "/api/playback/shuffle")));
        assert!(P1_ROUTES.contains(&("POST", "/api/playback/repeat")));
        assert!(P1_ROUTES.contains(&("POST", "/api/queue/move")));
        assert!(P1_ROUTES.contains(&("POST", "/api/queue/jump")));
        assert!(P1_ROUTES.contains(&("POST", "/api/queue/stop-after")));
        for r in P1_ROUTES {
            assert!(!P0_ROUTES.contains(r), "{r:?} is duplicated across P0 and P1");
        }
    }

    #[test]
    fn constant_time_eq_matches_only_identical_slices() {
        assert!(constant_time_eq(b"Bearer s3cret", b"Bearer s3cret"));
        assert!(!constant_time_eq(b"Bearer s3cret", b"Bearer wrong"));
        assert!(!constant_time_eq(b"Bearer s3cret", b"Bearer s3cret-extra"));
        assert!(!constant_time_eq(b"", b"x"));
    }

    fn code(r: Option<GateReject>) -> Option<&'static str> {
        r.map(|r| match r {
            GateReject::OriginForbidden => "origin_forbidden",
            GateReject::InvalidToken => "invalid_token",
        })
    }

    #[test]
    fn origin_header_is_refused_on_every_route_including_ping() {
        // Step 4(a): an Origin header → 403 origin_forbidden everywhere, even
        // /api/ping, and even in the open (token=None) default.
        for (m, p) in P0_ROUTES {
            assert_eq!(
                code(access_gate(true, m, p, None, None)),
                Some("origin_forbidden"),
                "{m} {p} with Origin must be refused"
            );
        }
        // ...and the Origin shield wins even when a valid Bearer is present.
        assert_eq!(
            code(access_gate(true, "GET", "/api/ping", Some("Bearer s3cret"), Some("s3cret"))),
            Some("origin_forbidden")
        );
    }

    #[test]
    fn open_mode_answers_every_route_without_auth() {
        // Step 4(b): token=None → no auth machinery; nothing is rejected.
        for (m, p) in P0_ROUTES {
            assert!(access_gate(false, m, p, None, None).is_none(), "{m} {p} must be open");
        }
    }

    #[test]
    fn opt_in_token_rejects_missing_or_wrong_bearer_but_never_ping() {
        // Step 4(c): token=Some → missing/wrong bearer is 401 on non-ping routes;
        // /api/ping stays 200 (exempt); the correct bearer passes.
        let tok = Some("s3cret");
        assert_eq!(code(access_gate(false, "GET", "/api/status", None, tok)), Some("invalid_token"));
        assert_eq!(
            code(access_gate(false, "GET", "/api/status", Some("Bearer nope"), tok)),
            Some("invalid_token")
        );
        assert!(access_gate(false, "GET", "/api/status", Some("Bearer s3cret"), tok).is_none());
        // /api/ping answers even with no/ wrong bearer.
        assert!(access_gate(false, "GET", "/api/ping", None, tok).is_none());
        assert!(access_gate(false, "GET", "/api/ping", Some("Bearer nope"), tok).is_none());
    }

    #[test]
    fn canon_volume_pins_0_8_exactly_no_f32_widening() {
        // `serde_json::Number::from_f32` widens f32→f64 (`0.8f32` would
        // serialize raw as `0.800000011920929`); `canon_volume` must not.
        assert_eq!(serde_json::to_string(&canon_volume(0.8f32)).unwrap(), "0.8");
        assert_eq!(serde_json::to_string(&canon_volume(1.0f32)).unwrap(), "1.0");
        assert_eq!(serde_json::to_string(&canon_volume(0.0f32)).unwrap(), "0.0");
        assert_eq!(serde_json::to_string(&canon_volume(0.75f32)).unwrap(), "0.75");
    }

    #[test]
    fn error_envelope_is_the_nested_shape_with_code() {
        // 02 §3.1.3 — the on-the-wire shape the CLI reads via `error.code`.
        let body = error_body("origin_forbidden", "refused", "not a browser API");
        assert_eq!(body["error"]["code"], "origin_forbidden");
        assert_eq!(body["error"]["message"], "refused");
        assert_eq!(body["error"]["hint"], "not a browser API");
    }

    #[test]
    fn owner_action_conflict_is_stable_and_contains_no_handoff_details() {
        let body = owner_action_conflict_body();
        assert_eq!(body["error"]["code"], "qconnect_authority_conflict");
        assert_eq!(
            body["error"]["message"],
            "the owner queue is unavailable while QConnect controls playback"
        );

        let encoded = serde_json::to_string(&body).unwrap();
        for secret_shape in [
            "Authorization",
            "Bearer",
            "jwt",
            "session_id",
            "endpoint",
            "http://",
            "https://",
            "?",
        ] {
            assert!(
                !encoded.contains(secret_shape),
                "conflict response reflected sensitive shape {secret_shape}"
            );
        }
    }

    #[test]
    fn transport_transition_conflict_is_retryable_and_origin_agnostic() {
        let body = transport_action_conflict_body();
        assert_eq!(body["error"]["code"], "qconnect_authority_transition");
        assert_eq!(body["error"]["hint"], "retry the transport command");
        let message = body["error"]["message"].as_str().unwrap();
        assert!(message.contains("handoff"));
        assert!(!message.contains("owner"));
        assert!(!message.contains("delegated"));
    }
}
