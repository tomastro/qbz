//! DLNA/UPnP device discovery via SSDP

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::TryStreamExt;
use rupnp::ssdp::{SearchTarget, URN};
use serde::Serialize;
use tokio::task::JoinHandle;

use crate::DlnaError;

const DISCOVERY_WINDOW_SECS: u64 = 3;
const DISCOVERY_SLEEP_SECS: u64 = 5;

/// Discovered DLNA/UPnP device
#[derive(Debug, Clone, Serialize)]
pub struct DiscoveredDlnaDevice {
    pub id: String,
    pub name: String,
    pub manufacturer: String,
    pub model: String,
    pub ip: String,
    pub url: String,
    pub has_av_transport: bool,
    pub has_rendering_control: bool,
}

#[derive(Default)]
struct DiscoveryState {
    devices: HashMap<String, DiscoveredDlnaDevice>,
}

/// Discovery manager for DLNA devices
pub struct DlnaDiscovery {
    state: Arc<Mutex<DiscoveryState>>,
    running: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl DlnaDiscovery {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(DiscoveryState::default())),
            running: Arc::new(AtomicBool::new(false)),
            handle: None,
        }
    }

    /// Start SSDP discovery in background
    pub async fn start_discovery(&mut self) -> Result<(), DlnaError> {
        if self.running.load(Ordering::SeqCst) {
            return Ok(());
        }

        let running = self.running.clone();
        let state = self.state.clone();
        running.store(true, Ordering::SeqCst);

        let handle = tokio::spawn(async move {
            while running.load(Ordering::SeqCst) {
                let target = SearchTarget::URN(media_renderer_urn());
                let discover =
                    rupnp::discover(&target, Duration::from_secs(DISCOVERY_WINDOW_SECS), None)
                        .await;

                if let Ok(stream) = discover {
                    let mut stream = std::pin::pin!(stream);
                    // Error-tolerant loop: a single device whose description
                    // fetch/parse fails must NOT kill the whole window (the
                    // old `while let Ok(...)` broke on the first Err and
                    // silently dropped every other responder, forever — the
                    // #667 class of silent failure). Log and continue.
                    loop {
                        match stream.try_next().await {
                            Ok(Some(device)) => {
                                let id = device.udn().to_string();
                                let name = device.friendly_name().to_string();
                                let manufacturer = device.manufacturer().to_string();
                                let model = device.model_name().to_string();
                                let url = device.url().to_string();
                                let ip = device.url().host().unwrap_or("unknown").to_string();

                                let mut has_av_transport = false;
                                let mut has_rendering_control = false;

                                for service in device.services_iter() {
                                    // Version-agnostic match: a renderer advertising only
                                    // AVTransport:2/:3 (not :1) must still count as capable.
                                    let service_type = service.service_type().to_string();
                                    if service_type.contains(":service:AVTransport:") {
                                        has_av_transport = true;
                                    }
                                    if service_type.contains(":service:RenderingControl:") {
                                        has_rendering_control = true;
                                    }
                                }

                                log::info!(
                                    "[Cast] DLNA discovered: '{name}' ({manufacturer} {model}) at {ip} avt={has_av_transport} rc={has_rendering_control}"
                                );

                                let discovered = DiscoveredDlnaDevice {
                                    id: id.clone(),
                                    name,
                                    manufacturer,
                                    model,
                                    ip,
                                    url,
                                    has_av_transport,
                                    has_rendering_control,
                                };

                                if let Ok(mut state) = state.lock() {
                                    state.devices.insert(id, discovered);
                                }
                            }
                            Ok(None) => break, // window elapsed
                            Err(e) => {
                                log::warn!(
                                    "[Cast] DLNA discovery: skipping a device (description fetch/parse failed): {e}"
                                );
                            }
                        }
                    }
                }

                tokio::time::sleep(Duration::from_secs(DISCOVERY_SLEEP_SECS)).await;
            }
        });

        self.handle = Some(handle);
        Ok(())
    }

    /// Stop discovery and release resources
    pub fn stop_discovery(&mut self) -> Result<(), DlnaError> {
        self.running.store(false, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            handle.abort();
        }
        Ok(())
    }

    /// Return list of discovered devices
    pub fn get_discovered_devices(&self) -> Vec<DiscoveredDlnaDevice> {
        self.state
            .lock()
            .map(|state| state.devices.values().cloned().collect())
            .unwrap_or_default()
    }

    /// Get a specific device by ID
    pub fn get_device(&self, device_id: &str) -> Option<DiscoveredDlnaDevice> {
        self.state
            .lock()
            .ok()
            .and_then(|state| state.devices.get(device_id).cloned())
    }
}

impl Default for DlnaDiscovery {
    fn default() -> Self {
        Self::new()
    }
}

fn media_renderer_urn() -> URN {
    URN::Device("schemas-upnp-org".into(), "MediaRenderer".into(), 1)
}
