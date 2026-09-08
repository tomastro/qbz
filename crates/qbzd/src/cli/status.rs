// crates/qbzd/src/cli/status.rs — the `status` and `ping` verbs (02 §2.2).
//
// Both render an already-parsed API payload; neither holds state. `status` also
// runs the version-skew check (§1.6, from the /api/status payload — it carries
// `version` + `api_version`, so it needs no /api/info fallback) and, on the
// daemon box, the linger check (§1.4). Exit codes come from the frozen table
// (§1.3): 0 healthy · 3 unreachable · 4 needs_auth · 5 device unopenable.
use serde_json::Value;

use crate::cli::client::ApiClient;
use crate::cli::copy;
use crate::paths::ProfileRoots;

/// `qbzd ping` — liveness. Human `pong`; `--json` the raw body. Exit 0 · 3.
pub async fn ping(host: Option<String>, json: bool, roots: &ProfileRoots) -> i32 {
    let client = ApiClient::new(host, roots);
    match client.get("/api/ping").await {
        Ok(v) => {
            if json {
                println!("{}", serde_json::to_string(&v).unwrap_or_default());
            } else {
                println!("pong");
            }
            0
        }
        Err(e) => {
            eprintln!("{e}");
            e.exit_code()
        }
    }
}

/// `qbzd status` — THE diagnostic. Human composite block; `--json` raw payload.
/// Exit 0 healthy · 3 unreachable · 4 needs_auth · 5 device unopenable.
pub async fn status(host: Option<String>, json: bool, roots: &ProfileRoots) -> i32 {
    let client = ApiClient::new(host, roots);
    let payload = match client.get("/api/status").await {
        Ok(v) => v,
        Err(e) => {
            eprintln!("{e}");
            return e.exit_code();
        }
    };

    // Version skew (§1.6): breaking api_version mismatch refuses; a semver-only
    // mismatch is a warning that does not stop the render.
    let daemon_api = payload.get("api_version").and_then(|a| a.as_u64()).unwrap_or(0) as u32;
    if daemon_api != crate::API_VERSION {
        eprintln!("{}", copy::api_version_skew(daemon_api, crate::API_VERSION));
        return 1;
    }
    let cli_ver = env!("CARGO_PKG_VERSION");
    if let Some(daemon_ver) = payload.get("version").and_then(|v| v.as_str()) {
        if !daemon_ver.is_empty() && daemon_ver != cli_ver {
            eprintln!("{}", copy::version_skew(daemon_ver, cli_ver));
        }
    }

    if json {
        println!("{}", serde_json::to_string(&payload).unwrap_or_default());
    } else {
        print!("{}", render(&payload, client.host()));
    }

    // Linger check on the daemon box only (§1.4) — a warning, never fatal.
    if client.is_local() {
        if let Some(w) = linger_warning() {
            eprintln!("{w}");
        }
    }

    exit_from_state(&payload)
}

/// 4 needs_auth · 5 configured device not present · else 0 (§1.3). Auth gates
/// before device: a login is the more common fix.
fn exit_from_state(p: &Value) -> i32 {
    let auth = str_at(p, &["auth", "state"]);
    if auth == "needs_auth" {
        return 4;
    }
    let configured = p
        .pointer("/audio/configured_device")
        .map(|v| !v.is_null())
        .unwrap_or(false);
    let present = p
        .pointer("/audio/device_present")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    if configured && !present {
        return 5;
    }
    0
}

/// The §2.2 composite block. `host` is the target (the payload has no `bind`).
fn render(p: &Value, host: &str) -> String {
    let version = str_at(p, &["version"]);
    let api = p.get("api_version").and_then(|a| a.as_u64()).unwrap_or(0);
    let uptime = fmt_uptime(p.get("uptime_secs").and_then(|u| u.as_u64()).unwrap_or(0));
    let data_root = str_at(p, &["data_root"]);

    let mut out = String::new();
    out.push_str(&format!(
        "qbzd {version} · api v{api} · up {uptime} · {host} · data {data_root}\n"
    ));
    out.push_str(&format!("auth      : {}\n", render_auth(p)));
    out.push_str(&format!("audio     : {}\n", render_audio(p)));
    out.push_str(&format!("playback  : {}\n", render_playback(p)));
    out.push_str(&format!("qconnect  : {}\n", render_qconnect(p)));
    out.push_str(&format!(
        "network   : {}\n",
        if p.pointer("/network/online").and_then(|v| v.as_bool()).unwrap_or(false) {
            "online"
        } else {
            "offline"
        }
    ));
    out.push_str(&format!("last error: {}\n", render_last_error(p)));
    out
}

fn render_auth(p: &Value) -> String {
    match str_at(p, &["auth", "state"]).as_str() {
        "logged_in" => {
            let user = p.pointer("/auth/user_id").and_then(|v| v.as_u64());
            let sub = p.pointer("/auth/subscription").and_then(|v| v.as_str());
            match (user, sub) {
                (Some(u), Some(s)) => format!("logged in (user {u}, {s})"),
                (Some(u), None) => format!("logged in (user {u})"),
                _ => "logged in".to_string(),
            }
        }
        "restoring" => "restoring session…".to_string(),
        _ => "not logged in".to_string(),
    }
}

fn render_audio(p: &Value) -> String {
    let backend = p.pointer("/audio/backend").and_then(|v| v.as_str());
    let device = p.pointer("/audio/configured_device").and_then(|v| v.as_str());
    let present = p.pointer("/audio/device_present").and_then(|v| v.as_bool()).unwrap_or(false);
    let bit_perfect = p.pointer("/audio/bit_perfect").and_then(|v| v.as_str());
    let sr = p.pointer("/audio/sample_rate").and_then(|v| v.as_u64());
    let bd = p.pointer("/audio/bit_depth").and_then(|v| v.as_u64());

    let mut parts: Vec<String> = Vec::new();
    let head = match (backend, device) {
        (Some(b), Some(d)) => format!("{b} {d}"),
        (Some(b), None) => format!("{b} (system default)"),
        (None, Some(d)) => d.to_string(),
        (None, None) => "system default".to_string(),
    };
    parts.push(head);
    parts.push(if present { "present".into() } else { "not present".into() });
    if let Some(bp) = bit_perfect {
        parts.push(format!("bit-perfect: {bp}"));
    }
    if let (Some(sr), Some(bd)) = (sr, bd) {
        parts.push(format!("{sr} Hz / {bd}-bit"));
    }
    if p.pointer("/audio/hardware_volume_enabled")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        let active = p
            .pointer("/audio/hardware_volume_active")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let control = p
            .pointer("/audio/hardware_volume_control")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown control");
        parts.push(format!(
            "hardware volume: {control} ({})",
            if active { "active" } else { "inactive" }
        ));
    }
    parts.join(" · ")
}

fn render_playback(p: &Value) -> String {
    let state = str_at(p, &["playback", "state"]);
    let queue = p.pointer("/playback/queue_len").and_then(|v| v.as_u64()).unwrap_or(0);
    if state == "stopped" {
        return format!("stopped · queue {queue}");
    }
    let title = p.pointer("/playback/title").and_then(|v| v.as_str());
    let artist = p.pointer("/playback/artist").and_then(|v| v.as_str());
    let pos = p.pointer("/playback/position").and_then(|v| v.as_u64()).unwrap_or(0);
    let dur = p.pointer("/playback/duration").and_then(|v| v.as_u64()).unwrap_or(0);
    let vol = p.pointer("/playback/volume").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let muted = p.pointer("/playback/muted").and_then(|v| v.as_bool()).unwrap_or(false);

    let track = match (title, artist) {
        (Some(t), Some(a)) => format!("\"{t}\" — {a}"),
        (Some(t), None) => format!("\"{t}\""),
        _ => "(unknown track)".to_string(),
    };
    let vol_str = if muted {
        "muted".to_string()
    } else {
        format!("vol {}%", (vol * 100.0).round() as i64)
    };
    format!(
        "{state} · {track} · {} / {} · {vol_str} · queue {queue}",
        fmt_mmss(pos),
        fmt_mmss(dur)
    )
}

fn render_qconnect(p: &Value) -> String {
    let enabled = p.pointer("/qconnect/enabled").and_then(|v| v.as_bool()).unwrap_or(false);
    if !enabled {
        return "off".to_string();
    }
    let state = p.pointer("/qconnect/state").and_then(|v| v.as_str()).unwrap_or("");
    let session = p.pointer("/qconnect/session_active").and_then(|v| v.as_bool()).unwrap_or(false);
    let name = p.pointer("/qconnect/device_name").and_then(|v| v.as_str()).unwrap_or("");
    let mut parts = vec![if state.is_empty() { "enabled".to_string() } else { state.to_string() }];
    if session {
        parts.push("session active".to_string());
    }
    if !name.is_empty() {
        parts.push(format!("name \"{name}\""));
    }
    parts.join(" · ")
}

fn render_last_error(p: &Value) -> String {
    for key in ["stream", "auth", "transport"] {
        if let Some(m) = p.pointer(&format!("/last_errors/{key}")).and_then(|v| v.as_str()) {
            if !m.is_empty() {
                return format!("{key}: {m}");
            }
        }
    }
    "none".to_string()
}

/// `loginctl show-user $USER -p Linger` → the §1.4 linger warning on `Linger=no`.
/// Any failure (no loginctl, no session) → no warning.
fn linger_warning() -> Option<String> {
    let user = std::env::var("USER").ok().filter(|u| !u.is_empty())?;
    let out = std::process::Command::new("loginctl")
        .args(["show-user", &user, "-p", "Linger"])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    if text.trim() == "Linger=no" {
        Some(copy::linger_off(&user))
    } else {
        None
    }
}

fn str_at(p: &Value, path: &[&str]) -> String {
    let mut cur = p;
    for k in path {
        match cur.get(k) {
            Some(v) => cur = v,
            None => return String::new(),
        }
    }
    cur.as_str().unwrap_or("").to_string()
}

fn fmt_mmss(secs: u64) -> String {
    format!("{}:{:02}", secs / 60, secs % 60)
}

fn fmt_uptime(secs: u64) -> String {
    let days = secs / 86_400;
    let hours = (secs % 86_400) / 3_600;
    let mins = (secs % 3_600) / 60;
    if days > 0 {
        format!("{days}d {hours}h")
    } else if hours > 0 {
        format!("{hours}h {mins}m")
    } else {
        format!("{mins}m")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn logged_in_payload() -> Value {
        serde_json::json!({
            "version": "2.1.0", "api_version": 1, "uptime_secs": 259_200,
            "data_root": "/home/pi/.local/share/qbzd", "driver_tick_age_ms": 210,
            "auth": {"state": "logged_in", "user_id": 1234567, "subscription": "studio"},
            "audio": {"backend": "alsa", "configured_device": "hw:CARD=D30,DEV=0",
                      "device_present": true, "device_open": true,
                      "bit_perfect": "DirectHardware", "sample_rate": 192000, "bit_depth": 24,
                      "hardware_volume_enabled": true, "hardware_volume_active": true,
                      "hardware_volume_control": "D50 III,0"},
            "playback": {"state": "playing", "track_id": 176544871, "title": "Spain",
                         "artist": "Chick Corea", "position": 192, "duration": 581,
                         "volume": 0.8, "muted": false, "queue_len": 14},
            "qconnect": {"enabled": true, "state": "connected", "device_name": "QBZ (kitchen-pi)",
                         "session_active": true, "last_transport_reconnect": null},
            "network": {"online": true},
            "last_errors": {"stream": null, "auth": null, "transport": null}
        })
    }

    #[test]
    fn healthy_status_exits_zero() {
        assert_eq!(exit_from_state(&logged_in_payload()), 0);
    }

    #[test]
    fn needs_auth_exits_four() {
        let mut p = logged_in_payload();
        p["auth"]["state"] = serde_json::json!("needs_auth");
        assert_eq!(exit_from_state(&p), 4);
    }

    #[test]
    fn configured_but_absent_device_exits_five() {
        let mut p = logged_in_payload();
        p["audio"]["device_present"] = serde_json::json!(false);
        assert_eq!(exit_from_state(&p), 5);
        // system default (no configured device) never trips exit 5.
        let mut sysdef = logged_in_payload();
        sysdef["audio"]["configured_device"] = serde_json::Value::Null;
        sysdef["audio"]["device_present"] = serde_json::json!(false);
        assert_eq!(exit_from_state(&sysdef), 0);
    }

    #[test]
    fn render_covers_the_composite_block() {
        let block = render(&logged_in_payload(), "127.0.0.1:8182");
        assert!(block.contains("qbzd 2.1.0 · api v1 · up 3d 0h · 127.0.0.1:8182"), "{block}");
        assert!(block.contains("auth      : logged in (user 1234567, studio)"), "{block}");
        assert!(block.contains("alsa hw:CARD=D30,DEV=0 · present · bit-perfect: DirectHardware · 192000 Hz / 24-bit"), "{block}");
        assert!(block.contains("hardware volume: D50 III,0 (active)"), "{block}");
        assert!(block.contains("playback  : playing · \"Spain\" — Chick Corea · 3:12 / 9:41 · vol 80% · queue 14"), "{block}");
        assert!(block.contains("qconnect  : connected · session active · name \"QBZ (kitchen-pi)\""), "{block}");
        assert!(block.contains("last error: none"), "{block}");
    }

    #[test]
    fn stopped_playback_renders_queue_only() {
        let mut p = logged_in_payload();
        p["playback"]["state"] = serde_json::json!("stopped");
        let line = render_playback(&p);
        assert_eq!(line, "stopped · queue 14");
    }
}
