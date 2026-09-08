use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use socket2::{Domain, Protocol, Socket, Type};
use zeroize::Zeroizing;

use crate::admission::{admission_channel, AdmissionInbox, AdmissionSender, SubmitError};
use crate::mdns::{MdnsRegistration, MdnsShutdownHandle};
use crate::validation::{parse_and_validate, EndpointPolicy, ValidationError, MAX_BODY_BYTES};
use crate::LanProjection;

pub const SERVICE_TYPE: &str = "_qobuz-connect._tcp.local.";
pub const DEFAULT_SDK_VERSION: &str = "0.9.5";

const POST_RATE_PER_MINUTE: f64 = 6.0;
const POST_BURST: f64 = 2.0;
const MAX_RATE_ORIGINS: usize = 256;
const HTTP_WORKERS: usize = 2;
const MAX_HEADER_BYTES: usize = 16 * 1024;
const ACCEPT_WAKE_TIMEOUT: Duration = Duration::from_millis(250);
const REJECTION_DRAIN_TIMEOUT: Duration = Duration::from_millis(25);
pub const DEFAULT_LOCAL_READ_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LanHttpMethod {
    Get,
    Post,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LanHttpRoute {
    GetDisplayInfo,
    GetConnectInfo,
    ConnectToQconnect,
    Unknown,
}

/// Sanitized, opt-in request observation for protocol probes and diagnostics.
/// It deliberately excludes remote addresses, headers, URLs and bodies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LanRequestObservation {
    pub method: LanHttpMethod,
    pub route: LanHttpRoute,
    pub status_code: u16,
    pub validation_error: Option<ValidationError>,
}

#[derive(Debug, thiserror::Error)]
pub enum LanError {
    #[error("qconnect-lan-bind")]
    Bind,
    #[error("qconnect-lan-address-discovery")]
    AddressDiscovery,
    #[error("qconnect-lan-no-addresses")]
    NoLanAddresses,
    #[error("qconnect-lan-mdns")]
    Mdns,
    #[error("qconnect-lan-config")]
    InvalidConfig,
}

impl From<mdns_sd::Error> for LanError {
    fn from(_: mdns_sd::Error) -> Self {
        Self::Mdns
    }
}

#[derive(Clone)]
pub struct LanServiceConfig {
    pub bind_addr: SocketAddr,
    pub projection: LanProjection,
    pub endpoint_policy: EndpointPolicy,
    pub device_uuid: String,
    pub sdk_version: String,
    /// Bounds the complete local read and parse operation, including clients
    /// that trickle bytes without ever going idle.
    pub read_timeout: Duration,
    /// Tests and environments without multicast can exercise the exact HTTP
    /// wire while keeping discovery disabled. Production adapters leave true.
    pub advertise_mdns: bool,
    /// Optional explicit address set, primarily for deterministic integration
    /// tests. Production selects the interface routed to mDNS multicast.
    pub advertised_addresses: Option<Vec<IpAddr>>,
    /// Optional bounded diagnostics sink. Events are best-effort and dropped
    /// when the receiver is busy or disconnected.
    pub request_observer: Option<mpsc::SyncSender<LanRequestObservation>>,
}

impl LanServiceConfig {
    pub fn new(
        projection: LanProjection,
        endpoint_policy: EndpointPolicy,
        device_uuid: impl Into<String>,
    ) -> Self {
        Self {
            bind_addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0),
            projection,
            endpoint_policy,
            device_uuid: device_uuid.into(),
            sdk_version: DEFAULT_SDK_VERSION.to_string(),
            read_timeout: DEFAULT_LOCAL_READ_TIMEOUT,
            advertise_mdns: true,
            advertised_addresses: None,
            request_observer: None,
        }
    }
}

pub struct LanService {
    port: u16,
    lifecycle: Arc<ServiceLifecycle>,
    accept_thread: Option<JoinHandle<()>>,
    worker_threads: Vec<JoinHandle<()>>,
    mdns: Option<MdnsRegistration>,
}

struct InFlightPermit {
    count: Arc<AtomicUsize>,
}

impl InFlightPermit {
    fn try_acquire(count: Arc<AtomicUsize>) -> Option<Self> {
        count
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                (current < HTTP_WORKERS).then_some(current + 1)
            })
            .ok()
            .map(|_| Self { count })
    }
}

impl Drop for InFlightPermit {
    fn drop(&mut self) {
        self.count.fetch_sub(1, Ordering::AcqRel);
    }
}

#[derive(Default)]
struct ActiveConnections {
    next_id: AtomicUsize,
    // Cloned handles exist only so shutdown can interrupt reads in workers.
    // The worker-owned stream remains the sole reader and writer.
    streams: Mutex<HashMap<usize, TcpStream>>,
}

impl ActiveConnections {
    fn insert(&self, stream: TcpStream) -> usize {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        recover_lock(&self.streams).insert(id, stream);
        id
    }

    fn remove(&self, id: usize) {
        recover_lock(&self.streams).remove(&id);
    }

    fn shutdown_all(&self) {
        for stream in recover_lock(&self.streams).values() {
            let _ = stream.shutdown(Shutdown::Both);
        }
    }
}

struct ServiceLifecycle {
    running: AtomicBool,
    admission: AdmissionSender,
    accept_wake_addr: SocketAddr,
    active_connections: Arc<ActiveConnections>,
    discovery: Mutex<Option<MdnsShutdownHandle>>,
}

impl ServiceLifecycle {
    fn new(
        admission: AdmissionSender,
        accept_wake_addr: SocketAddr,
        active_connections: Arc<ActiveConnections>,
    ) -> Self {
        Self {
            running: AtomicBool::new(true),
            admission,
            accept_wake_addr,
            active_connections,
            discovery: Mutex::new(None),
        }
    }

    fn is_running(&self) -> bool {
        self.running.load(Ordering::Acquire)
    }

    fn install_discovery(&self, discovery: MdnsShutdownHandle) {
        let stop_immediately = {
            let mut slot = recover_lock(&self.discovery);
            if self.is_running() {
                *slot = Some(discovery.clone());
                false
            } else {
                true
            }
        };
        if stop_immediately {
            discovery.shutdown();
        }
    }

    /// One fail-stop edge owns teardown. Every later caller observes the
    /// fenced state and cannot revive admission or discovery.
    fn stop(&self) {
        if !self.running.swap(false, Ordering::AcqRel) {
            return;
        }
        self.admission.close();
        // Withdraw discovery before interrupting the endpoint. This handle is
        // also called from unexpected thread-exit guards, not only Drop.
        let discovery = recover_lock(&self.discovery).clone();
        if let Some(discovery) = discovery {
            discovery.shutdown();
        }
        self.active_connections.shutdown_all();
        wake_acceptor(self.accept_wake_addr);
    }
}

struct UnexpectedThreadExit {
    lifecycle: Arc<ServiceLifecycle>,
}

impl UnexpectedThreadExit {
    fn new(lifecycle: Arc<ServiceLifecycle>) -> Self {
        Self { lifecycle }
    }
}

impl Drop for UnexpectedThreadExit {
    fn drop(&mut self) {
        self.lifecycle.stop();
    }
}

struct ConnectionJob {
    stream: TcpStream,
    active_connections: Arc<ActiveConnections>,
    active_id: usize,
    _permit: InFlightPermit,
}

impl Drop for ConnectionJob {
    fn drop(&mut self) {
        self.active_connections.remove(self.active_id);
    }
}

impl LanService {
    /// Bind first, spawn the fixed HTTP worker set, then publish DNS-SD. On
    /// any discovery failure the listener is torn down before returning.
    pub fn start(config: LanServiceConfig) -> Result<(Self, AdmissionInbox), LanError> {
        validate_config(&config)?;
        let listener = bind_http(config.bind_addr)?;
        let listener_addr = listener.local_addr().map_err(|_| LanError::Bind)?;
        let port = listener_addr.port();
        let accept_wake_addr = accept_wake_addr(listener_addr);

        let (admission, inbox) = admission_channel();
        let rate_limiter = Arc::new(Mutex::new(PostRateLimiter::default()));
        let (request_tx, request_rx) = mpsc::sync_channel::<ConnectionJob>(HTTP_WORKERS);
        let request_rx = Arc::new(Mutex::new(request_rx));
        let in_flight = Arc::new(AtomicUsize::new(0));
        let active_connections = Arc::new(ActiveConnections::default());
        let lifecycle = Arc::new(ServiceLifecycle::new(
            admission.clone(),
            accept_wake_addr,
            Arc::clone(&active_connections),
        ));

        // Discovery is prepared and its fail-stop handle installed before any
        // supervised HTTP thread starts. Publication itself remains last, once
        // the endpoint can actually serve official-client requests.
        let mut mdns = if config.advertise_mdns {
            let advertised_addresses = config.advertised_addresses.clone().or_else(|| {
                (!config.bind_addr.ip().is_unspecified()).then(|| vec![config.bind_addr.ip()])
            });
            let registration = MdnsRegistration::prepare(
                &config.device_uuid,
                port,
                &config.sdk_version,
                advertised_addresses,
            )?;
            lifecycle.install_discovery(registration.shutdown_handle());
            Some(registration)
        } else {
            None
        };

        let mut worker_threads = Vec::with_capacity(HTTP_WORKERS);
        for _ in 0..HTTP_WORKERS {
            let worker_rx = Arc::clone(&request_rx);
            let worker_lifecycle = Arc::clone(&lifecycle);
            let worker_admission = admission.clone();
            let worker_projection = config.projection.clone();
            let worker_policy = config.endpoint_policy.clone();
            let worker_limiter = Arc::clone(&rate_limiter);
            let worker_observer = config.request_observer.clone();
            let read_timeout = config.read_timeout;
            worker_threads.push(std::thread::spawn(move || {
                let _exit_guard = UnexpectedThreadExit::new(Arc::clone(&worker_lifecycle));
                loop {
                    let job = {
                        let receiver = recover_lock(&worker_rx);
                        receiver.recv()
                    };
                    let mut job = match job {
                        Ok(job) => job,
                        Err(_) => break,
                    };
                    if worker_lifecycle.is_running() {
                        handle_connection(
                            &mut job.stream,
                            read_timeout,
                            &worker_projection,
                            &worker_policy,
                            &worker_admission,
                            &worker_limiter,
                            worker_observer.as_ref(),
                        );
                    } else {
                        let _ =
                            write_response(&mut job.stream, &error_response(503, "unavailable"));
                    }
                }
            }));
        }

        let accept_lifecycle = Arc::clone(&lifecycle);
        let accept_in_flight = Arc::clone(&in_flight);
        let accept_active_connections = Arc::clone(&active_connections);
        let accept_observer = config.request_observer.clone();
        // The listener deliberately has no SO_RCVTIMEO: that option also
        // times out accept() on Linux. Shutdown wakes this blocking accept
        // with one bounded local connection after fencing the lifecycle.
        let accept_thread = std::thread::spawn(move || {
            let _exit_guard = UnexpectedThreadExit::new(Arc::clone(&accept_lifecycle));
            loop {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        if !accept_lifecycle.is_running() {
                            break;
                        }
                        let Some(permit) =
                            InFlightPermit::try_acquire(Arc::clone(&accept_in_flight))
                        else {
                            observe_status(accept_observer.as_ref(), 429);
                            reject_connection(&mut stream, &error_response(429, "busy"));
                            continue;
                        };
                        let shutdown_stream = match stream.try_clone() {
                            Ok(stream) => stream,
                            Err(_) => {
                                drop(permit);
                                observe_status(accept_observer.as_ref(), 503);
                                reject_connection(&mut stream, &error_response(503, "unavailable"));
                                continue;
                            }
                        };
                        let active_id = accept_active_connections.insert(shutdown_stream);
                        let job = ConnectionJob {
                            stream,
                            active_connections: Arc::clone(&accept_active_connections),
                            active_id,
                            _permit: permit,
                        };
                        match request_tx.try_send(job) {
                            Ok(()) => {}
                            Err(TrySendError::Full(mut job)) => {
                                observe_status(accept_observer.as_ref(), 429);
                                reject_connection(&mut job.stream, &error_response(429, "busy"));
                            }
                            Err(TrySendError::Disconnected(mut job)) => {
                                observe_status(accept_observer.as_ref(), 503);
                                reject_connection(
                                    &mut job.stream,
                                    &error_response(503, "unavailable"),
                                );
                                accept_lifecycle.stop();
                                break;
                            }
                        }
                    }
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Err(_) if !accept_lifecycle.is_running() => break,
                    Err(_) => {
                        accept_lifecycle.stop();
                        break;
                    }
                }
            }
        });

        if let Some(registration) = mdns.as_mut() {
            let weak_lifecycle = Arc::downgrade(&lifecycle);
            if let Err(error) = registration.publish(move || {
                if let Some(lifecycle) = weak_lifecycle.upgrade() {
                    lifecycle.stop();
                }
            }) {
                lifecycle.stop();
                registration.shutdown();
                let _ = accept_thread.join();
                for worker in worker_threads {
                    let _ = worker.join();
                }
                return Err(error);
            }
        }
        if !lifecycle.is_running() {
            lifecycle.stop();
            if let Some(registration) = mdns.as_mut() {
                registration.shutdown();
            }
            let _ = accept_thread.join();
            for worker in worker_threads {
                let _ = worker.join();
            }
            return Err(LanError::Mdns);
        }

        Ok((
            Self {
                port,
                lifecycle,
                accept_thread: Some(accept_thread),
                worker_threads,
                mdns,
            },
            inbox,
        ))
    }

    pub const fn port(&self) -> u16 {
        self.port
    }

    /// False after explicit shutdown or any supervised HTTP/mDNS fatal exit.
    pub fn is_running(&self) -> bool {
        self.lifecycle.is_running()
    }

    pub fn shutdown(&mut self) {
        self.lifecycle.stop();
        if let Some(mut mdns) = self.mdns.take() {
            mdns.shutdown();
        }
        if let Some(thread) = self.accept_thread.take() {
            let _ = thread.join();
        }
        for thread in self.worker_threads.drain(..) {
            let _ = thread.join();
        }
    }
}

impl Drop for LanService {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn validate_config(config: &LanServiceConfig) -> Result<(), LanError> {
    let display = config.projection.display_info();
    let connect = config.projection.connect_info();
    let required = [
        display.friendly_name.as_str(),
        display.serial_number.as_str(),
        display.brand_display_name.as_str(),
        display.model_display_name.as_str(),
        display.software_version.as_str(),
        connect.app_id.as_str(),
        config.device_uuid.as_str(),
        config.sdk_version.as_str(),
    ];
    if config.bind_addr.is_ipv6()
        || config.read_timeout.is_zero()
        || required.iter().any(|value| value.trim().is_empty())
        || display.serial_number != config.device_uuid
    {
        return Err(LanError::InvalidConfig);
    }
    if config.advertise_mdns {
        let bind_ip = config.bind_addr.ip();
        if bind_ip.is_loopback() {
            return Err(LanError::InvalidConfig);
        }
        if let Some(addresses) = &config.advertised_addresses {
            if addresses.is_empty()
                || addresses.iter().any(|address| {
                    !address.is_ipv4()
                        || address.is_loopback()
                        || address.is_unspecified()
                        || (!bind_ip.is_unspecified() && *address != bind_ip)
                })
            {
                return Err(LanError::InvalidConfig);
            }
        }
    }
    Ok(())
}

fn bind_http(bind_addr: SocketAddr) -> Result<TcpListener, LanError> {
    let domain = if bind_addr.is_ipv4() {
        Domain::IPV4
    } else {
        Domain::IPV6
    };
    let socket =
        Socket::new(domain, Type::STREAM, Some(Protocol::TCP)).map_err(|_| LanError::Bind)?;
    socket.set_reuse_address(true).map_err(|_| LanError::Bind)?;
    if bind_addr.is_ipv6() {
        socket.set_only_v6(true).map_err(|_| LanError::Bind)?;
    }
    socket.bind(&bind_addr.into()).map_err(|_| LanError::Bind)?;
    socket.listen(128).map_err(|_| LanError::Bind)?;
    Ok(socket.into())
}

fn accept_wake_addr(listener_addr: SocketAddr) -> SocketAddr {
    if !listener_addr.ip().is_unspecified() {
        return listener_addr;
    }
    let loopback = if listener_addr.is_ipv4() {
        IpAddr::V4(Ipv4Addr::LOCALHOST)
    } else {
        IpAddr::V6(Ipv6Addr::LOCALHOST)
    };
    SocketAddr::new(loopback, listener_addr.port())
}

fn wake_acceptor(address: SocketAddr) {
    if let Ok(stream) = TcpStream::connect_timeout(&address, ACCEPT_WAKE_TIMEOUT) {
        let _ = stream.shutdown(Shutdown::Both);
    }
}

fn recover_lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReadRequestError {
    Invalid,
    TooLarge,
    TimedOut,
    ExpectationFailed,
}

struct ParsedRequest {
    method: LanHttpMethod,
    path: String,
    content_type_json: bool,
    body: Zeroizing<Vec<u8>>,
    remote_ip: Option<IpAddr>,
}

fn handle_connection(
    stream: &mut TcpStream,
    read_timeout: Duration,
    projection: &LanProjection,
    policy: &EndpointPolicy,
    admission: &AdmissionSender,
    rate_limiter: &Arc<Mutex<PostRateLimiter>>,
    observer: Option<&mpsc::SyncSender<LanRequestObservation>>,
) {
    let _ = stream.set_write_timeout(Some(read_timeout));
    let request = match read_request(stream, read_timeout) {
        Ok(request) => request,
        Err(error) => {
            let status = match error {
                ReadRequestError::TooLarge => 413,
                ReadRequestError::Invalid | ReadRequestError::TimedOut => 400,
                ReadRequestError::ExpectationFailed => 417,
            };
            let code = match error {
                ReadRequestError::TooLarge => "request_too_large",
                ReadRequestError::Invalid => "invalid_request",
                ReadRequestError::TimedOut => "request_timeout",
                ReadRequestError::ExpectationFailed => "expectation_failed",
            };
            observe_status(observer, status);
            let _ = write_response(stream, &error_response(status, code));
            return;
        }
    };
    let observed_route = route_for(&request.path);
    let mut validation_error = None;
    let response = handle_request(
        &request,
        projection,
        policy,
        admission,
        rate_limiter,
        &mut validation_error,
    );
    if let Some(observer) = observer {
        let _ = observer.try_send(LanRequestObservation {
            method: request.method,
            route: observed_route,
            status_code: response.status_code,
            validation_error,
        });
    }
    let _ = write_response(stream, &response);
}

fn observe_status(observer: Option<&mpsc::SyncSender<LanRequestObservation>>, status_code: u16) {
    if let Some(observer) = observer {
        let _ = observer.try_send(LanRequestObservation {
            method: LanHttpMethod::Other,
            route: LanHttpRoute::Unknown,
            status_code,
            validation_error: None,
        });
    }
}

fn read_request(
    stream: &mut TcpStream,
    timeout: Duration,
) -> Result<ParsedRequest, ReadRequestError> {
    let deadline = Instant::now()
        .checked_add(timeout)
        .ok_or(ReadRequestError::Invalid)?;
    let mut bytes = Zeroizing::new(Vec::with_capacity(4096));
    let header_end = loop {
        if let Some(end) = find_header_end(&bytes) {
            if end > MAX_HEADER_BYTES {
                return Err(ReadRequestError::TooLarge);
            }
            break end;
        }
        if bytes.len() >= MAX_HEADER_BYTES {
            return Err(ReadRequestError::TooLarge);
        }
        read_more(stream, &mut bytes, deadline)?;
    };

    let head =
        std::str::from_utf8(&bytes[..header_end - 4]).map_err(|_| ReadRequestError::Invalid)?;
    let mut lines = head.split("\r\n");
    let mut request_line = lines
        .next()
        .ok_or(ReadRequestError::Invalid)?
        .split_ascii_whitespace();
    let method = match request_line.next().ok_or(ReadRequestError::Invalid)? {
        "GET" => LanHttpMethod::Get,
        "POST" => LanHttpMethod::Post,
        _ => LanHttpMethod::Other,
    };
    let path = request_line
        .next()
        .filter(|path| path.starts_with('/'))
        .ok_or(ReadRequestError::Invalid)?
        .to_string();
    let version = request_line.next().ok_or(ReadRequestError::Invalid)?;
    if request_line.next().is_some() || !matches!(version, "HTTP/1.0" | "HTTP/1.1") {
        return Err(ReadRequestError::Invalid);
    }

    let mut content_length = None;
    let mut content_type_json = false;
    for line in lines {
        let (name, value) = line.split_once(':').ok_or(ReadRequestError::Invalid)?;
        let name = name.trim();
        let value = value.trim();
        if name.eq_ignore_ascii_case("content-length") {
            if content_length.is_some() {
                return Err(ReadRequestError::Invalid);
            }
            content_length = Some(
                value
                    .parse::<usize>()
                    .map_err(|_| ReadRequestError::Invalid)?,
            );
        } else if name.eq_ignore_ascii_case("content-type") {
            content_type_json = value
                .split(';')
                .next()
                .is_some_and(|value| value.trim().eq_ignore_ascii_case("application/json"));
        } else if name.eq_ignore_ascii_case("transfer-encoding") {
            return Err(ReadRequestError::Invalid);
        } else if name.eq_ignore_ascii_case("expect") {
            return Err(ReadRequestError::ExpectationFailed);
        }
    }

    let body_len = content_length.unwrap_or(0);
    if body_len > MAX_BODY_BYTES {
        return Err(ReadRequestError::TooLarge);
    }
    let total_len = header_end
        .checked_add(body_len)
        .ok_or(ReadRequestError::TooLarge)?;
    while bytes.len() < total_len {
        read_more(stream, &mut bytes, deadline)?;
    }
    if bytes.len() > total_len {
        bytes[total_len..].fill(0);
    }
    bytes.truncate(total_len);
    let body = Zeroizing::new(bytes[header_end..].to_vec());

    Ok(ParsedRequest {
        method,
        path,
        content_type_json,
        body,
        remote_ip: stream.peer_addr().ok().map(|address| address.ip()),
    })
}

fn read_more(
    stream: &mut TcpStream,
    bytes: &mut Vec<u8>,
    deadline: Instant,
) -> Result<(), ReadRequestError> {
    let remaining = deadline
        .checked_duration_since(Instant::now())
        .filter(|remaining| !remaining.is_zero())
        .ok_or(ReadRequestError::TimedOut)?;
    stream
        .set_read_timeout(Some(remaining))
        .map_err(|_| ReadRequestError::Invalid)?;
    let mut chunk = Zeroizing::new([0_u8; 4096]);
    match stream.read(&mut chunk[..]) {
        Ok(0) => Err(ReadRequestError::Invalid),
        Ok(count) => {
            bytes.extend_from_slice(&chunk[..count]);
            Ok(())
        }
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
            ) =>
        {
            Err(ReadRequestError::TimedOut)
        }
        Err(_) => Err(ReadRequestError::Invalid),
    }
}

fn find_header_end(bytes: &[u8]) -> Option<usize> {
    bytes
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|position| position + 4)
}

fn route_for(path: &str) -> LanHttpRoute {
    match path {
        "/get-display-info" => LanHttpRoute::GetDisplayInfo,
        "/get-connect-info" => LanHttpRoute::GetConnectInfo,
        "/connect-to-qconnect" => LanHttpRoute::ConnectToQconnect,
        _ => LanHttpRoute::Unknown,
    }
}

fn handle_request(
    request: &ParsedRequest,
    projection: &LanProjection,
    policy: &EndpointPolicy,
    admission: &AdmissionSender,
    rate_limiter: &Arc<Mutex<PostRateLimiter>>,
    validation_error: &mut Option<ValidationError>,
) -> HttpResponse {
    match (route_for(&request.path), request.method) {
        (LanHttpRoute::GetDisplayInfo, LanHttpMethod::Get) => {
            json_response(200, &projection.display_info())
        }
        (LanHttpRoute::GetConnectInfo, LanHttpMethod::Get) => {
            json_response(200, &projection.connect_info())
        }
        (LanHttpRoute::ConnectToQconnect, LanHttpMethod::Post) => {
            handle_connect(request, policy, admission, rate_limiter, validation_error)
        }
        (LanHttpRoute::Unknown, _) => error_response(404, "not_found"),
        _ => error_response(405, "method_not_allowed"),
    }
}

fn handle_connect(
    request: &ParsedRequest,
    policy: &EndpointPolicy,
    admission: &AdmissionSender,
    rate_limiter: &Arc<Mutex<PostRateLimiter>>,
    validation_error: &mut Option<ValidationError>,
) -> HttpResponse {
    if !request.content_type_json {
        return error_response(415, "json_required");
    }
    if !recover_lock(rate_limiter).allow(request.remote_ip, Instant::now()) {
        return error_response(429, "rate_limited");
    }

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .min(i64::MAX as u64) as i64;
    let candidate = match parse_and_validate(&request.body, policy, now) {
        Ok(candidate) => candidate,
        Err(error @ ValidationError::BecomeActiveRequired) => {
            *validation_error = Some(error);
            return error_response(422, "active_required");
        }
        Err(error) => {
            *validation_error = Some(error);
            return error_response(400, "invalid_handoff");
        }
    };

    match admission.submit(candidate) {
        Ok(()) => json_response(200, &serde_json::json!({})),
        Err(SubmitError::Closed) => error_response(503, "unavailable"),
    }
}

struct HttpResponse {
    status_code: u16,
    body: Vec<u8>,
}

fn json_response<T: serde::Serialize>(status_code: u16, value: &T) -> HttpResponse {
    HttpResponse {
        status_code,
        body: serde_json::to_vec(value).unwrap_or_else(|_| b"{}".to_vec()),
    }
}

fn error_response(status_code: u16, code: &'static str) -> HttpResponse {
    json_response(status_code, &serde_json::json!({ "error": code }))
}

fn write_response(stream: &mut TcpStream, response: &HttpResponse) -> io::Result<()> {
    let reason = match response.status_code {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        413 => "Payload Too Large",
        415 => "Unsupported Media Type",
        417 => "Expectation Failed",
        422 => "Unprocessable Entity",
        429 => "Too Many Requests",
        503 => "Service Unavailable",
        _ => "Error",
    };
    write!(
        stream,
        "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        response.status_code,
        reason,
        response.body.len()
    )?;
    stream.write_all(&response.body)?;
    stream.flush()
}

/// Send a bounded overload/shutdown response without immediately closing a
/// socket that still has unread request bytes. On Linux, closing with unread
/// bytes emits a reset that can hide an otherwise complete 429 from strict
/// clients. Read at most one declared request under an absolute 25 ms deadline,
/// then respond and close the write half. A trickling peer therefore cannot
/// retain the accept thread indefinitely, while a normal official POST gets a
/// complete HTTP response and clean EOF.
fn reject_connection(stream: &mut TcpStream, response: &HttpResponse) {
    let Some(deadline) = Instant::now().checked_add(REJECTION_DRAIN_TIMEOUT) else {
        return;
    };
    let drain_cap = MAX_HEADER_BYTES.saturating_add(MAX_BODY_BYTES);
    let mut drained = Zeroizing::new(Vec::with_capacity(1024));
    let mut scratch = Zeroizing::new([0_u8; 1024]);
    while drained.len() < drain_cap {
        let Some(remaining_time) = deadline
            .checked_duration_since(Instant::now())
            .filter(|remaining| !remaining.is_zero())
        else {
            break;
        };
        let _ = stream.set_read_timeout(Some(remaining_time));
        let remaining_bytes = (drain_cap - drained.len()).min(scratch.len());
        match stream.read(&mut scratch[..remaining_bytes]) {
            Ok(0) => break,
            Ok(count) => {
                drained.extend_from_slice(&scratch[..count]);
                if rejected_request_is_complete(&drained) {
                    break;
                }
            }
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                break;
            }
            Err(_) => break,
        }
    }

    let _ = stream.set_write_timeout(Some(Duration::from_secs(1)));
    let _ = write_response(stream, response);
    let _ = stream.shutdown(Shutdown::Write);
}

fn rejected_request_is_complete(bytes: &[u8]) -> bool {
    let Some(header_end) = find_header_end(bytes) else {
        return false;
    };
    let Ok(head) = std::str::from_utf8(&bytes[..header_end.saturating_sub(4)]) else {
        return true;
    };
    let mut content_length = 0_usize;
    for line in head.split("\r\n").skip(1) {
        let Some((name, value)) = line.split_once(':') else {
            return true;
        };
        if name.trim().eq_ignore_ascii_case("content-length") {
            let Ok(parsed) = value.trim().parse::<usize>() else {
                return true;
            };
            content_length = parsed.min(MAX_BODY_BYTES);
            break;
        }
    }
    bytes.len() >= header_end.saturating_add(content_length)
}

#[derive(Debug, Clone, Copy)]
struct RateBucket {
    tokens: f64,
    updated_at: Instant,
}

#[derive(Default)]
struct PostRateLimiter {
    buckets: HashMap<Option<IpAddr>, RateBucket>,
}

impl PostRateLimiter {
    fn allow(&mut self, origin: Option<IpAddr>, now: Instant) -> bool {
        if self.buckets.len() >= MAX_RATE_ORIGINS && !self.buckets.contains_key(&origin) {
            if let Some(oldest) = self
                .buckets
                .iter()
                .min_by_key(|(_, bucket)| bucket.updated_at)
                .map(|(origin, _)| *origin)
            {
                self.buckets.remove(&oldest);
            }
        }
        let bucket = self.buckets.entry(origin).or_insert(RateBucket {
            tokens: POST_BURST,
            updated_at: now,
        });
        let elapsed = now
            .saturating_duration_since(bucket.updated_at)
            .as_secs_f64();
        bucket.tokens = (bucket.tokens + elapsed * POST_RATE_PER_MINUTE / 60.0).min(POST_BURST);
        bucket.updated_at = now;
        if bucket.tokens < 1.0 {
            return false;
        }
        bucket.tokens -= 1.0;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn post_rate_limit_allows_burst_then_refills() {
        let mut limiter = PostRateLimiter::default();
        let now = Instant::now();
        let ip = Some(IpAddr::V4(Ipv4Addr::LOCALHOST));
        assert!(limiter.allow(ip, now));
        assert!(limiter.allow(ip, now));
        assert!(!limiter.allow(ip, now));
        assert!(limiter.allow(ip, now + Duration::from_secs(10)));
    }

    #[test]
    fn finds_only_complete_crlf_header_boundary() {
        assert_eq!(find_header_end(b"GET / HTTP/1.1\r\n\r\nbody"), Some(18));
        assert_eq!(find_header_end(b"GET / HTTP/1.1\n\n"), None);
    }

    #[test]
    fn in_flight_permit_is_released_during_unwind() {
        let count = Arc::new(AtomicUsize::new(0));
        let unwind_count = Arc::clone(&count);
        let result = std::panic::catch_unwind(move || {
            let _permit = InFlightPermit::try_acquire(unwind_count).unwrap();
            panic!("intentional permit-unwind regression");
        });

        assert!(result.is_err());
        assert_eq!(count.load(Ordering::Acquire), 0);
    }

    #[test]
    fn poisoned_server_lock_remains_recoverable() {
        let value = Mutex::new(0_u8);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = value.lock().unwrap();
            panic!("intentional lock-poison regression");
        }));

        assert!(result.is_err());
        *recover_lock(&value) = 1;
        assert_eq!(*recover_lock(&value), 1);
    }

    #[test]
    fn unexpected_thread_exit_fail_stops_admission_once() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let wake_addr = listener.local_addr().unwrap();
        let (admission, inbox) = admission_channel();
        let lifecycle = Arc::new(ServiceLifecycle::new(
            admission,
            wake_addr,
            Arc::new(ActiveConnections::default()),
        ));

        drop(UnexpectedThreadExit::new(Arc::clone(&lifecycle)));
        lifecycle.stop();

        assert!(!lifecycle.is_running());
        assert!(inbox.is_closed());
        drop(listener);
    }
}
