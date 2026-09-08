// crates/qbzd/src/cli/transport.rs — the 10 transport verbs (02-cli-and-api.md
// §2.2): `now play pause toggle stop next prev seek volume mute`. Each
// networked verb is exactly one HTTP request (§1.1); the pure argument
// parsers/renderers below are unit-tested without a running daemon.
use serde_json::Value;

use crate::cli::client::ApiClient;
use crate::paths::ProfileRoots;

// ============================ `now` ============================

/// `qbzd now [--json]` — renders `GET /api/now-playing` (§2.2/§3.3.4).
/// Exit: 0 · 3 · 4 (via `ApiClient`/`error_from_envelope`, unchanged here).
pub async fn now(host: Option<String>, json: bool, roots: &ProfileRoots) -> i32 {
    let client = ApiClient::new(host, roots);
    match client.get("/api/now-playing").await {
        Ok(v) => {
            if json {
                println!("{}", serde_json::to_string(&v).unwrap_or_default());
            } else {
                println!("{}", render_now(&v));
            }
            0
        }
        Err(e) => {
            eprintln!("{e}");
            e.exit_code()
        }
    }
}

/// `playing · Chick Corea – Spain · 3:12/9:41 · 96kHz/24bit · vol 80%` /
/// `stopped · queue 14 tracks` (02 §2.2, verbatim shape).
fn render_now(v: &Value) -> String {
    let track = v.get("track").filter(|t| !t.is_null());
    let Some(track) = track else {
        let queue_len = v.pointer("/playback/queue_len").and_then(|n| n.as_u64()).unwrap_or(0);
        return format!("stopped · queue {queue_len} tracks");
    };

    let is_playing = v.pointer("/playback/is_playing").and_then(|b| b.as_bool()).unwrap_or(false);
    let state = if is_playing { "playing" } else { "paused" };
    let artist = track.get("artist").and_then(|a| a.as_str()).unwrap_or("");
    let title = track.get("title").and_then(|a| a.as_str()).unwrap_or("");
    let pos = v.pointer("/playback/position").and_then(|p| p.as_u64()).unwrap_or(0);
    let dur = v.pointer("/playback/duration").and_then(|p| p.as_u64()).unwrap_or(0);
    let vol = v.pointer("/playback/volume").and_then(|p| p.as_f64()).unwrap_or(0.0);
    let sr = v.pointer("/playback/sample_rate").and_then(|p| p.as_u64());
    let bd = v.pointer("/playback/bit_depth").and_then(|p| p.as_u64());

    let mut parts = vec![
        state.to_string(),
        format!("{artist} – {title}"),
        format!("{}/{}", fmt_mmss(pos), fmt_mmss(dur)),
    ];
    if let (Some(sr), Some(bd)) = (sr, bd) {
        parts.push(format!("{}/{bd}bit", fmt_khz(sr)));
    }
    parts.push(format!("vol {}%", fraction_to_pct(vol)));
    parts.join(" · ")
}

// ============================ play/pause/toggle/stop ============================

/// `POST` to `path` with no body, printing the response's `state` field
/// (`play`/`pause`/`toggle`/`stop` share this exact shape — 02 §3.3.5-8).
async fn transport_state(host: Option<String>, roots: &ProfileRoots, path: &str) -> i32 {
    let client = ApiClient::new(host, roots);
    match client.post(path, serde_json::json!({})).await {
        Ok(v) => {
            let state = v.get("state").and_then(|s| s.as_str()).unwrap_or("");
            println!("{state}");
            0
        }
        Err(e) => {
            eprintln!("{e}");
            e.exit_code()
        }
    }
}

pub async fn play(host: Option<String>, roots: &ProfileRoots) -> i32 {
    transport_state(host, roots, "/api/playback/play").await
}

pub async fn pause(host: Option<String>, roots: &ProfileRoots) -> i32 {
    transport_state(host, roots, "/api/playback/pause").await
}

pub async fn toggle(host: Option<String>, roots: &ProfileRoots) -> i32 {
    transport_state(host, roots, "/api/playback/toggle").await
}

pub async fn stop(host: Option<String>, roots: &ProfileRoots) -> i32 {
    transport_state(host, roots, "/api/playback/stop").await
}

// ============================ next/prev ============================

/// `POST` to `path` with no body; the response is the landing `QueueTrack` or
/// `null` at queue end (02 §3.3.9-10, legacy shape).
async fn transport_advance(host: Option<String>, roots: &ProfileRoots, path: &str) -> i32 {
    let client = ApiClient::new(host, roots);
    match client.post(path, serde_json::json!({})).await {
        Ok(v) => {
            println!("{}", render_advance(&v));
            0
        }
        Err(e) => {
            eprintln!("{e}");
            e.exit_code()
        }
    }
}

/// `-> Chick Corea – 500 Miles High` / `queue finished` (02 §2.2, verbatim) —
/// or `-> queued (next)` for the spawn-and-ack advance response (the ritual
/// runs async; follow the landing track via `qbzd now`).
fn render_advance(v: &Value) -> String {
    if v.is_null() {
        return "queue finished".to_string();
    }
    if v.get("queued").and_then(|q| q.as_bool()).unwrap_or(false) {
        let direction = v.get("direction").and_then(|d| d.as_str()).unwrap_or("next");
        return format!("-> queued ({direction})");
    }
    let artist = v.get("artist").and_then(|s| s.as_str()).unwrap_or("");
    let title = v.get("title").and_then(|s| s.as_str()).unwrap_or("");
    format!("-> {artist} – {title}")
}

pub async fn next(host: Option<String>, roots: &ProfileRoots) -> i32 {
    transport_advance(host, roots, "/api/playback/next").await
}

pub async fn prev(host: Option<String>, roots: &ProfileRoots) -> i32 {
    transport_advance(host, roots, "/api/playback/previous").await
}

// ============================ seek ============================

/// A parsed `qbzd seek` argument (02 §2.2: absolute seconds, `+N`/`-N`
/// relative, or `mm:ss`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeekArg {
    Absolute(u64),
    Delta(i64),
}

/// `90` -> absolute seconds · `+30`/`-10` -> relative seconds · `1:23` ->
/// absolute seconds (mm:ss). Usage errors (exit 2) name what was expected.
pub fn parse_seek_arg(s: &str) -> Result<SeekArg, String> {
    let s = s.trim();
    if let Some(rest) = s.strip_prefix('+') {
        return rest
            .parse::<i64>()
            .map(SeekArg::Delta)
            .map_err(|_| format!("invalid seek offset '{s}' — expected +N seconds"));
    }
    if let Some(rest) = s.strip_prefix('-') {
        return rest
            .parse::<i64>()
            .map(|n| SeekArg::Delta(-n))
            .map_err(|_| format!("invalid seek offset '{s}' — expected -N seconds"));
    }
    if let Some((mm, ss)) = s.split_once(':') {
        let mm: u64 = mm
            .parse()
            .map_err(|_| format!("invalid seek position '{s}' — expected mm:ss"))?;
        let ss: u64 = ss
            .parse()
            .map_err(|_| format!("invalid seek position '{s}' — expected mm:ss"))?;
        if ss >= 60 {
            return Err(format!("invalid seek position '{s}' — seconds must be 0-59"));
        }
        return Ok(SeekArg::Absolute(mm * 60 + ss));
    }
    s.parse::<u64>()
        .map(SeekArg::Absolute)
        .map_err(|_| format!("invalid seek position '{s}' — expected seconds, +N, -N, or mm:ss"))
}

/// `SeekArg` -> the `POST /api/playback/seek` body (02 §3.3.11): `{"position"}`
/// (absolute, legacy field name) or `{"delta"}` (additive).
pub fn seek_body(arg: SeekArg) -> Value {
    match arg {
        SeekArg::Absolute(n) => serde_json::json!({"position": n}),
        SeekArg::Delta(n) => serde_json::json!({"delta": n}),
    }
}

/// `qbzd seek <POS|+N|-N|mm:ss>` — human `at 1:30 / 9:41` (02 §2.2, verbatim
/// — note the spaced slash, unlike `now`'s unspaced `3:12/9:41`). Exit:
/// 0 · 1 · 2 (local parse failure) · 3 · 5.
pub async fn seek(host: Option<String>, roots: &ProfileRoots, arg: String) -> i32 {
    let parsed = match parse_seek_arg(&arg) {
        Ok(a) => a,
        Err(msg) => {
            eprintln!("error: {msg}");
            return 2;
        }
    };
    let client = ApiClient::new(host, roots);
    match client.post("/api/playback/seek", seek_body(parsed)).await {
        Ok(v) => {
            let pos = v.get("position").and_then(|p| p.as_u64()).unwrap_or(0);
            let dur = v.get("duration").and_then(|d| d.as_u64()).unwrap_or(0);
            println!("at {} / {}", fmt_mmss(pos), fmt_mmss(dur));
            0
        }
        Err(e) => {
            eprintln!("{e}");
            e.exit_code()
        }
    }
}

// ============================ volume ============================

/// A parsed `qbzd volume` argument (02 §2.2: 0-100 absolute, or `+N`/`-N`
/// relative — both in CLI percent-space).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VolumeArg {
    Absolute(u8),
    Delta(i32),
}

/// `80` -> absolute 0-100 · `+5`/`-5` -> relative percent.
pub fn parse_volume_arg(s: &str) -> Result<VolumeArg, String> {
    let s = s.trim();
    if let Some(rest) = s.strip_prefix('+') {
        return rest
            .parse::<i32>()
            .map(VolumeArg::Delta)
            .map_err(|_| format!("invalid volume offset '{s}' — expected +N"));
    }
    if let Some(rest) = s.strip_prefix('-') {
        return rest
            .parse::<i32>()
            .map(|n| VolumeArg::Delta(-n))
            .map_err(|_| format!("invalid volume offset '{s}' — expected -N"));
    }
    let n: u8 = s
        .parse()
        .map_err(|_| format!("invalid volume '{s}' — expected 0-100, +N, or -N"))?;
    if n > 100 {
        return Err(format!("invalid volume '{s}' — must be 0-100"));
    }
    Ok(VolumeArg::Absolute(n))
}

/// CLI 0-100 <-> API 0.0-1.0 (02 §2.2: "CLI speaks 0-100; the API speaks
/// 0.0-1.0 — legacy contract"). `f64` throughout — JSON numbers ARE f64
/// (`serde_json::Number`), and computing in f32 then widening to build the
/// request body round-trips imprecisely (`0.8f32 as f64` != the `0.8` JSON
/// literal); the eventual `as f32` narrowing happens once, server-side,
/// right before the `Player::set_volume` call.
pub fn pct_to_fraction(pct: u8) -> f64 {
    (pct as f64 / 100.0).clamp(0.0, 1.0)
}

pub fn fraction_to_pct(frac: f64) -> i64 {
    (frac.clamp(0.0, 1.0) * 100.0).round() as i64
}

/// `VolumeArg` -> the `POST /api/playback/volume` body: `{"volume"}`
/// (absolute fraction, legacy field name) or `{"delta"}` (additive fraction —
/// the CLI's `+N`/`-N` percent converted to the API's 0.0-1.0 space).
pub fn volume_body(arg: VolumeArg) -> Value {
    match arg {
        VolumeArg::Absolute(pct) => serde_json::json!({"volume": pct_to_fraction(pct)}),
        VolumeArg::Delta(pct) => serde_json::json!({"delta": pct as f64 / 100.0}),
    }
}

/// `qbzd volume [<0-100>|+N|-N] [--json]`. Bare = read via `GET
/// /api/now-playing`, extracting `{volume, muted}` (no dedicated read route,
/// 02 §2.2). With an argument: `POST /api/playback/volume`. Exit:
/// 0 · 1 · 2 (local parse failure) · 3 · 5.
pub async fn volume(host: Option<String>, roots: &ProfileRoots, value: Option<String>, json: bool) -> i32 {
    let client = ApiClient::new(host, roots);
    match value {
        None => match client.get("/api/now-playing").await {
            Ok(v) => {
                let vol = v.pointer("/playback/volume").and_then(|x| x.as_f64()).unwrap_or(0.0);
                let muted = v.pointer("/playback/muted").and_then(|x| x.as_bool()).unwrap_or(false);
                if json {
                    let out = serde_json::json!({"volume": vol, "muted": muted});
                    println!("{}", serde_json::to_string(&out).unwrap_or_default());
                } else {
                    let pct = fraction_to_pct(vol);
                    if muted {
                        println!("vol {pct}% (muted)");
                    } else {
                        println!("vol {pct}%");
                    }
                }
                0
            }
            Err(e) => {
                eprintln!("{e}");
                e.exit_code()
            }
        },
        Some(arg) => {
            let parsed = match parse_volume_arg(&arg) {
                Ok(a) => a,
                Err(msg) => {
                    eprintln!("error: {msg}");
                    return 2;
                }
            };
            match client.post("/api/playback/volume", volume_body(parsed)).await {
                Ok(v) => {
                    let vol = v.get("volume").and_then(|x| x.as_f64()).unwrap_or(0.0);
                    println!("vol {}%", fraction_to_pct(vol));
                    0
                }
                Err(e) => {
                    eprintln!("{e}");
                    e.exit_code()
                }
            }
        }
    }
}

// ============================ mute ============================

/// `None` (bare = toggle) / `Some("on")` / `Some("off")` -> the
/// `{"mute": "on"|"off"|"toggle"}` body form (02 §2.2/§3.3.12 — "same route
/// as volume, costs no extra route").
pub fn mute_body(arg: Option<&str>) -> Result<Value, String> {
    match arg {
        None => Ok(serde_json::json!({"mute": "toggle"})),
        Some("on") => Ok(serde_json::json!({"mute": "on"})),
        Some("off") => Ok(serde_json::json!({"mute": "off"})),
        Some(other) => Err(format!("invalid mute state '{other}' — use on or off")),
    }
}

/// `qbzd mute [on|off]` — human `muted (was 80%)` / `unmuted · vol 80%`
/// (02 §2.2, verbatim). Exit: 0 · 1 · 2 (local parse failure) · 3 · 5.
pub async fn mute(host: Option<String>, roots: &ProfileRoots, state_arg: Option<String>) -> i32 {
    let body = match mute_body(state_arg.as_deref()) {
        Ok(b) => b,
        Err(msg) => {
            eprintln!("error: {msg}");
            return 2;
        }
    };
    let client = ApiClient::new(host, roots);
    match client.post("/api/playback/volume", body).await {
        Ok(v) => {
            let vol = v.get("volume").and_then(|x| x.as_f64()).unwrap_or(0.0);
            let muted = v.get("muted").and_then(|x| x.as_bool()).unwrap_or(false);
            let pct = fraction_to_pct(vol);
            if muted {
                println!("muted (was {pct}%)");
            } else {
                println!("unmuted · vol {pct}%");
            }
            0
        }
        Err(e) => {
            eprintln!("{e}");
            e.exit_code()
        }
    }
}

// ============================ shared rendering ============================

fn fmt_mmss(secs: u64) -> String {
    format!("{}:{:02}", secs / 60, secs % 60)
}

/// `96000` Hz -> `"96kHz"`; `44100` Hz -> `"44.1kHz"` — the `now` human line's
/// own Hz->kHz rendering choice (independent of the documented Hz-vs-kHz JSON
/// quirk between `playback.sample_rate` and `track.sample_rate`, which is
/// left as-is on the wire, 02 §2.2).
fn fmt_khz(hz: u64) -> String {
    if hz % 1000 == 0 {
        format!("{}kHz", hz / 1000)
    } else {
        format!("{:.1}kHz", hz as f64 / 1000.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seek_arg_parses_absolute_relative_and_mmss() {
        assert_eq!(parse_seek_arg("90"), Ok(SeekArg::Absolute(90)));
        assert_eq!(parse_seek_arg("+30"), Ok(SeekArg::Delta(30)));
        assert_eq!(parse_seek_arg("-10"), Ok(SeekArg::Delta(-10)));
        assert_eq!(parse_seek_arg("1:23"), Ok(SeekArg::Absolute(83)));
        assert!(parse_seek_arg("1:99").is_err());
        assert!(parse_seek_arg("nonsense").is_err());
    }

    #[test]
    fn seek_body_maps_to_legacy_position_or_additive_delta() {
        assert_eq!(seek_body(SeekArg::Absolute(90)), serde_json::json!({"position": 90}));
        assert_eq!(seek_body(SeekArg::Delta(-10)), serde_json::json!({"delta": -10}));
    }

    #[test]
    fn volume_arg_parses_absolute_and_relative() {
        assert_eq!(parse_volume_arg("80"), Ok(VolumeArg::Absolute(80)));
        assert_eq!(parse_volume_arg("+5"), Ok(VolumeArg::Delta(5)));
        assert_eq!(parse_volume_arg("-5"), Ok(VolumeArg::Delta(-5)));
        assert!(parse_volume_arg("101").is_err());
        assert!(parse_volume_arg("nonsense").is_err());
    }

    #[test]
    fn cli_percent_and_api_fraction_convert_both_ways() {
        // 02 §2.2 — "CLI speaks 0-100; the API speaks 0.0-1.0".
        assert_eq!(pct_to_fraction(80), 0.8);
        assert_eq!(fraction_to_pct(0.8), 80);
        assert_eq!(fraction_to_pct(0.75), 75);
        assert_eq!(pct_to_fraction(0), 0.0);
        assert_eq!(pct_to_fraction(100), 1.0);
    }

    #[test]
    fn volume_body_converts_absolute_and_delta_percent_to_fraction() {
        assert_eq!(volume_body(VolumeArg::Absolute(80)), serde_json::json!({"volume": 0.8}));
        assert_eq!(volume_body(VolumeArg::Delta(5)), serde_json::json!({"delta": 0.05}));
        assert_eq!(volume_body(VolumeArg::Delta(-5)), serde_json::json!({"delta": -0.05}));
    }

    #[test]
    fn mute_body_maps_bare_on_off_to_the_three_states() {
        assert_eq!(mute_body(None).unwrap(), serde_json::json!({"mute": "toggle"}));
        assert_eq!(mute_body(Some("on")).unwrap(), serde_json::json!({"mute": "on"}));
        assert_eq!(mute_body(Some("off")).unwrap(), serde_json::json!({"mute": "off"}));
        assert!(mute_body(Some("bogus")).is_err());
    }

    #[test]
    fn render_now_matches_the_documented_playing_example() {
        // 02 §2.2 `now --json` example, human line: "playing · Chick Corea –
        // Spain · 3:12/9:41 · 96kHz/24bit · vol 80%".
        let v = serde_json::json!({
            "playback": {
                "is_playing": true, "position": 192, "duration": 581,
                "volume": 0.8, "muted": false, "sample_rate": 96000, "bit_depth": 24,
                "queue_len": 14
            },
            "track": {"id": 176544871, "title": "Spain", "artist": "Chick Corea"}
        });
        assert_eq!(
            render_now(&v),
            "playing · Chick Corea – Spain · 3:12/9:41 · 96kHz/24bit · vol 80%"
        );
    }

    #[test]
    fn render_now_stopped_state_shows_queue_count_and_no_track() {
        let v = serde_json::json!({
            "playback": {"is_playing": false, "position": 0, "duration": 0,
                         "volume": 0.8, "muted": false, "queue_len": 14},
            "track": null
        });
        assert_eq!(render_now(&v), "stopped · queue 14 tracks");
    }

    #[test]
    fn render_advance_shows_landing_track_or_queue_finished() {
        let landing = serde_json::json!({"artist": "Chick Corea", "title": "500 Miles High"});
        assert_eq!(render_advance(&landing), "-> Chick Corea – 500 Miles High");
        assert_eq!(render_advance(&serde_json::Value::Null), "queue finished");
    }

    #[test]
    fn render_advance_shows_spawn_and_ack_queued() {
        let ack = serde_json::json!({"queued": true, "direction": "next"});
        assert_eq!(render_advance(&ack), "-> queued (next)");
        let ack_prev = serde_json::json!({"queued": true, "direction": "previous"});
        assert_eq!(render_advance(&ack_prev), "-> queued (previous)");
    }

    #[test]
    fn fmt_khz_rounds_only_when_not_exact() {
        assert_eq!(fmt_khz(96000), "96kHz");
        assert_eq!(fmt_khz(192000), "192kHz");
        assert_eq!(fmt_khz(44100), "44.1kHz");
    }
}
