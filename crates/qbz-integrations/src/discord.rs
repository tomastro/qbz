//! Discord Rich Presence integration (frontend-agnostic).
//!
//! Ported ~1:1 from the Tauri `discord_rpc.rs` backend, minus the
//! `#[tauri::command]` / `tauri::State` wrappers — the logic is plain Rust so
//! any frontend (Slint, TUI, CLI) drives it through [`DiscordRpc`].
//!
//! Connection is lazy: the IPC client is created on the first [`DiscordRpc::update`]
//! call when the feature is enabled, and dropped if any operation fails. Failure
//! is silent — when Discord is not running, when the sandbox blocks the IPC
//! socket, or when the user has not opted in, the activity simply does not appear.
//!
//! The IPC calls are blocking; call [`DiscordRpc`] methods off the UI thread
//! (e.g. inside `tokio::task::spawn_blocking`).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use discord_rich_presence::{activity, DiscordIpc, DiscordIpcClient};

/// Public Discord Application ID for QBZ. Public identifier, not a secret.
const DISCORD_APP_ID: &str = "1501835855587708988";
const QBZ_HOMEPAGE: &str = "https://qbz.lol";

/// Bridge our Flatpak sandbox to the Discord-Flatpak IPC socket.
///
/// The `discord-rich-presence` crate searches `$XDG_RUNTIME_DIR/discord-ipc-N`
/// for `N=0..9`. Native Discord (deb / rpm / AUR) writes its socket directly
/// to that path, so the search succeeds out of the box. Discord installed via
/// Flathub writes to `$XDG_RUNTIME_DIR/app/com.discordapp.Discord/discord-ipc-N`
/// because each Flatpak app gets its own runtime subdirectory; the crate's
/// default search misses it, and Rich Presence appears broken even though
/// Discord is running.
///
/// Other Flathub apps work around this with a startup wrapper script that
/// unconditionally symlinks every iteration into `$XDG_RUNTIME_DIR` for every
/// user. That conflicts with QBZ's external-service-integration posture: all
/// optional integrations are opt-in (Last.fm, ListenBrainz, MusicBrainz, Plex,
/// Discord here). Running the wrapper for every Flatpak instance — including
/// users who never enabled Discord RPC — silently turns the integration into
/// opt-out at the sandbox layer.
///
/// We make the symlinks here instead, lazily, the first time the user actually
/// connects to Discord. If the user never toggles Discord RPC on, these links
/// are never created and the sandbox stays untouched. The links live for the
/// lifetime of the Flatpak instance (`$XDG_RUNTIME_DIR` is scoped per-instance)
/// so there's no cross-session pollution either.
///
/// Returns silently on every failure path — missing env var, non-Flatpak
/// process, ENOENT on the source dir, EEXIST on the link target, etc.
fn prepare_flatpak_discord_socket_links() {
    if std::env::var_os("FLATPAK_ID").is_none() {
        return;
    }
    let Some(runtime_dir) = std::env::var_os("XDG_RUNTIME_DIR") else {
        return;
    };
    let runtime_dir = std::path::PathBuf::from(runtime_dir);

    // discord-rich-presence iterates 0..=9; only bother creating a link when
    // the slot in $XDG_RUNTIME_DIR is currently empty AND the corresponding
    // path inside the Discord-Flatpak app dir exists. Either condition failing
    // is normal (Discord not running, slot already taken by native Discord,
    // etc.) and silently skipped.
    for i in 0..=9 {
        let link_target = runtime_dir.join(format!("discord-ipc-{}", i));
        if link_target.exists() || link_target.symlink_metadata().is_ok() {
            continue;
        }
        let source = runtime_dir
            .join("app")
            .join("com.discordapp.Discord")
            .join(format!("discord-ipc-{}", i));
        if !source.exists() {
            continue;
        }
        #[cfg(unix)]
        let _ = std::os::unix::fs::symlink(&source, &link_target);
    }
}

/// The "now listening" snapshot pushed to Discord. Mirrors the params the Tauri
/// `v2_discord_rpc_update` command took.
#[derive(Debug, Clone, Default)]
pub struct NowListening {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub is_playing: bool,
    /// Current playback position, seconds.
    pub current_time: f64,
    /// Track duration, seconds (0 = unknown; no end timestamp then).
    pub duration: f64,
    /// Large-image asset: a Discord asset key OR an http(s) cover URL. `None`
    /// falls back to the "cover" asset key.
    pub cover_url: Option<String>,
}

/// Discord Rich Presence client holder. Owns the lazy IPC connection + the
/// opt-in flag. Cheap to keep as a process-global (the Slint service does).
#[derive(Default)]
pub struct DiscordRpc {
    client: Mutex<Option<DiscordIpcClient>>,
    enabled: AtomicBool,
}

impl DiscordRpc {
    pub const fn new() -> Self {
        Self {
            client: Mutex::new(None),
            enabled: AtomicBool::new(false),
        }
    }

    /// Whether the integration is currently opted in.
    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::SeqCst)
    }

    /// Flip the opt-in flag. Disabling tears down the live activity + IPC
    /// connection so the presence disappears immediately.
    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::SeqCst);
        if !enabled {
            self.clear();
        }
    }

    /// Clear the activity and drop the IPC connection (no-op when not connected).
    pub fn clear(&self) {
        if let Ok(mut guard) = self.client.lock() {
            if let Some(client) = guard.as_mut() {
                let _ = client.clear_activity();
                let _ = client.close();
            }
            *guard = None;
        }
    }

    /// Push the current "now listening" snapshot. Lazily connects on first use
    /// when enabled; a no-op (Ok) when disabled. Silent on every IPC failure
    /// (Discord closed / sandbox blocked): the connection is dropped so the
    /// next update retries cleanly.
    pub fn update(&self, meta: &NowListening) {
        if !self.enabled.load(Ordering::SeqCst) {
            return;
        }

        let Ok(mut guard) = self.client.lock() else {
            return;
        };

        if guard.is_none() {
            // Bridge the Flatpak sandbox to Discord's IPC socket if we're
            // running under Flatpak. No-op outside that context.
            prepare_flatpak_discord_socket_links();
            // discord-rich-presence 1.x: DiscordIpcClient::new is infallible
            // (returns Self directly); failures show up at connect() time.
            let mut client = DiscordIpcClient::new(DISCORD_APP_ID);
            if client.connect().is_ok() {
                *guard = Some(client);
            }
        }

        let Some(client) = guard.as_mut() else {
            return;
        };

        let state_str = format!("by {}", meta.artist);
        let small_text = if meta.is_playing {
            "Playing".to_string()
        } else {
            let mins = (meta.current_time / 60.0).floor() as u32;
            let secs = (meta.current_time % 60.0).floor() as u32;
            format!("Paused at {:02}:{:02}", mins, secs)
        };
        let large_image = meta
            .cover_url
            .clone()
            .unwrap_or_else(|| "cover".to_string());

        let mut act = activity::Activity::new()
            .details(&meta.title)
            .state(&state_str)
            .activity_type(activity::ActivityType::Listening)
            .assets(
                activity::Assets::new()
                    .large_image(&large_image)
                    .small_image("icon")
                    .large_text(&meta.album)
                    .small_text(&small_text),
            )
            .buttons(vec![activity::Button::new("Get QBZ", QBZ_HOMEPAGE)]);

        if meta.is_playing {
            if let Ok(now) = SystemTime::now().duration_since(UNIX_EPOCH) {
                let start_time = now.as_secs() as i64 - meta.current_time as i64;
                let mut ts = activity::Timestamps::new().start(start_time);
                if meta.duration > 0.0 {
                    ts = ts.end(start_time + meta.duration as i64);
                }
                act = act.timestamps(ts);
            }
        }

        if client.set_activity(act).is_err() {
            *guard = None;
        }
    }
}
