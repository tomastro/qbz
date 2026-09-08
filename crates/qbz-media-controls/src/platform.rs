//! macOS (MediaRemote / MPNowPlayingInfoCenter) + Windows (SMTC) backend via
//! souvlaki. No MPRIS / DesktopEntry here — macOS keys the Now Playing icon off
//! the app bundle; Windows SMTC off the package. On macOS `MediaControls` is a
//! zero-sized handle over global objc singletons (Send + Sync) and its command
//! callbacks fire on the app's run loop (Slint's winit loop), so it is driven
//! from any thread without main-thread marshaling — same as the Tauri build.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use souvlaki::{MediaControls, MediaMetadata, MediaPlayback, MediaPosition};
use souvlaki::{MediaControlEvent, PlatformConfig, SeekDirection};

use crate::types::{MediaEvent, MediaIntegration, PlaybackStatus, TrackMeta};
use crate::NativeWindow;

/// Default step for a magnitude-less MPRIS-style `Seek` (5 seconds, micros).
const SEEK_STEP_MICROS: i64 = 5_000_000;

type EventCb = Arc<dyn Fn(MediaEvent) + Send + Sync>;

/// Keeps Windows from sleeping while a track plays.
///
/// `SetThreadExecutionState` sets state on the CALLING THREAD, and this
/// module's own contract says outbound `set_playback` is "free from any
/// thread" -- in practice the mpris-server thread, a tokio worker, or the Qt
/// GUI thread, whichever pushed the update. Calling the API from there would
/// pin the flag to a thread that may be a pool worker with a short life, and
/// the request would silently evaporate with it.
///
/// So the state is owned by ONE thread that lives as long as the process, and
/// callers only send it a bool. Windows drops every request a thread made when
/// that thread exits, which is exactly why this thread never does.
#[cfg(target_os = "windows")]
mod sleep {
    use std::sync::mpsc::{channel, Sender};
    use std::sync::OnceLock;

    fn worker() -> &'static Sender<bool> {
        static TX: OnceLock<Sender<bool>> = OnceLock::new();
        TX.get_or_init(|| {
            let (tx, rx) = channel::<bool>();
            std::thread::Builder::new()
                .name("qbz-sleep-inhibit".to_string())
                .spawn(move || {
                    use windows_sys::Win32::System::Power::{
                        SetThreadExecutionState, ES_CONTINUOUS, ES_SYSTEM_REQUIRED,
                    };
                    let mut held = false;
                    while let Ok(playing) = rx.recv() {
                        if playing == held {
                            continue;
                        }
                        // ES_SYSTEM_REQUIRED without ES_DISPLAY_REQUIRED: an
                        // audio player must keep the MACHINE awake, not the
                        // screen. No ES_AWAYMODE_REQUIRED either -- that is for
                        // media-centre appliances and it suppresses the sleep
                        // the user asked for.
                        let flags = if playing {
                            ES_CONTINUOUS | ES_SYSTEM_REQUIRED
                        } else {
                            ES_CONTINUOUS
                        };
                        // SAFETY: a plain flags call with no pointers. Always on
                        // this thread, which is the whole point of the module.
                        let previous = unsafe { SetThreadExecutionState(flags) };
                        if previous == 0 {
                            // The call FAILED (it returns the previous state,
                            // or zero). Leave `held` alone: recording the new
                            // value here would make every later message with
                            // the same bool a no-op, so a failed RELEASE would
                            // pin the machine awake for the rest of the
                            // session with nothing left to undo it.
                            log::warn!(
                                "[inhibit] SetThreadExecutionState failed; execution state unchanged"
                            );
                            continue;
                        }
                        held = playing;
                        log::debug!(
                            "[inhibit] Windows execution state -> {}",
                            if playing { "system required" } else { "continuous" }
                        );
                    }
                })
                .expect("spawn sleep-inhibit thread");
            tx
        })
    }

    pub(super) fn set_playing(playing: bool) {
        let _ = worker().send(playing);
    }
}

/// Keep the machine awake while playing, INDEPENDENTLY of whether the media
/// integration started.
///
/// Deliberately a free function and not a `PlatformHandle` method. Hanging it
/// off the handle tied it to SMTC: every caller returns early when
/// `handle()` is `None`, so a failed souvlaki init or an HWND that never
/// arrived would leave playback working and the machine falling asleep
/// mid-track -- two unrelated features failing together for no reason.
///
/// A no-op everywhere but Windows: Linux hangs its logind inhibitor off the
/// MPRIS update (`inhibit.rs`) and macOS needs none.
pub fn set_sleep_inhibit(playing: bool) {
    #[cfg(target_os = "windows")]
    sleep::set_playing(playing);
    #[cfg(not(target_os = "windows"))]
    let _ = playing;
}

pub struct PlatformHandle {
    controls: Arc<Mutex<Option<MediaControls>>>,
}

impl MediaIntegration for PlatformHandle {
    fn set_metadata(&self, meta: &TrackMeta) {
        if let Ok(mut guard) = self.controls.lock() {
            if let Some(c) = guard.as_mut() {
                // SMTC wants a NATIVE path where MPRIS wants a URL; see
                // `cover_for_souvlaki`. Bound to a local so the &str borrowed
                // into MediaMetadata outlives the call.
                let cover = meta
                    .art_url
                    .as_deref()
                    .map(|u| cover_for_souvlaki(u, cfg!(target_os = "windows")));
                let md = MediaMetadata {
                    title: Some(meta.title.as_str()),
                    artist: Some(meta.artist.as_str()),
                    album: Some(meta.album.as_str()),
                    duration: meta.duration,
                    cover_url: cover.as_deref(),
                };
                let _ = c.set_metadata(md);
            }
        }
    }

    fn set_playback(&self, status: PlaybackStatus, position: Option<Duration>) {
        if let Ok(mut guard) = self.controls.lock() {
            if let Some(c) = guard.as_mut() {
                let progress = position.map(MediaPosition);
                let pb = match status {
                    PlaybackStatus::Playing => MediaPlayback::Playing { progress },
                    PlaybackStatus::Paused => MediaPlayback::Paused { progress },
                    PlaybackStatus::Stopped => MediaPlayback::Stopped,
                };
                let _ = c.set_playback(pb);
            }
        }
    }

    fn set_volume(&self, _vol: f64) {
        // souvlaki exposes no outbound volume (SMTC/MediaRemote manage it);
        // inbound SetVolume still arrives as a MediaEvent. No-op here.
    }
}

fn map_event(e: MediaControlEvent) -> Option<MediaEvent> {
    Some(match e {
        MediaControlEvent::Play => MediaEvent::Play,
        MediaControlEvent::Pause => MediaEvent::Pause,
        MediaControlEvent::Toggle => MediaEvent::Toggle,
        MediaControlEvent::Next => MediaEvent::Next,
        MediaControlEvent::Previous => MediaEvent::Previous,
        MediaControlEvent::Stop => MediaEvent::Stop,
        MediaControlEvent::Raise => MediaEvent::Raise,
        MediaControlEvent::Quit => MediaEvent::Quit,
        MediaControlEvent::Seek(SeekDirection::Forward) => MediaEvent::SeekBy(SEEK_STEP_MICROS),
        MediaControlEvent::Seek(SeekDirection::Backward) => MediaEvent::SeekBy(-SEEK_STEP_MICROS),
        MediaControlEvent::SeekBy(dir, dur) => {
            let micros = dur.as_micros() as i64;
            MediaEvent::SeekBy(match dir {
                SeekDirection::Forward => micros,
                SeekDirection::Backward => -micros,
            })
        }
        MediaControlEvent::SetPosition(pos) => MediaEvent::SetPosition(pos.0.as_micros() as i64),
        MediaControlEvent::SetVolume(v) => MediaEvent::SetVolume(v),
        MediaControlEvent::OpenUri(_) => return None,
    })
}

/// Convert a cover-art URL into the exact string souvlaki's Windows arm can
/// open.
///
/// souvlaki 0.8.3 (`src/platform/windows/mod.rs:192-203`) branches like this:
///
/// ```text
/// if url.starts_with("file://") {
///     let path = url.trim_start_matches("file://");
///     StorageFile::GetFileFromPathAsync(path)     // wants C:\x or \\nas\x
/// } else {
///     RandomAccessStreamReference::CreateFromUri(url)
/// }
/// ```
///
/// So `file://` is the DISCRIMINATOR and must survive, while everything after
/// it has to be a native Windows path. Two ways to get this wrong, and the
/// first draft of this function managed one of them:
///
/// - Hand over `file:///C:/x.jpg` and souvlaki trims seven characters, leaving
///   `/C:/x.jpg` -- a leading slash before a drive letter, which never opens.
/// - Hand over a bare `C:\x.jpg` and it no longer starts with `file://`, so it
///   takes the REMOTE branch and `Uri::CreateUri` fails on it.
///
/// The answer is the hybrid `file://C:\x.jpg`: prefix kept, remainder native.
///
/// UNC is a real Windows path too. `file://nas/music/a.jpg` names
/// `\\nas\music\a.jpg`, and leaving it alone would hand souvlaki the
/// relative `nas/music/a.jpg`.
///
/// The percent-decode is FULL and applied ONCE. `art_url` reaches this from
/// two producers -- `url::Url::from_file_path`, which escapes a space as
/// `%20`, and `fs_url::file_url`, which escapes only `%`, `#` and `?` -- and a
/// single full decode is correct for both. `fs_url::local_path` is NOT used
/// here for that reason: its deliberate three-escape decode would leave a
/// `%20` from the other producer intact.
pub(crate) fn cover_for_souvlaki(url: &str, windows: bool) -> String {
    if !windows {
        return url.to_string();
    }
    let Some(rest) = url.strip_prefix("file://") else {
        // http(s) and anything else: souvlaki's Uri branch is the right one.
        return url.to_string();
    };

    let decoded = urlencoding::decode(rest)
        .map(|c| c.into_owned())
        .unwrap_or_else(|_| rest.to_string());
    let b = decoded.as_bytes();

    let native = if b.len() >= 3 && b[0] == b'/' && b[1].is_ascii_alphabetic() && b[2] == b':' {
        // `file:///C:/x` -> the empty authority's slash sits in front of the
        // drive letter; drop it.
        decoded[1..].to_string()
    } else if !decoded.starts_with('/') && !decoded.is_empty() {
        // A non-empty authority is a UNC server: `file://nas/share/x`.
        format!("//{decoded}")
    } else {
        // A POSIX absolute path. Not a Windows path at all, and nothing here
        // can make it one -- hand it back untouched rather than inventing
        // `\home\...`.
        return url.to_string();
    };

    format!("file://{}", native.replace('/', "\\"))
}

pub fn spawn(on_event: EventCb, native: NativeWindow) -> Option<PlatformHandle> {
    // souvlaki's Windows backend does NOT degrade gracefully without an HWND:
    // `config.hwnd.expect(...)` at souvlaki-0.8.3/src/platform/windows/mod.rs:57-59
    // PANICS. The guard therefore stays even now that a handle IS wired:
    // a missing integration is a missing feature, a panic inside a tokio task
    // is a dead session. The caller reads the HWND on the GUI thread after the
    // window is shown, so None here means it asked too early.
    #[cfg(target_os = "windows")]
    if native.hwnd.is_none() {
        log::warn!(
            "[media-controls] SMTC disabled: no top-level window yet (init ran before it was shown)"
        );
        return None;
    }

    spawn_native(on_event, native)
}

fn spawn_native(on_event: EventCb, native: NativeWindow) -> Option<PlatformHandle> {
    let config = PlatformConfig {
        dbus_name: "com.blitzfc.qbz",
        display_name: "QBZ",
        // macOS ignores it; Windows registers SMTC against this window.
        hwnd: native.hwnd,
    };

    let mut controls = match MediaControls::new(config) {
        Ok(c) => c,
        Err(e) => {
            log::warn!("[media-controls] souvlaki init failed: {e:?}");
            return None;
        }
    };

    if let Err(e) = controls.attach(move |event: MediaControlEvent| {
        if let Some(ev) = map_event(event) {
            on_event(ev);
        }
    }) {
        log::warn!("[media-controls] souvlaki attach failed: {e:?}");
        return None;
    }

    log::info!("[media-controls] souvlaki (SMTC/MediaRemote) initialized");
    Some(PlatformHandle {
        controls: Arc::new(Mutex::new(Some(controls))),
    })
}

#[cfg(test)]
mod tests {
    use super::cover_for_souvlaki;

    #[test]
    fn windows_keeps_the_file_prefix_and_makes_the_rest_native() {
        // The prefix is souvlaki's discriminator: drop it and the string takes
        // the remote-URI branch. Keep it, and what it trims must be a real
        // Windows path.
        assert_eq!(
            cover_for_souvlaki("file:///C:/Users/v/c.jpg", true),
            "file://C:\\Users\\v\\c.jpg"
        );
    }

    #[test]
    fn non_windows_is_byte_identical() {
        assert_eq!(
            cover_for_souvlaki("file:///home/v/c.jpg", false),
            "file:///home/v/c.jpg"
        );
        assert_eq!(
            cover_for_souvlaki("file:///C:/Users/v/c.jpg", false),
            "file:///C:/Users/v/c.jpg"
        );
    }

    #[test]
    fn remote_urls_are_never_touched() {
        assert_eq!(cover_for_souvlaki("https://x/y.jpg", true), "https://x/y.jpg");
    }

    #[test]
    fn a_unc_url_becomes_a_unc_path() {
        // `file://nas/music/a.jpg` is a real Windows path. Left alone, souvlaki
        // would trim it to the RELATIVE `nas/music/a.jpg`.
        assert_eq!(
            cover_for_souvlaki("file://nas/music/a.jpg", true),
            "file://\\\\nas\\music\\a.jpg"
        );
    }

    #[test]
    fn a_posix_path_is_left_alone_even_on_windows() {
        // No drive, no authority: nothing here can turn it into a Windows path.
        assert_eq!(
            cover_for_souvlaki("file:///home/v/c.jpg", true),
            "file:///home/v/c.jpg"
        );
    }

    #[test]
    fn both_producers_percent_decode_the_same_way() {
        // fs_url::file_url leaves a space LITERAL...
        assert_eq!(
            cover_for_souvlaki("file:///C:/My Music/a b.jpg", true),
            "file://C:\\My Music\\a b.jpg"
        );
        // ...while url::Url::from_file_path escapes it as %20. One full decode
        // is right for both.
        assert_eq!(
            cover_for_souvlaki("file:///C:/My%20Music/a.jpg", true),
            "file://C:\\My Music\\a.jpg"
        );
    }

    #[test]
    fn the_escapes_that_matter_are_undone() {
        // A # in a filename reaches SMTC as %23 otherwise, and the cover
        // silently never loads.
        assert_eq!(
            cover_for_souvlaki("file:///C:/m/a%23b%3Fc.jpg", true),
            "file://C:\\m\\a#b?c.jpg"
        );
    }

    #[test]
    fn a_literal_percent_twenty_is_not_double_decoded() {
        // A filename that really contains "%20" is escaped to %2520 by either
        // producer; ONE decode must give back "%20", not a space.
        assert_eq!(
            cover_for_souvlaki("file:///C:/m/a%2520b.jpg", true),
            "file://C:\\m\\a%20b.jpg"
        );
    }
}
