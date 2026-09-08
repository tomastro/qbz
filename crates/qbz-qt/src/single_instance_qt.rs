//! Linux single-instance guard and warm deep-link forwarding.
//!
//! The primary owns `com.blitzfc.qbz` on the session bus. A later launch asks
//! it to present its current window, forwarding a launcher URL when present,
//! and exits. D-Bus failure is deliberately fail-open so startup is never
//! blocked by a broken or absent session bus.
#![cfg(target_os = "linux")]

use std::cell::Cell;
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use async_io::{Async, Timer};
use futures_util::future::{select, Either};
use zbus::fdo::{RequestNameFlags, RequestNameReply};
use zbus::Connection;

const BUS_NAME: &str = "com.blitzfc.qbz";
const OBJECT_PATH: &str = "/com/blitzfc/qbz";
const IFACE_NAME: &str = "com.blitzfc.qbz.SingleInstance";

const PROBE_TIMEOUT: Duration = Duration::from_secs(2);
// An existing non-socket gives Qt/libdbus an immediate connection failure.
// Unsetting the address would enable autolaunch and another unbounded wait.
const UNAVAILABLE_BUS: &str = "unix:path=/dev/null";

#[derive(Debug)]
struct ProbeError {
    detail: String,
    quarantine_bus: bool,
}

impl std::fmt::Display for ProbeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.detail)
    }
}

#[derive(Clone, Copy)]
enum ProbePhase {
    Connect,
    Handshake,
    RequestName,
    OpenUrl,
    Present,
    Raise,
}

impl ProbePhase {
    fn label(self) -> &'static str {
        match self {
            Self::Connect => "connect",
            Self::Handshake => "AUTH/Hello/object registration",
            Self::RequestName => "RequestName",
            Self::OpenUrl => "OpenUrl",
            Self::Present => "Present",
            Self::Raise => "MPRIS Raise",
        }
    }

    fn failure(self, detail: String, error: Option<&zbus::Error>) -> ProbeError {
        // A stalled remote QBZ method says nothing about the session bus.
        // Likewise, a D-Bus error reply proves that the bus is responsive.
        // Unsupported transports remain available to Qt, which supports more
        // address types than this deliberately Unix-only startup probe.
        let quarantine_bus = matches!(self, Self::Connect | Self::Handshake | Self::RequestName)
            && !matches!(
                error,
                Some(zbus::Error::Unsupported | zbus::Error::MethodError(..))
            );
        ProbeError {
            detail: format!("{}: {detail}", self.label()),
            quarantine_bus,
        }
    }
}

static CONN: OnceLock<SessionConnection> = OnceLock::new();
static UI_READY: AtomicBool = AtomicBool::new(false);
static PENDING_PRESENT: AtomicBool = AtomicBool::new(false);

struct SingleInstanceIface;

fn present_or_defer() {
    PENDING_PRESENT.store(true, Ordering::SeqCst);
    // Publishing the intent first closes the race with bind_ui's drain.
    if UI_READY.load(Ordering::SeqCst) && PENDING_PRESENT.swap(false, Ordering::SeqCst) {
        crate::tray_qt::present();
    }
}

#[zbus::interface(name = "com.blitzfc.qbz.SingleInstance")]
impl SingleInstanceIface {
    fn present(&self) {
        present_or_defer();
    }

    fn open_url(&self, url: &str) {
        crate::deep_link_qt::stash(url.to_string());
        present_or_defer();
        crate::deep_link_qt::drain_pending();
    }
}

/// Called once QbzTray has registered its Qt-thread hop. It is independent of
/// whether the optional tray icon itself is enabled.
pub(crate) fn bind_ui() {
    UI_READY.store(true, Ordering::SeqCst);
    if PENDING_PRESENT.swap(false, Ordering::SeqCst) {
        crate::tray_qt::present();
    }
}

/// Own the transport as well as zbus. Cancellation shuts down the socket
/// synchronously, even if an executor task still holds a connection clone.
/// Nothing can acquire a name or send another RPC after this guard is dropped.
struct SessionConnection {
    socket: Arc<Async<UnixStream>>,
    connection: Option<Connection>,
}

impl Drop for SessionConnection {
    fn drop(&mut self) {
        let _ = self.socket.get_ref().shutdown(std::net::Shutdown::Both);
    }
}

/// True when this process should continue as the primary instance.
/// Uncertain IPC deliberately sacrifices uniqueness for availability. A link
/// is consumed only after a successful OpenUrl reply; we never retry a timed
/// out RPC. The remote may have acted before its reply was lost: exactly-once
/// delivery cannot be promised when the bus fails mid-call.
pub(crate) fn acquire_or_raise() -> bool {
    let mut pending = crate::deep_link_qt::take_pending();
    let result = zbus::Address::session()
        .map_err(|error| ProbeError {
            detail: format!("address: {error}"),
            quarantine_bus: false,
        })
        .and_then(|address| probe_at(&address, &mut pending, PROBE_TIMEOUT, SingleInstanceIface));
    if let Some(url) = pending {
        crate::deep_link_qt::restore_pending(url);
    }
    match result {
        Ok(Some(conn)) => {
            let _ = CONN.set(conn);
            true
        }
        Ok(None) => false,
        Err(error) => {
            log::warn!("[qbz-qt] single-instance probe failed ({error}); continuing");
            if error.quarantine_bus {
                // Before either graphics child or the main QGuiApplication:
                // QDesktopUnixServices synchronously opens QDBusConnection
                // during platform integration. Without this handoff, it can
                // repeat the AUTH/Hello hang we just bounded and canceled.
                // This affects only this process and its children; nothing is
                // persisted and the next launch retries the original bus.
                std::env::set_var("DBUS_SESSION_BUS_ADDRESS", UNAVAILABLE_BUS);
                log::warn!("[qbz-qt] D-Bus disabled for this launch before Qt startup");
            }
            true
        }
    }
}

/// zbus's address connector uses a blocking worker for Unix connect. Use
/// async-io directly so a full listen backlog cannot leave a worker behind.
/// Native Linux session buses use filesystem or abstract Unix sockets. Other
/// transports fail open immediately: this optional startup guard must not
/// spawn an autolaunch/SSH command or uncancelable DNS lookup.
fn socket_path(address: &zbus::Address) -> zbus::Result<std::path::PathBuf> {
    use std::os::unix::ffi::{OsStrExt, OsStringExt};
    use zbus::address::transport::{Transport, UnixSocket};
    match address.transport() {
        Transport::Unix(unix) => match unix.path() {
            UnixSocket::File(path) => Ok(path.clone()),
            UnixSocket::Abstract(name) => {
                let mut bytes = vec![0];
                bytes.extend_from_slice(name.as_bytes());
                // async-io interprets a leading NUL as an abstract name.
                Ok(std::ffi::OsString::from_vec(bytes).into())
            }
            _ => Err(zbus::Error::Unsupported),
        },
        _ => Err(zbus::Error::Unsupported),
    }
}

fn probe_at<I: zbus::object_server::Interface>(
    address: &zbus::Address,
    pending: &mut Option<String>,
    budget: Duration,
    interface: I,
) -> Result<Option<SessionConnection>, ProbeError> {
    let deadline = Instant::now() + budget;
    let phase = Cell::new(ProbePhase::Connect);
    let operation = async {
        let socket = Arc::new(Async::<UnixStream>::connect(socket_path(address)?).await?);
        let mut owned = SessionConnection {
            socket,
            connection: None,
        };
        phase.set(ProbePhase::Handshake);
        let conn = zbus::connection::Builder::socket(zbus::connection::socket::BoxedSplit::new(
            Box::new(owned.socket.clone()),
            Box::new(owned.socket.clone()),
        ))
        .serve_at(OBJECT_PATH, interface)?
        .build()
        .await?;
        if address
            .guid()
            .is_some_and(|expected| *expected != **conn.server_guid())
        {
            return Err(zbus::Error::Handshake("session bus GUID mismatch".into()));
        }
        owned.connection = Some(conn.clone());
        phase.set(ProbePhase::RequestName);
        // Raw calls avoid proxy property subscriptions and detached cleanup
        // RPCs. The sole timeout below includes every call and the handshake.
        let reply = conn
            .call_method(
                Some("org.freedesktop.DBus"),
                "/org/freedesktop/DBus",
                Some("org.freedesktop.DBus"),
                "RequestName",
                &(BUS_NAME, RequestNameFlags::DoNotQueue as u32),
            )
            .await?;
        match reply.body().deserialize::<RequestNameReply>()? {
            RequestNameReply::PrimaryOwner | RequestNameReply::AlreadyOwner => Ok(Some(owned)),
            RequestNameReply::Exists | RequestNameReply::InQueue => {
                if let Some(url) = pending.as_deref() {
                    phase.set(ProbePhase::OpenUrl);
                    conn.call_method(
                        Some(BUS_NAME),
                        OBJECT_PATH,
                        Some(IFACE_NAME),
                        "OpenUrl",
                        &url,
                    )
                    .await?;
                    // No await after confirmation: no late completion can
                    // consume a link that the caller has already recovered.
                    pending.take();
                    return Ok(None);
                }
                phase.set(ProbePhase::Present);
                if conn
                    .call_method(
                        Some(BUS_NAME),
                        OBJECT_PATH,
                        Some(IFACE_NAME),
                        "Present",
                        &(),
                    )
                    .await
                    .is_ok()
                {
                    return Ok(None);
                }
                phase.set(ProbePhase::Raise);
                conn.call_method(
                    Some("org.mpris.MediaPlayer2.com.blitzfc.qbz"),
                    "/org/mpris/MediaPlayer2",
                    Some("org.mpris.MediaPlayer2"),
                    "Raise",
                    &(),
                )
                .await?;
                Ok(None)
            }
        }
    };
    async_io::block_on(async {
        // Timer is polled first, including when both futures become ready in
        // the same turn. Dropping the losing future closes its socket guard.
        match select(Box::pin(Timer::at(deadline)), Box::pin(operation)).await {
            Either::Left((_, operation)) => {
                drop(operation);
                Err(phase.get().failure(
                    format!("total deadline of {} ms expired", budget.as_millis()),
                    None,
                ))
            }
            Either::Right((result, _)) => result
                .map_err(|error: zbus::Error| phase.get().failure(error.to_string(), Some(&error))),
        }
    })
}

#[cfg(test)]
#[path = "single_instance_tests.rs"]
mod tests;
