//! Development-only official-client handoff probe.
//!
//! This binary advertises one ephemeral LAN renderer, accepts exactly the
//! official handoff shape, and validates the two delegated contexts without
//! printing token, endpoint, account or session values. It never starts the
//! player and never persists the handoff.

use std::collections::BTreeSet;
use std::error::Error;
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use qbz_qobuz::{endpoints, QobuzClient};
use qconnect_core::QueueVersion;
use qconnect_lan::{
    ConnectInfo, DeviceType, DisplayInfo, EndpointPolicy, HandoffCandidate, LanProjection,
    LanService, LanServiceConfig, MaxAudioQuality,
};
use qconnect_protocol::{
    build_qconnect_renderer_outbound_envelope, RendererBufferState, RendererCommandType,
    RendererReport, RendererReportType,
};
use qconnect_transport_ws::{NativeWsTransport, TransportEvent, WsTransport, WsTransportConfig};
use reqwest::redirect::Policy;
use serde_json::{json, Value};
use uuid::Uuid;
use zeroize::Zeroize;

const WAIT_FOR_HANDOFF: Duration = Duration::from_secs(10 * 60);
const CLOUD_VALIDATION_TIMEOUT: Duration = Duration::from_secs(30);

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    let client = QobuzClient::new()?;
    client.init().await?;
    login_from_saved_oauth(&client).await?;
    let app_id = client.app_id().await?;
    let (qws_endpoint, _expires_at, mut owner_qws_jwt) =
        client.create_qconnect_token().await?.into_parts();
    owner_qws_jwt.zeroize();
    let qws_endpoint = qws_endpoint.ok_or("owner QWS token omitted its endpoint")?;
    let endpoint_policy =
        EndpointPolicy::from_trusted_endpoints(endpoints::BASE_URL, &qws_endpoint)
            .map_err(|_| "owner token supplied an invalid trusted endpoint")?;

    let device_uuid = Uuid::new_v4().to_string();
    let projection = LanProjection::new(
        DisplayInfo {
            friendly_name: "QBZ protocol probe".to_string(),
            serial_number: device_uuid.clone(),
            brand_display_name: "QBZ".to_string(),
            model_display_name: "Development harness".to_string(),
            max_audio_quality: MaxAudioQuality::UpToHires192,
            device_type: DeviceType::Computer,
            software_version: env!("CARGO_PKG_VERSION").to_string(),
        },
        ConnectInfo {
            app_id: app_id.clone(),
            current_session_id: None,
        },
    );
    let (observations_tx, observations_rx) = mpsc::sync_channel(32);
    let mut config = LanServiceConfig::new(projection, endpoint_policy, device_uuid.clone());
    config.request_observer = Some(observations_tx);
    let (mut lan, inbox) = LanService::start(config)?;

    eprintln!("[probe] advertised 'QBZ protocol probe' for ten minutes");
    eprintln!("[probe] select it from an official Qobuz client on this LAN");
    let observation_task = tokio::task::spawn_blocking(move || {
        while let Ok(observation) = observations_rx.recv() {
            eprintln!("[probe] LAN {observation:?}");
        }
    });
    let wait = tokio::task::spawn_blocking(move || inbox.take_timeout(WAIT_FOR_HANDOFF));
    let candidate = tokio::select! {
        result = wait => result.map_err(|_| "handoff waiter failed")?,
        _ = tokio::signal::ctrl_c() => {
            lan.shutdown();
            return Ok(());
        }
    }
    .ok_or("no official handoff arrived before the deadline")?;
    lan.shutdown();
    let _ = observation_task.await;

    eprintln!("[probe] exact official handoff admitted; no secret was logged");
    print_jwt_shape("jwt_api", candidate.api_token().jwt());
    print_jwt_shape("jwt_qconnect", candidate.qconnect_token().jwt());

    let api = probe_api_auth(&candidate, &app_id).await?;
    eprintln!(
        "[probe] delegated API user/get: baseline={}, x-user-auth-token={}, bearer={}",
        api.baseline.label(),
        api.x_user_auth_token.label(),
        api.bearer.label()
    );

    validate_qws_session(&candidate, &device_uuid).await?;
    eprintln!("[probe] delegated QWS accepted the controller-supplied session UUID");
    Ok(())
}

async fn login_from_saved_oauth(client: &QobuzClient) -> Result<(), Box<dyn Error>> {
    // Match the application's no-ambient-runtime credential derivation.
    let (keyring, file) = std::thread::spawn(|| {
        (
            qbz_credentials::load_oauth_token().ok().flatten(),
            qbz_credentials::load_oauth_token_from_file().ok().flatten(),
        )
    })
    .join()
    .map_err(|_| "credential loader panicked")?;

    let mut candidates = Vec::new();
    if let Some(token) = keyring {
        candidates.push(token);
    }
    if let Some(token) = file {
        if !candidates.iter().any(|existing| existing == &token) {
            candidates.push(token);
        }
    }
    for mut token in candidates {
        let result = client.login_with_token(&token).await;
        token.zeroize();
        if result.is_ok() {
            return Ok(());
        }
    }
    Err("no saved OAuth token could initialize the probe".into())
}

fn print_jwt_shape(label: &str, jwt: &str) {
    let segments = jwt.split('.').collect::<Vec<_>>();
    if segments.len() != 3 {
        eprintln!(
            "[probe] {label}: opaque token with {} segment(s)",
            segments.len()
        );
        return;
    }
    let header = json_object_keys(segments[0]);
    let claims = json_object_keys(segments[1]);
    eprintln!("[probe] {label}: JWT header_keys={header:?} claim_keys={claims:?}");
}

fn json_object_keys(segment: &str) -> BTreeSet<String> {
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(segment)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|value| value.as_object().cloned())
        .map(|object| object.into_iter().map(|(key, _)| key).collect())
        .unwrap_or_default()
}

#[derive(Clone, Copy)]
struct ProbeResponse {
    status: u16,
    user_shape: bool,
}

impl ProbeResponse {
    fn label(self) -> String {
        format!(
            "{}:{}",
            self.status,
            if self.user_shape { "user" } else { "other" }
        )
    }
}

struct ApiProbe {
    baseline: ProbeResponse,
    x_user_auth_token: ProbeResponse,
    bearer: ProbeResponse,
}

async fn probe_api_auth(
    candidate: &HandoffCandidate,
    app_id: &str,
) -> Result<ApiProbe, Box<dyn Error>> {
    let mut endpoint = url::Url::parse(candidate.api_token().endpoint())?;
    if !endpoint.path().ends_with('/') {
        endpoint.set_path(&format!("{}/", endpoint.path()));
    }
    let url = endpoint.join("user/get")?;
    let http = reqwest::Client::builder()
        .redirect(Policy::none())
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(10))
        .build()?;

    let baseline = inspect_api_response(
        http.get(url.clone())
            .header("X-App-Id", app_id)
            .send()
            .await?,
    )
    .await;
    let x_user_auth_token = inspect_api_response(
        http.get(url.clone())
            .header("X-App-Id", app_id)
            .header("X-User-Auth-Token", candidate.api_token().jwt())
            .send()
            .await?,
    )
    .await;
    let bearer = inspect_api_response(
        http.get(url)
            .header("X-App-Id", app_id)
            .bearer_auth(candidate.api_token().jwt())
            .send()
            .await?,
    )
    .await;

    Ok(ApiProbe {
        baseline,
        x_user_auth_token,
        bearer,
    })
}

async fn inspect_api_response(response: reqwest::Response) -> ProbeResponse {
    let status = response.status().as_u16();
    let user_shape = response
        .json::<Value>()
        .await
        .ok()
        .and_then(|value| value.as_object().cloned())
        .is_some_and(|object| {
            object.contains_key("id")
                || object.contains_key("user")
                || object.contains_key("credential")
        });
    ProbeResponse { status, user_shape }
}

async fn validate_qws_session(
    candidate: &HandoffCandidate,
    device_uuid: &str,
) -> Result<(), Box<dyn Error>> {
    let transport = Arc::new(NativeWsTransport::new());
    let mut events = transport.subscribe();
    let mut config = WsTransportConfig::default();
    config.endpoint_url = candidate.qconnect_token().endpoint().to_string();
    config.jwt_qws = Some(candidate.qconnect_token().jwt().to_string());
    config.require_jwt = true;
    config.subscribe_channels = vec![vec![0x01], vec![0x02], vec![0x03]];
    config.reconnect_max_attempts = Some(1);
    transport.connect(config).await?;

    let validation = tokio::time::timeout(CLOUD_VALIDATION_TIMEOUT, async {
        let mut joined = false;
        loop {
            match events.recv().await {
                Ok(TransportEvent::Subscribed) if !joined => {
                    joined = true;
                    let report = RendererReport::new(
                        RendererReportType::RndrSrvrJoinSession,
                        Uuid::new_v4().to_string(),
                        QueueVersion::default(),
                        json!({
                            "session_uuid": candidate.session_id(),
                            "device_info": {
                                "device_uuid": device_uuid,
                                "friendly_name": "QBZ protocol probe",
                                "brand": "QBZ",
                                "model": "Development harness",
                                "device_type": 5,
                                "capabilities": {
                                    "min_audio_quality": 1,
                                    "max_audio_quality": 4,
                                    "volume_remote_control": 2
                                },
                                "software_version": env!("CARGO_PKG_VERSION")
                            },
                            "is_active": candidate.become_active(),
                            "reason": 1,
                            "initial_state": {
                                "playing_state": 1,
                                "buffer_state": RendererBufferState::Ok.as_i32(),
                                "current_position": 0,
                                "duration": 0,
                                "queue_version": { "major": 0, "minor": 0 }
                            }
                        }),
                    );
                    let envelope = build_qconnect_renderer_outbound_envelope(report)?;
                    transport.send(envelope).await?;
                }
                Ok(TransportEvent::Connected) => eprintln!("[probe] QWS connected"),
                Ok(TransportEvent::Authenticated) => eprintln!("[probe] QWS authenticated"),
                Ok(TransportEvent::Subscribed) if joined => {
                    eprintln!("[probe] QWS subscribed again after delegated join")
                }
                Ok(TransportEvent::SessionEstablished) => {
                    eprintln!("[probe] QWS controller session established")
                }
                Ok(TransportEvent::CloudError { code, .. }) => {
                    return Err(format!("QWS rejected delegated join with code {code}").into());
                }
                Ok(TransportEvent::OutboundSent { message_type, .. }) => {
                    eprintln!("[probe] QWS outbound {message_type}");
                }
                Ok(TransportEvent::InboundFrameDecoded {
                    cloud_message_type,
                    payload_size,
                }) => {
                    eprintln!(
                        "[probe] QWS inbound cloud_type={cloud_message_type} bytes={payload_size}"
                    );
                }
                Ok(TransportEvent::InboundQueueServerEvent(event)) => {
                    eprintln!("[probe] QWS inbound {}", event.message_type());
                }
                Ok(TransportEvent::InboundRendererServerCommand(command)) => {
                    eprintln!("[probe] QWS inbound {}", command.message_type());
                    if command.command_type == RendererCommandType::SrvrRndrSetActive
                        && command.payload.get("active").and_then(Value::as_bool) == Some(true)
                    {
                        return Ok::<(), Box<dyn Error>>(());
                    }
                }
                Ok(TransportEvent::InboundReceived(_)) => {
                    eprintln!("[probe] QWS inbound JSON envelope");
                }
                Ok(TransportEvent::TransportError { stage, .. }) => {
                    eprintln!("[probe] QWS transport error at stage={stage}");
                }
                Ok(TransportEvent::ReconnectScheduled { attempt, .. }) => {
                    eprintln!("[probe] QWS reconnect scheduled attempt={attempt}");
                }
                Ok(TransportEvent::MaxReconnectAttemptsExceeded { attempts, .. }) => {
                    return Err(
                        format!("QWS validation exhausted after {attempts} attempt(s)").into(),
                    );
                }
                Ok(TransportEvent::Disconnected) => eprintln!("[probe] QWS disconnected"),
                Ok(_) => {}
                Err(_) => return Err("QWS validation event channel closed".into()),
            }
        }
    })
    .await;

    let _ = transport.disconnect().await;
    validation.map_err(|_| "QWS delegated session validation timed out")?
}
