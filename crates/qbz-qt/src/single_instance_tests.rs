//! Synthetic buses use real wire messages, isolated sockets and no global env.
use super::*;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixListener;
use std::sync::Mutex;
use zbus::message::Message;
use zbus::zvariant::{serialized, Endian};

#[derive(Clone, Default)]
struct TestIface(Arc<Mutex<Vec<String>>>);

#[zbus::interface(name = "com.blitzfc.qbz.SingleInstance")]
impl TestIface {
    fn present(&self) {
        self.0.lock().unwrap().push("present".into());
    }
    fn open_url(&self, url: &str) {
        self.0.lock().unwrap().push(url.into());
    }
}

struct TempBusDir(std::path::PathBuf);
impl TempBusDir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("qbz bus {}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn address(&self) -> zbus::Address {
        use zbus::address::transport::{Transport, Unix, UnixSocket};
        zbus::Address::new(Transport::Unix(Unix::new(UnixSocket::File(
            self.0.join("bus"),
        ))))
    }
}
impl Drop for TempBusDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn read_message(reader: &mut BufReader<UnixStream>) -> Message {
    let mut bytes = vec![0; 16];
    reader.read_exact(&mut bytes).unwrap();
    let endian = if bytes[0] == b'l' {
        Endian::Little
    } else {
        Endian::Big
    };
    let number = |slice: &[u8]| {
        let bytes = slice.try_into().unwrap();
        if endian == Endian::Little {
            u32::from_le_bytes(bytes)
        } else {
            u32::from_be_bytes(bytes)
        }
    };
    let body = number(&bytes[4..8]) as usize;
    let fields = number(&bytes[12..16]) as usize;
    let size = (16 + fields + 7) / 8 * 8 + body;
    assert!(size <= 65536);
    bytes.resize(size, 0);
    reader.read_exact(&mut bytes[16..]).unwrap();
    // SAFETY: bytes come exclusively from zbus's serializer in this test;
    // no file descriptors are sent by any of these methods.
    unsafe {
        Message::from_bytes(serialized::Data::new(
            bytes,
            serialized::Context::new_dbus(endian, 0),
        ))
    }
    .unwrap()
}

fn reply(
    reader: &mut BufReader<UnixStream>,
    message: &Message,
    body: impl serde::Serialize + zbus::zvariant::DynamicType,
) {
    let response = Message::method_return(&message.header())
        .unwrap()
        .build(&body)
        .unwrap();
    reader.get_mut().write_all(response.data()).unwrap();
}

fn reject(reader: &mut BufReader<UnixStream>, message: &Message) {
    let response = Message::error(
        &message.header(),
        "org.freedesktop.DBus.Error.UnknownMethod",
    )
    .unwrap()
    .build(&"fixture rejection")
    .unwrap();
    reader.get_mut().write_all(response.data()).unwrap();
}

/// After the deadline, no bytes may follow and EOF must be observable. This
/// catches detached AUTH/RPC workers that a mere elapsed-time assertion misses.
fn assert_closed(reader: &mut BufReader<UnixStream>) {
    let mut byte = [0];
    assert_eq!(
        reader.read(&mut byte).unwrap(),
        0,
        "probe left a live socket or sent another call"
    );
}

fn stalled_phase(phase: &'static str, with_url: bool) {
    stalled_phase_after_delays(phase, with_url, Duration::ZERO, Duration::from_millis(150));
}

fn stalled_phase_after_delays(
    phase: &'static str,
    with_url: bool,
    delay: Duration,
    budget: Duration,
) {
    let dir = TempBusDir::new();
    let listener = UnixListener::bind(dir.0.join("bus")).unwrap();
    listener.set_nonblocking(true).unwrap();
    let worker = std::thread::spawn(move || {
        let start = Instant::now();
        let socket = loop {
            match listener.accept() {
                Ok((socket, _)) => break socket,
                Err(e)
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        && start.elapsed() < Duration::from_secs(2) =>
                {
                    std::thread::sleep(Duration::from_millis(2))
                }
                Err(e) => panic!("fixture accept: {e}"),
            }
        };
        socket
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut reader = BufReader::new(socket);
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        assert!(line.contains("AUTH EXTERNAL"));
        if phase == "AUTH" {
            assert_closed(&mut reader);
            return;
        }
        reader
            .get_mut()
            .write_all(b"OK 0123456789abcdef0123456789abcdef\r\n")
            .unwrap();
        loop {
            line.clear();
            reader.read_line(&mut line).unwrap();
            if line.starts_with("NEGOTIATE_UNIX_FD") {
                reader.get_mut().write_all(b"AGREE_UNIX_FD\r\n").unwrap();
            } else if line.starts_with("BEGIN") {
                break;
            } else {
                panic!("unexpected handshake: {line:?}");
            }
        }
        loop {
            let message = read_message(&mut reader);
            let header = message.header();
            let member = header.member().unwrap().as_str();
            if member == phase {
                assert_closed(&mut reader);
                // Even a reply arriving after cancellation cannot reactivate
                // the caller or cause a follow-up RPC: its transport is shut.
                let response = Message::method_return(&header).unwrap().build(&()).unwrap();
                let _ = reader.get_mut().write_all(response.data());
                return;
            }
            std::thread::sleep(delay);
            match member {
                "Hello" => reply(&mut reader, &message, ":1.42"),
                "RequestName" => reply(&mut reader, &message, 3u32),
                "Present" => reject(&mut reader, &message),
                other => panic!("unexpected fixture method: {other}"),
            }
        }
    });
    let mut url = with_url.then(|| "qobuzapp://album/fixture".to_owned());
    let before = url.clone();
    let started = Instant::now();
    let result = probe_at(&dir.address(), &mut url, budget, TestIface::default());
    let error = result.err().expect("stalled bus must fail open");
    assert!(error.detail.contains("deadline"), "{error}");
    assert_eq!(
        error.quarantine_bus,
        matches!(phase, "AUTH" | "Hello" | "RequestName"),
        "only a bus failure may disable D-Bus before Qt: {error}"
    );
    assert!(
        started.elapsed() < budget + Duration::from_millis(170),
        "{error}"
    );
    if !delay.is_zero() {
        assert!(
            error.detail.contains(phase),
            "must reach the final phase within the global budget: {error}"
        );
    }
    assert_eq!(url, before, "unconfirmed link was lost");
    worker.join().unwrap();
}

#[test]
fn auth_timeout_closes_socket_repeatedly() {
    for _ in 0..3 {
        stalled_phase("AUTH", true);
    }
}
#[test]
fn hello_timeout_closes_socket() {
    stalled_phase("Hello", true);
}
#[test]
fn request_name_timeout_closes_socket() {
    stalled_phase("RequestName", true);
}
#[test]
fn open_url_timeout_keeps_unconfirmed_link() {
    stalled_phase("OpenUrl", true);
}
#[test]
fn present_timeout_closes_socket() {
    stalled_phase("Present", false);
}
#[test]
fn raise_uses_the_same_deadline() {
    stalled_phase("Raise", false);
}

#[test]
fn slow_successful_calls_cannot_restart_the_final_rpc_budget() {
    stalled_phase_after_delays(
        "Raise",
        false,
        Duration::from_millis(80),
        Duration::from_millis(350),
    );
}

#[test]
fn missing_and_refused_bus_return_promptly() {
    let dir = TempBusDir::new();
    for refused in [false, true] {
        if refused {
            drop(UnixListener::bind(dir.0.join("bus")).unwrap());
        }
        let started = Instant::now();
        let mut link = Some("qobuzapp://track/1".into());
        let error = probe_at(
            &dir.address(),
            &mut link,
            PROBE_TIMEOUT,
            TestIface::default(),
        )
        .err()
        .expect("unavailable bus must fail open");
        assert!(error.quarantine_bus);
        assert!(started.elapsed() < Duration::from_secs(1));
        assert!(link.is_some());
    }
}

#[test]
fn unsupported_transport_does_not_launch_an_external_worker() {
    let address: zbus::Address = "tcp:host=unresolvable.invalid,port=1234"
        .try_into()
        .unwrap();
    assert!(socket_path(&address).is_err());
    let error = probe_at(&address, &mut None, PROBE_TIMEOUT, TestIface::default())
        .err()
        .expect("unsupported transport must fail open");
    assert!(!error.quarantine_bus, "Qt may support this transport");
}

#[test]
fn rejected_bus_method_does_not_disable_a_responsive_bus() {
    let error = zbus::Error::MethodError(
        "org.freedesktop.DBus.Error.AccessDenied"
            .try_into()
            .unwrap(),
        Some("fixture policy".into()),
        Message::signal("/fixture", "com.example.Fixture", "Test")
            .unwrap()
            .build(&())
            .unwrap(),
    );
    assert!(
        !ProbePhase::RequestName
            .failure(error.to_string(), Some(&error))
            .quarantine_bus
    );
}

#[test]
fn quarantined_bus_cannot_autolaunch_or_wait_for_auth() {
    let address = UNAVAILABLE_BUS.try_into().unwrap();
    let started = Instant::now();
    let error = probe_at(&address, &mut None, PROBE_TIMEOUT, TestIface::default())
        .err()
        .expect("quarantined bus must fail immediately");
    assert!(error.quarantine_bus);
    assert!(started.elapsed() < Duration::from_secs(1));
    assert!(error.detail.starts_with("connect:"));
}

struct RealBus {
    process: std::process::Child,
    _dir: TempBusDir,
    address: zbus::Address,
}
impl RealBus {
    fn new() -> Self {
        Self::with_abstract_socket(false)
    }
    fn with_abstract_socket(abstract_socket: bool) -> Self {
        let dir = TempBusDir::new();
        let address = if abstract_socket {
            format!("unix:abstract=qbz-test-{}", uuid::Uuid::new_v4())
                .as_str()
                .try_into()
                .unwrap()
        } else {
            dir.address()
        };
        let mut process = std::process::Command::new("dbus-daemon")
            .args(["--session", "--nofork", "--print-address=1"])
            .arg(format!("--address={address}"))
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("dbus-daemon is required by the Linux Qt gate");
        let mut ready = String::new();
        BufReader::new(process.stdout.take().unwrap())
            .read_line(&mut ready)
            .unwrap();
        if !ready.starts_with("unix:") {
            let _ = process.kill();
            let _ = process.wait();
            panic!("private dbus-daemon did not publish its address");
        }
        Self {
            process,
            _dir: dir,
            address,
        }
    }
}
impl Drop for RealBus {
    fn drop(&mut self) {
        let _ = self.process.kill();
        let _ = self.process.wait();
    }
}

#[test]
fn primary_keeps_server_alive_and_secondaries_forward_once() {
    let bus = RealBus::new();
    let interface = TestIface::default();
    let primary = probe_at(&bus.address, &mut None, PROBE_TIMEOUT, interface.clone())
        .unwrap()
        .unwrap();
    for link in [Some("qobuzapp://album/one".to_string()), None] {
        let mut pending = link;
        assert!(probe_at(
            &bus.address,
            &mut pending,
            PROBE_TIMEOUT,
            TestIface::default()
        )
        .unwrap()
        .is_none());
        assert!(pending.is_none());
    }
    assert_eq!(
        *interface.0.lock().unwrap(),
        ["qobuzapp://album/one", "present"]
    );
    drop(primary);
    assert!(
        probe_at(&bus.address, &mut None, PROBE_TIMEOUT, TestIface::default())
            .unwrap()
            .is_some()
    );
}

#[test]
fn native_abstract_session_bus_keeps_ownership() {
    let bus = RealBus::with_abstract_socket(true);
    let interface = TestIface::default();
    let primary = probe_at(&bus.address, &mut None, PROBE_TIMEOUT, interface.clone())
        .unwrap()
        .unwrap();
    assert!(
        probe_at(&bus.address, &mut None, PROBE_TIMEOUT, TestIface::default())
            .unwrap()
            .is_none()
    );
    assert_eq!(*interface.0.lock().unwrap(), ["present"]);
    drop(primary);
}

#[test]
fn early_handoff_survives_ui_binding_in_an_isolated_process() {
    const CHILD: &str = "QBZ_TEST_EARLY_HANDOFF";
    if std::env::var(CHILD).as_deref() == Ok("1") {
        // Production interface + pending-link/controller state, without any
        // Qt window, authenticated session, keyring or global state of other
        // tests. The actual Qt-thread hop is checked by the release smoke.
        let interface = SingleInstanceIface;
        interface.open_url("qobuzapp://album/before-bind");
        assert!(!UI_READY.load(Ordering::SeqCst));
        assert!(PENDING_PRESENT.load(Ordering::SeqCst));
        assert_eq!(
            crate::deep_link_qt::take_pending().as_deref(),
            Some("qobuzapp://album/before-bind")
        );
        crate::deep_link_qt::restore_pending("qobuzapp://album/unconfirmed".into());
        crate::deep_link_qt::stash("qobuzapp://album/newer".into());
        crate::deep_link_qt::restore_pending("qobuzapp://album/older".into());
        bind_ui();
        assert!(UI_READY.load(Ordering::SeqCst));
        assert!(!PENDING_PRESENT.load(Ordering::SeqCst));
        assert_eq!(
            crate::deep_link_qt::take_pending().as_deref(),
            Some("qobuzapp://album/newer")
        );
        for shown in [false, true] {
            crate::tray_qt::set_window_shown(shown);
            interface.present();
            assert!(!PENDING_PRESENT.load(Ordering::SeqCst));
            assert!(crate::deep_link_qt::take_pending().is_none());
        }
        return;
    }
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "single_instance_qt::tests::early_handoff_survives_ui_binding_in_an_isolated_process",
            "--nocapture",
        ])
        .env(CHILD, "1")
        .spawn()
        .unwrap();
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success());
            break;
        }
        if started.elapsed() > Duration::from_secs(5) {
            let _ = child.kill();
            let _ = child.wait();
            panic!("isolated handoff controller test did not terminate");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}
