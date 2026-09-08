use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use qconnect_lan::{
    ConnectInfo, DeviceType, DisplayInfo, EndpointPolicy, LanError, LanProjection, LanService,
    LanServiceConfig, MaxAudioQuality, MAX_BODY_BYTES,
};
use reqwest::StatusCode;

const DEVICE_UUID: &str = "550e8400-e29b-41d4-a716-446655440000";

fn start_service() -> (
    LanService,
    qconnect_lan::AdmissionInbox,
    String,
    LanProjection,
) {
    start_service_with_timeout(Duration::from_secs(5))
}

fn start_service_with_timeout(
    read_timeout: Duration,
) -> (
    LanService,
    qconnect_lan::AdmissionInbox,
    String,
    LanProjection,
) {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    let projection = LanProjection::new(
        DisplayInfo {
            friendly_name: "Kitchen".to_string(),
            serial_number: DEVICE_UUID.to_string(),
            brand_display_name: "QBZ".to_string(),
            model_display_name: "QBZ Daemon".to_string(),
            max_audio_quality: MaxAudioQuality::UpToHires192,
            device_type: DeviceType::Streamer,
            software_version: "2.1.0".to_string(),
        },
        ConnectInfo {
            app_id: "trusted-app-id".to_string(),
            current_session_id: None,
        },
    );
    let mut config = LanServiceConfig::new(
        projection.clone(),
        EndpointPolicy::new(["api.qobuz.test"], ["qws.qobuz.test"]),
        DEVICE_UUID,
    );
    config.bind_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0);
    config.read_timeout = read_timeout;
    config.advertise_mdns = false;
    let (service, inbox) = LanService::start(config).unwrap();
    let base = format!("http://127.0.0.1:{}", service.port());
    (service, inbox, base, projection)
}

fn test_config(bind_addr: SocketAddr) -> LanServiceConfig {
    let projection = LanProjection::new(
        DisplayInfo {
            friendly_name: "Kitchen".to_string(),
            serial_number: DEVICE_UUID.to_string(),
            brand_display_name: "QBZ".to_string(),
            model_display_name: "QBZ Daemon".to_string(),
            max_audio_quality: MaxAudioQuality::UpToHires192,
            device_type: DeviceType::Streamer,
            software_version: "2.1.0".to_string(),
        },
        ConnectInfo {
            app_id: "trusted-app-id".to_string(),
            current_session_id: None,
        },
    );
    let mut config = LanServiceConfig::new(
        projection,
        EndpointPolicy::new(["api.qobuz.test"], ["qws.qobuz.test"]),
        DEVICE_UUID,
    );
    config.bind_addr = bind_addr;
    config
}

fn stalled_post(port: u16, content_length: usize) -> TcpStream {
    let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    write!(
        stream,
        "POST /connect-to-qconnect HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {content_length}\r\nConnection: close\r\n\r\n{{"
    )
    .unwrap();
    stream.flush().unwrap();
    stream
}

fn stalled_headers(port: u16) -> TcpStream {
    let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    stream
        .write_all(b"POST /connect-to-qconnect HTTP/1.1\r\nHost: localhost\r\nContent-Ty")
        .unwrap();
    stream.flush().unwrap();
    stream
}

fn read_raw_response(stream: &mut TcpStream) -> String {
    let mut response = String::new();
    match stream.read_to_string(&mut response) {
        Ok(_) => response,
        Err(error) if error.kind() == std::io::ErrorKind::ConnectionReset => response,
        Err(error) => panic!("failed to read raw HTTP response: {error}"),
    }
}

fn read_raw_response_without_reset(stream: &mut TcpStream) -> String {
    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .expect("overload response must end with a clean EOF");
    response
}

fn raw_exchange(port: u16, request: &[u8]) -> String {
    let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    stream.write_all(request).unwrap();
    stream.flush().unwrap();
    read_raw_response(&mut stream)
}

async fn wait_for_status(base: &str, expected: StatusCode, budget: Duration) {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(1))
        .build()
        .unwrap();
    let deadline = Instant::now() + budget;
    loop {
        let status = client
            .get(format!("{base}/get-display-info"))
            .send()
            .await
            .unwrap()
            .status();
        if status == expected {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "expected HTTP {expected}, last response was {status}"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

fn handoff(session_id: &str, become_active: bool) -> serde_json::Value {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    serde_json::json!({
        "session_id": session_id,
        "jwt_api": {
            "endpoint": "https://api.qobuz.test/api.json/0.2",
            "exp": now + 600,
            "jwt": "api-secret"
        },
        "jwt_qconnect": {
            "endpoint": "wss://qws.qobuz.test/ws",
            "exp": now + 600,
            "jwt": "qws-secret"
        },
        "become_active": become_active
    })
}

#[tokio::test]
async fn serves_official_get_union_and_admits_exact_post_shape() {
    let (mut service, inbox, base, _) = start_service();
    let client = reqwest::Client::new();
    assert!(service.is_running());

    let display = client
        .get(format!("{base}/get-display-info"))
        .header("Connection", "close")
        .send()
        .await
        .unwrap();
    assert_eq!(display.status(), StatusCode::OK);
    assert_eq!(
        display.headers()["content-type"].to_str().unwrap(),
        "application/json"
    );
    assert_eq!(
        display.json::<serde_json::Value>().await.unwrap(),
        serde_json::json!({
            "friendly_name": "Kitchen",
            "serial_number": DEVICE_UUID,
            "brand_display_name": "QBZ",
            "model_display_name": "QBZ Daemon",
            "max_audio_quality": "UP_TO_HIRES_192",
            "type": "Streamer",
            "software_version": "2.1.0"
        })
    );

    let connect = client
        .get(format!("{base}/get-connect-info"))
        .send()
        .await
        .unwrap();
    assert_eq!(connect.status(), StatusCode::OK);
    assert_eq!(
        connect.json::<serde_json::Value>().await.unwrap(),
        serde_json::json!({ "app_id": "trusted-app-id", "current_session_id": null })
    );

    let accepted = client
        .post(format!("{base}/connect-to-qconnect"))
        .json(&handoff("controller-session", true))
        .send()
        .await
        .unwrap();
    assert_eq!(accepted.status(), StatusCode::OK);
    assert_eq!(
        accepted.json::<serde_json::Value>().await.unwrap(),
        serde_json::json!({})
    );
    let candidate = inbox.take_timeout(Duration::from_secs(1)).unwrap();
    assert_eq!(candidate.session_id(), "controller-session");
    assert_eq!(
        candidate.api_token().endpoint(),
        "https://api.qobuz.test/api.json/0.2"
    );
    assert_eq!(
        candidate.qconnect_token().endpoint(),
        "wss://qws.qobuz.test/ws"
    );

    service.shutdown();
    service.shutdown();
    assert!(!service.is_running());
    assert!(inbox.is_closed());
}

#[tokio::test]
async fn pending_post_is_latest_wins_and_rate_limit_is_bounded() {
    let (mut service, inbox, base, _) = start_service();
    let client = reqwest::Client::new();

    for session in ["first", "second"] {
        let response = client
            .post(format!("{base}/connect-to-qconnect"))
            .json(&handoff(session, true))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }
    assert_eq!(inbox.try_take().unwrap().session_id(), "second");

    let limited = client
        .post(format!("{base}/connect-to-qconnect"))
        .json(&handoff("third", true))
        .send()
        .await
        .unwrap();
    assert_eq!(limited.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(!limited.text().await.unwrap().contains("api-secret"));
    service.shutdown();
}

#[tokio::test]
async fn rejects_noncanonical_http_and_oversized_or_inactive_handoffs() {
    let (mut service, _inbox, base, _) = start_service();
    let client = reqwest::Client::new();

    assert_eq!(
        client
            .get(format!("{base}/get-display-info?alias=1"))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        client
            .post(format!("{base}/get-connect-info"))
            .json(&serde_json::json!({}))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::METHOD_NOT_ALLOWED
    );
    assert_eq!(
        client
            .post(format!("{base}/connect-to-qconnect"))
            .body("{}")
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNSUPPORTED_MEDIA_TYPE
    );
    assert_eq!(
        client
            .post(format!("{base}/connect-to-qconnect"))
            .json(&handoff("inactive", false))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );

    let (mut oversized_service, _, oversized_base, _) = start_service();
    let oversized = client
        .post(format!("{oversized_base}/connect-to-qconnect"))
        .header("Content-Type", "application/json")
        .body(vec![b'x'; MAX_BODY_BYTES + 1])
        .send()
        .await
        .unwrap();
    assert_eq!(oversized.status(), StatusCode::PAYLOAD_TOO_LARGE);

    service.shutdown();
    oversized_service.shutdown();
}

#[tokio::test]
async fn connect_projection_updates_without_restarting_listener() {
    let (mut service, _, base, projection) = start_service();
    projection.set_current_session_id(Some("active-session".to_string()));

    let value = reqwest::get(format!("{base}/get-connect-info"))
        .await
        .unwrap()
        .json::<serde_json::Value>()
        .await
        .unwrap();
    assert_eq!(value["current_session_id"], "active-session");
    service.shutdown();
}

#[tokio::test]
async fn listener_survives_idle_beyond_per_request_deadline() {
    let (mut service, _, base, _) = start_service_with_timeout(Duration::from_millis(100));

    tokio::time::sleep(Duration::from_millis(250)).await;
    wait_for_status(&base, StatusCode::OK, Duration::from_secs(1)).await;

    service.shutdown();
}

#[tokio::test]
async fn two_partial_bodies_are_bounded_and_capacity_recovers() {
    let (mut service, _, base, _) = start_service_with_timeout(Duration::from_millis(750));
    let mut first = stalled_post(service.port(), 512);
    let mut second = stalled_post(service.port(), 512);

    wait_for_status(
        &base,
        StatusCode::TOO_MANY_REQUESTS,
        Duration::from_millis(300),
    )
    .await;

    for stream in [&mut first, &mut second] {
        let response = read_raw_response(stream);
        assert!(response.starts_with("HTTP/1.1 400"), "{response}");
    }
    wait_for_status(&base, StatusCode::OK, Duration::from_secs(1)).await;

    service.shutdown();
}

#[tokio::test]
async fn two_partial_headers_are_bounded_and_capacity_recovers() {
    let (mut service, _, base, _) = start_service_with_timeout(Duration::from_millis(750));
    let mut first = stalled_headers(service.port());
    let mut second = stalled_headers(service.port());

    wait_for_status(
        &base,
        StatusCode::TOO_MANY_REQUESTS,
        Duration::from_millis(300),
    )
    .await;

    for stream in [&mut first, &mut second] {
        let response = read_raw_response(stream);
        assert!(response.starts_with("HTTP/1.1 400"), "{response}");
    }
    wait_for_status(&base, StatusCode::OK, Duration::from_secs(1)).await;

    service.shutdown();
}

#[test]
fn saturated_full_post_receives_complete_429_without_reset() {
    let (mut service, _, _, _) = start_service_with_timeout(Duration::from_secs(2));
    let first = stalled_post(service.port(), 512);
    let second = stalled_post(service.port(), 512);
    std::thread::sleep(Duration::from_millis(25));

    let body = vec![b'x'; 32 * 1024];
    let mut third = TcpStream::connect((Ipv4Addr::LOCALHOST, service.port())).unwrap();
    third
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    write!(
        third,
        "POST /connect-to-qconnect HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .unwrap();
    third.write_all(&body).unwrap();
    third.flush().unwrap();

    let response = read_raw_response_without_reset(&mut third);
    assert!(response.starts_with("HTTP/1.1 429"), "{response}");
    let (head, response_body) = response.split_once("\r\n\r\n").unwrap();
    let declared = head
        .lines()
        .find_map(|line| {
            line.strip_prefix("Content-Length: ")
                .and_then(|value| value.parse::<usize>().ok())
        })
        .unwrap();
    assert_eq!(response_body.len(), declared);
    assert_eq!(response_body, r#"{"error":"busy"}"#);

    drop(first);
    drop(second);
    service.shutdown();
}

#[test]
fn rejected_header_trickle_cannot_delay_shutdown() {
    let (mut service, _, _, _) = start_service_with_timeout(Duration::from_secs(2));
    let first = stalled_post(service.port(), 512);
    let second = stalled_post(service.port(), 512);
    std::thread::sleep(Duration::from_millis(25));

    let mut third = TcpStream::connect((Ipv4Addr::LOCALHOST, service.port())).unwrap();
    third.write_all(b"P").unwrap();
    third.flush().unwrap();
    let drip = std::thread::spawn(move || {
        for _ in 0..100 {
            std::thread::sleep(Duration::from_millis(10));
            if third.write_all(b"O").is_err() || third.flush().is_err() {
                break;
            }
        }
    });

    std::thread::sleep(Duration::from_millis(75));
    let started = Instant::now();
    service.shutdown();
    assert!(
        started.elapsed() < Duration::from_millis(250),
        "shutdown was retained by rejected trickle: {:?}",
        started.elapsed()
    );

    drip.join().unwrap();
    drop(first);
    drop(second);
}

#[tokio::test]
async fn trickled_body_hits_absolute_deadline_and_capacity_recovers() {
    let timeout = Duration::from_millis(500);
    let (mut service, _, base, _) = start_service_with_timeout(timeout);
    let mut writer = TcpStream::connect((Ipv4Addr::LOCALHOST, service.port())).unwrap();
    writer
        .set_write_timeout(Some(Duration::from_secs(1)))
        .unwrap();
    let started = Instant::now();
    writer
        .write_all(
            b"POST /connect-to-qconnect HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: 64\r\nConnection: close\r\n\r\n{",
        )
        .unwrap();
    writer.flush().unwrap();
    let mut reader = writer.try_clone().unwrap();
    reader
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let drip = std::thread::spawn(move || {
        for _ in 0..4 {
            std::thread::sleep(Duration::from_millis(80));
            if writer.write_all(b"x").is_err() || writer.flush().is_err() {
                break;
            }
        }
    });

    let response = read_raw_response(&mut reader);
    let elapsed = started.elapsed();
    drip.join().unwrap();

    assert!(response.starts_with("HTTP/1.1 400"), "{response}");
    assert!(
        elapsed >= Duration::from_millis(300),
        "elapsed: {elapsed:?}"
    );
    assert!(elapsed < Duration::from_millis(700), "elapsed: {elapsed:?}");
    wait_for_status(&base, StatusCode::OK, Duration::from_secs(1)).await;
    service.shutdown();
}

#[test]
fn oversized_declared_body_is_rejected_without_waiting_for_body() {
    let (mut service, _, _, _) = start_service_with_timeout(Duration::from_secs(2));
    let request = format!(
        "POST /connect-to-qconnect HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        MAX_BODY_BYTES + 1
    );

    let started = Instant::now();
    let response = raw_exchange(service.port(), request.as_bytes());
    assert!(response.starts_with("HTTP/1.1 413"), "{response}");
    assert!(started.elapsed() < Duration::from_secs(1));
    service.shutdown();
}

#[test]
fn oversized_incomplete_header_is_rejected_at_the_header_cap() {
    let (mut service, _, _, _) = start_service_with_timeout(Duration::from_secs(2));
    let request = format!(
        "GET /get-display-info HTTP/1.1\r\nHost: localhost\r\nX-Fill: {}",
        "a".repeat(17 * 1024)
    );

    let response = raw_exchange(service.port(), request.as_bytes());
    assert!(response.starts_with("HTTP/1.1 413"), "{response}");
    service.shutdown();
}

#[test]
fn expect_continue_is_rejected_immediately_without_reading_the_body() {
    let (mut service, _, _, _) = start_service_with_timeout(Duration::from_secs(2));
    let request = b"POST /connect-to-qconnect HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: 1024\r\nExpect: 100-continue\r\nConnection: close\r\n\r\n";

    let started = Instant::now();
    let response = raw_exchange(service.port(), request);
    assert!(response.starts_with("HTTP/1.1 417"), "{response}");
    assert!(started.elapsed() < Duration::from_secs(1));
    service.shutdown();
}

#[tokio::test]
async fn shutdown_interrupts_two_slowloris_connections_and_closes_the_port() {
    let (mut service, inbox, base, _) = start_service_with_timeout(Duration::from_secs(5));
    let port = service.port();
    let first = stalled_headers(port);
    let second = stalled_headers(port);
    wait_for_status(
        &base,
        StatusCode::TOO_MANY_REQUESTS,
        Duration::from_millis(500),
    )
    .await;

    let started = Instant::now();
    service.shutdown();
    assert!(started.elapsed() < Duration::from_secs(1));
    assert!(inbox.is_closed());
    assert!(TcpStream::connect_timeout(
        &SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port),
        Duration::from_millis(100),
    )
    .is_err());

    service.shutdown();
    drop(first);
    drop(second);
}

#[test]
fn invalid_mdns_address_is_rejected_before_bind() {
    let reservation = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let bind_addr = reservation.local_addr().unwrap();
    drop(reservation);

    let mut config = test_config(bind_addr);
    config.advertise_mdns = true;
    config.advertised_addresses = Some(vec![IpAddr::V4(Ipv4Addr::LOCALHOST)]);
    let error = match LanService::start(config) {
        Ok(_) => panic!("loopback mDNS address unexpectedly accepted"),
        Err(error) => error,
    };
    assert!(matches!(error, LanError::InvalidConfig));

    std::thread::sleep(Duration::from_millis(25));
    assert!(TcpStream::connect_timeout(&bind_addr, Duration::from_millis(100)).is_err());
}

#[test]
fn explicit_mdns_address_must_match_specific_bind_address() {
    let mut config = test_config(SocketAddr::from(([192, 0, 2, 10], 0)));
    config.advertise_mdns = true;
    config.advertised_addresses = Some(vec![IpAddr::V4(Ipv4Addr::new(192, 0, 2, 11))]);

    let error = match LanService::start(config) {
        Ok(_) => panic!("mismatched bind and advertised addresses unexpectedly accepted"),
        Err(error) => error,
    };
    assert!(matches!(error, LanError::InvalidConfig));
}
