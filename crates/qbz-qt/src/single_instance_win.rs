//! Windows single instance, and the deep-link handoff that rides on it.
//!
//! The sibling of `single_instance_qt.rs`, which does the same job on Linux
//! over the session bus. Same three entry points, same `present_or_defer`
//! behaviour, so `main.rs` reads the same either way.
//!
//! ## Why a mutex AND a pipe, rather than one of them
//!
//! Arbitration and transport are different problems and Windows solves them
//! with different objects.
//!
//! `CreateMutexW` arbitrates ATOMICALLY: the kernel either creates the object
//! or tells you it already existed, with no window in between for a second
//! launch to slip through. A named pipe cannot be asked that question without
//! `FILE_FLAG_FIRST_PIPE_INSTANCE`, and a Qt `QLocalServer` never passes it
//! (`qlocalserver_win.cpp` opens with `PIPE_UNLIMITED_INSTANCES`), so two
//! processes can both believe they are the server.
//!
//! The pipe then carries the message. The plan called for QLocalServer here,
//! which would have meant a new `Q_OBJECT` C++ file in the moc list and
//! `.qt_module("Network")` in the build. A raw named pipe is the same
//! primitive underneath with none of that build surface, and this module owns
//! both halves of a protocol that is two verbs long.
//!
//! ## The race the retry exists for
//!
//! The primary creates the mutex BEFORE its pipe server is listening. A second
//! launch inside that gap finds the mutex taken and the pipe absent, so it
//! must WAIT for the pipe rather than conclude nothing is there -- otherwise
//! the deep link that started it is dropped on the floor.
#![cfg(target_os = "windows")]

use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::time::Duration;

/// `Local\` = per-session, which matches the per-user MSI: two users logged in
/// at once each get their own QBZ, exactly as they each get their own D-Bus
/// session on Linux.
const MUTEX_NAME: &str = "Local\\com.blitzfc.qbz.singleton\0";

/// The pipe name, QUALIFIED BY SESSION ID.
///
/// The mutex is `Local\`, which is per-session. Pipe names are NOT: the
/// `\\.\pipe\` namespace is machine-global and has no session prefix. With one
/// fixed name and two users signed in at once, each acquires its own mutex
/// (different namespace) but only the first can create the pipe -- the second
/// primary then serves nothing, and ITS later launches would forward to the
/// OTHER USER'S process. Qualifying by session id restores the pairing the
/// mutex already had.
fn pipe_name() -> String {
    format!("\\\\.\\pipe\\com.blitzfc.qbz.singleton.{}", session_id())
}

/// This process's Terminal Services session. 0 on failure, which is still a
/// consistent answer -- every instance that fails agrees on it.
fn session_id() -> u32 {
    use windows_sys::Win32::System::RemoteDesktop::ProcessIdToSessionId;
    use windows_sys::Win32::System::Threading::GetCurrentProcessId;

    let mut id: u32 = 0;
    // SAFETY: `id` is a valid out-slot; the call only writes it.
    let ok = unsafe { ProcessIdToSessionId(GetCurrentProcessId(), &mut id) };
    if ok == 0 {
        0
    } else {
        id
    }
}

/// Held for the process lifetime and deliberately never closed: releasing it
/// would let a second instance start while this one is still running.
static MUTEX: OnceLock<usize> = OnceLock::new();
static UI_READY: AtomicBool = AtomicBool::new(false);
static PENDING_PRESENT: AtomicBool = AtomicBool::new(false);

/// Called once QbzTray has registered its Qt-thread hop, exactly as the Linux
/// module is. Independent of whether the tray icon itself is enabled.
pub(crate) fn bind_ui() {
    UI_READY.store(true, Ordering::SeqCst);
    if PENDING_PRESENT.swap(false, Ordering::SeqCst) {
        crate::tray_qt::present();
    }
}

/// Present now if the UI can take it, or remember to present when it can.
///
/// A forwarded message can arrive before the QML shell exists; presenting into
/// that gap does nothing and the user's second launch looks ignored.
fn present_or_defer() {
    if UI_READY.load(Ordering::SeqCst) {
        crate::tray_qt::present();
        return;
    }
    PENDING_PRESENT.store(true, Ordering::SeqCst);
    // RE-CHECK, and this is not belt-and-braces. `bind_ui` can set UI_READY
    // and run its own swap in the window between the load above and the store
    // below it; the flag would then be set with nobody left to consume it, and
    // the user's second launch would look ignored. Reading UI_READY again
    // after publishing closes that order.
    if UI_READY.load(Ordering::SeqCst) && PENDING_PRESENT.swap(false, Ordering::SeqCst) {
        crate::tray_qt::present();
    }
}

/// True when this process should continue as the primary instance.
pub(crate) fn acquire_or_raise() -> bool {
    use windows_sys::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
    use windows_sys::Win32::System::Threading::CreateMutexW;

    let name: Vec<u16> = MUTEX_NAME.encode_utf16().collect();
    // SAFETY: NUL-terminated UTF-16 that outlives the call; a null security
    // descriptor asks for the default, which is what a per-session object
    // wants.
    let handle = unsafe { CreateMutexW(std::ptr::null(), 1, name.as_ptr()) };
    if handle.is_null() {
        // Cannot arbitrate. Continuing as primary is the safe failure: the
        // worst case is two windows, where bailing out would be no window.
        log::warn!("[qbz-qt] CreateMutexW failed; continuing as the primary instance");
        return true;
    }

    // SAFETY: reads this thread's last-error, set by the call above.
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        forward_to_primary();
        return false;
    }

    let _ = MUTEX.set(handle as usize);
    start_server();
    true
}

/// Hand our argv to the instance that owns the mutex.
///
/// RETRIES, because the primary creates the mutex before it listens. Ten
/// attempts at 100 ms covers the gap without making a genuinely absent server
/// (a primary that died between the two) cost more than a second.
fn forward_to_primary() {
    let line = match crate::deep_link_qt::take_pending() {
        Some(url) => format!("OPEN {url}\n"),
        None => "PRESENT\n".to_string(),
    };

    for attempt in 0..10 {
        match std::fs::OpenOptions::new().write(true).open(pipe_name()) {
            Ok(mut pipe) => {
                if let Err(e) = pipe.write_all(line.as_bytes()) {
                    log::warn!("[qbz-qt] forwarding to the primary instance failed: {e}");
                }
                let _ = pipe.flush();
                log::info!(
                    "[qbz-qt] another instance owns the mutex; forwarded {}",
                    line.split_whitespace().next().unwrap_or("?")
                );
                return;
            }
            Err(_) if attempt < 9 => std::thread::sleep(Duration::from_millis(100)),
            Err(e) => {
                log::warn!("[qbz-qt] the primary instance is not listening ({e}); exiting anyway")
            }
        }
    }
}

/// Serve the pipe for the life of the process.
///
/// One connection at a time is enough: the messages are two words long and a
/// second launch is a human action. A fresh pipe instance is created per
/// connection because `std::fs::File` closes its handle on drop, which
/// disconnects the client cleanly.
fn start_server() {
    let spawned = std::thread::Builder::new()
        .name("qbz-single-instance".to_string())
        .spawn(|| loop {
            let Some(handle) = create_pipe_instance() else {
                // The name is unusable; without a server this process is still
                // a perfectly good primary, so stop serving rather than spin.
                log::warn!("[qbz-qt] single-instance pipe unavailable; not listening");
                return;
            };
            if let Some(line) = accept_one(handle) {
                handle_line(line.trim());
            }
        });
    if let Err(e) = spawned {
        log::warn!("[qbz-qt] could not spawn the single-instance server: {e}");
    }
}

fn create_pipe_instance() -> Option<isize> {
    use windows_sys::Win32::Storage::FileSystem::PIPE_ACCESS_INBOUND;
    use windows_sys::Win32::System::Pipes::{
        CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT,
    };

    let name: Vec<u16> = pipe_name()
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    // SAFETY: NUL-terminated UTF-16 alive across the call. Inbound only -- the
    // protocol never answers.
    //
    // The security descriptor is the DEFAULT, and what that means is worth
    // stating correctly rather than assuming: full control for SYSTEM,
    // administrators and the creator-owner, plus READ for Everyone. Clients
    // here ask for write, so another ordinary user cannot inject a line, and
    // Mandatory Integrity Control keeps a low-integrity process out. What CAN
    // write is a process running as this same user, or an elevated
    // administrator -- both of which can already do far worse to a process
    // they own. A logon-SID DACL would close the same-user-other-session case
    // properly and is the documented way; the session-qualified NAME above
    // separates the sessions in practice, and `handle_line` validates what
    // arrives rather than trusting it.
    //
    // PIPE_REJECT_REMOTE_CLIENTS because this is strictly local IPC: the
    // namespace is reachable over SMB and nothing about a second launch is.
    let h = unsafe {
        CreateNamedPipeW(
            name.as_ptr(),
            PIPE_ACCESS_INBOUND,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            1,
            512,
            512,
            0,
            std::ptr::null(),
        )
    };
    if h == windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE {
        return None;
    }
    Some(h as isize)
}

/// Block until one client connects, read what it sends, then close.
fn accept_one(handle: isize) -> Option<String> {
    use std::os::windows::io::FromRawHandle;
    use windows_sys::Win32::System::Pipes::ConnectNamedPipe;

    // SAFETY: a live pipe handle from `create_pipe_instance`. Blocks until a
    // client arrives, which is the point of the dedicated thread.
    let connected = unsafe { ConnectNamedPipe(handle as _, std::ptr::null_mut()) };
    // A client that connected between CreateNamedPipeW and here makes this
    // return 0 with ERROR_PIPE_CONNECTED, which is success.
    if connected == 0 {
        use windows_sys::Win32::Foundation::{GetLastError, ERROR_PIPE_CONNECTED};
        // SAFETY: reads this thread's last-error.
        if unsafe { GetLastError() } != ERROR_PIPE_CONNECTED {
            // SAFETY: taking ownership so the handle is closed on drop.
            drop(unsafe { std::fs::File::from_raw_handle(handle as _) });
            return None;
        }
    }

    // SAFETY: ownership moves to the File, which closes the handle on drop and
    // disconnects the client with it.
    let mut file = unsafe { std::fs::File::from_raw_handle(handle as _) };
    let mut buf = String::new();
    // BOUNDED. The protocol's longest legal line is a URL; anything past this
    // is not ours, and an unbounded read on a pipe anyone in this session can
    // write is a free way to grow this process without limit.
    match file.take(4096).read_to_string(&mut buf) {
        Ok(_) => Some(buf),
        Err(e) => {
            log::warn!("[qbz-qt] single-instance read failed: {e}");
            None
        }
    }
}

/// The whole protocol: two verbs.
fn handle_line(line: &str) {
    if let Some(url) = line.strip_prefix("OPEN ") {
        let url = url.trim();
        if url.is_empty() {
            return;
        }
        // VALIDATE. This is not argv: it arrived on a pipe, and the only
        // thing keeping it honest is this check. Anything that is not a link
        // we would have accepted on our own command line is dropped.
        if !crate::deep_link_qt::is_actionable(url) {
            log::warn!("[qbz-qt] single-instance: ignoring an unusable OPEN payload");
            return;
        }
        log::info!(
            "[qbz-qt] deep link forwarded from a second instance: {}",
            url.split('?').next().unwrap_or(url)
        );
        crate::deep_link_qt::stash(url.to_string());
        present_or_defer();
        crate::deep_link_qt::drain_pending();
    } else if line == "PRESENT" {
        present_or_defer();
    }
}
