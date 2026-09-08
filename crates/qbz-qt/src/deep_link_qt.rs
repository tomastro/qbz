//! Launcher deep links shared by cold starts and the Linux single-instance
//! D-Bus handoff.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

static PENDING: Mutex<Option<String>> = Mutex::new(None);
static ONLINE_SESSION: AtomicBool = AtomicBool::new(false);

/// The native Qobuz URL shapes accepted by the desktop launcher files.
pub(crate) fn is_qobuz_link(arg: &str) -> bool {
    arg.starts_with("qobuzapp://")
        || arg.starts_with("https://play.qobuz.com/")
        || arg.starts_with("http://play.qobuz.com/")
        || arg.starts_with("https://open.qobuz.com/")
        || arg.starts_with("http://open.qobuz.com/")
}

/// `qbz://` is OUR scheme, registered by the Windows MSI unconditionally
/// (`qobuzapp://` is a runtime opt-in there — link_handler_qt). It is rewritten rather than taught downstream: the resolver
/// (`qbz-music-link`) knows `qobuzapp://` and the Qobuz web shapes and nothing
/// else, so an accepted `qbz://` would travel the whole way and then be
/// classified "unsupported or invalid music link" -- which is exactly what a
/// live test of the forwarding path produced. The two carry the same
/// `album/track/artist` shape, so a prefix swap is the whole translation.
///
/// `is_qobuz_link` deliberately still answers false for it: that predicate
/// describes what the LAUNCHER files advertise, and its test pins that.
fn rewrite_own_scheme(arg: &str) -> Option<String> {
    arg.strip_prefix("qbz://")
        .map(|rest| format!("qobuzapp://{rest}"))
}

/// The first argument that is a link we can actually act on.
///
/// Keeps SCANNING past one it cannot use rather than giving up: a launcher can
/// hand over several arguments and the first match is not necessarily the
/// usable one.
fn select_link(args: &[String]) -> Option<String> {
    for arg in args {
        let candidate = if is_qobuz_link(arg) {
            arg.clone()
        } else if let Some(rewritten) = rewrite_own_scheme(arg) {
            rewritten
        } else {
            continue;
        };

        // WINDOWS ONLY. ShellExecute hands the app whatever the registered
        // `shell\open\command` produced, and an unencoded space in the link
        // splits it across argv -- so a fragment can carry the scheme and
        // nothing else. Acting on that navigates nowhere and reads as the deep
        // link silently failing.
        //
        // Not applied elsewhere: Linux and macOS have accepted whatever
        // matched the prefix since this shipped, and tightening that from a
        // Windows port would be a behaviour change on platforms that never
        // asked for it.
        #[cfg(target_os = "windows")]
        if url::Url::parse(&candidate).is_err() {
            log::warn!(
                "[qbz-qt] deep link: ignoring an unparseable argument ({})",
                candidate.split('?').next().unwrap_or(&candidate)
            );
            continue;
        }

        return Some(candidate);
    }
    None
}

/// Is this something the forwarding path may act on?
///
/// The single-instance server feeds it whatever arrived on the pipe, which is
/// not argv and is not ours.
#[cfg(any(target_os = "windows", test))]
pub(crate) fn is_actionable(url: &str) -> bool {
    if !is_qobuz_link(url) && !url.starts_with("qbz://") {
        return false;
    }
    // A parse is NOT enough, which a test caught: `Url::parse("qbz://")`
    // SUCCEEDS -- a valid scheme with an empty everything -- so a bare scheme
    // would have passed the gate and reached the resolver as a navigation
    // request for nothing.
    //
    // Every shape this accepts carries its target in the AUTHORITY:
    // `qobuzapp://album/1` and `qbz://album/1` put "album" there, and the web
    // URLs put the Qobuz host there. An empty one is not a link.
    match url::Url::parse(url) {
        Ok(u) => u.host_str().is_some_and(|h| !h.is_empty()),
        Err(_) => false,
    }
}

/// Capture argv before the single-instance decision so a duplicate process
/// can forward its URL to the owner of the bus name.
pub(crate) fn capture_argv() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(url) = select_link(&args) {
        log::info!(
            "[qbz-qt] deep link: captured {}",
            url.split('?').next().unwrap_or(&url)
        );
        stash(url);
    }
}

/// Newest launch intent wins, matching the former frontend.
pub(crate) fn stash(url: String) {
    if let Ok(mut pending) = PENDING.lock() {
        *pending = Some(url);
    }
}

/// Return an unconfirmed handoff without replacing a newer launch intent.
#[cfg(target_os = "linux")]
pub(crate) fn restore_pending(url: String) {
    if let Ok(mut pending) = PENDING.lock() {
        pending.get_or_insert(url);
    }
}

pub(crate) fn take_pending() -> Option<String> {
    PENDING.lock().ok().and_then(|mut pending| pending.take())
}

pub(crate) fn has_pending() -> bool {
    PENDING.lock().is_ok_and(|pending| pending.is_some())
}

/// Bind/unbind the only context in which a Qobuz route can be fetched.
pub(crate) fn set_online_session(active: bool) {
    ONLINE_SESSION.store(active, Ordering::SeqCst);
    if active {
        drain_pending();
    }
}

/// Resolve the pending link once an authenticated shell is available.
pub(crate) fn drain_pending() {
    if !ONLINE_SESSION.load(Ordering::SeqCst) {
        return;
    }
    let Some(url) = take_pending() else {
        return;
    };
    log::info!(
        "[qbz-qt] deep link: resolving {}",
        url.split('?').next().unwrap_or(&url)
    );
    crate::link_resolver_qt::resolve_deep_link(url);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_launcher_qobuz_shapes() {
        assert!(is_qobuz_link("https://open.qobuz.com/album/a"));
        assert!(is_qobuz_link("http://play.qobuz.com/track/1"));
        assert!(is_qobuz_link("qobuzapp://artist/1"));
        assert!(!is_qobuz_link("https://open.qobuz.com.evil.test/album/a"));
        assert!(!is_qobuz_link("https://spotify.com/track/1"));
        assert!(!is_qobuz_link("qbz://album/a"));
    }

    #[test]
    fn our_own_scheme_is_rewritten_not_taught() {
        // The resolver knows qobuzapp:// and the Qobuz web shapes. A qbz://
        // URL that reached it came back "unsupported or invalid music link",
        // so the translation happens here.
        assert_eq!(
            rewrite_own_scheme("qbz://album/12345").as_deref(),
            Some("qobuzapp://album/12345")
        );
        assert_eq!(rewrite_own_scheme("qobuzapp://album/1"), None);
        assert_eq!(rewrite_own_scheme("https://open.qobuz.com/album/1"), None);
    }

    #[test]
    fn a_rewritten_link_is_selected() {
        let args = vec!["--flag".to_string(), "qbz://track/9".to_string()];
        assert_eq!(select_link(&args).as_deref(), Some("qobuzapp://track/9"));
    }

    #[test]
    fn the_pipe_gate_accepts_only_real_links() {
        assert!(is_actionable("qobuzapp://album/1"));
        assert!(is_actionable("qbz://album/1"));
        assert!(is_actionable("https://play.qobuz.com/album/1"));
        assert!(!is_actionable("https://spotify.com/track/1"));
        assert!(!is_actionable("qbz://"));
        assert!(!is_actionable("not a url at all"));
    }

    #[test]
    fn first_matching_argument_wins() {
        let args = vec![
            "--flag".to_string(),
            "https://open.qobuz.com/album/first".to_string(),
            "qobuzapp://album/second".to_string(),
        ];
        assert_eq!(
            select_link(&args),
            Some("https://open.qobuz.com/album/first".to_string())
        );
    }
}
