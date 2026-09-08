//! Chromecast device discovery via mDNS

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use mdns_sd::{ServiceDaemon, ServiceEvent};
use serde::Serialize;

use crate::CastError;

const SERVICE_TYPE: &str = "_googlecast._tcp.local.";

/// Discovered Chromecast device
#[derive(Debug, Clone, Serialize)]
pub struct DiscoveredDevice {
    pub id: String,
    pub name: String,
    pub model: String,
    pub ip: String,
    pub port: u16,
    /// Whether `id` came from the mDNS TXT `id` record — the Cast device
    /// UUID, stable across restarts and renames (comparable to a DLNA UDN).
    /// When the record is absent, `id` falls back to the mDNS fullname,
    /// which is derived from the friendly name and so DETACHES on rename:
    /// callers must not persist per-device state keyed on an unstable id
    /// (#638 fix 4 offers no quality cap for such devices).
    pub id_is_stable: bool,
}

#[derive(Default)]
struct DiscoveryState {
    devices: HashMap<String, DiscoveredDevice>,
    fullname_to_id: HashMap<String, String>,
}

/// Discovery manager for Chromecast devices
pub struct DeviceDiscovery {
    state: Arc<Mutex<DiscoveryState>>,
    running: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
    daemon: Option<ServiceDaemon>,
}

impl DeviceDiscovery {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(DiscoveryState::default())),
            running: Arc::new(AtomicBool::new(false)),
            handle: None,
            daemon: None,
        }
    }

    /// Start mDNS discovery in background
    pub fn start_discovery(&mut self) -> Result<(), CastError> {
        if self.running.load(Ordering::SeqCst) {
            return Ok(());
        }

        let mdns = ServiceDaemon::new()
            .map_err(|e| CastError::Discovery(format!("Failed to create mDNS daemon: {}", e)))?;
        let receiver = mdns
            .browse(SERVICE_TYPE)
            .map_err(|e| CastError::Discovery(format!("Failed to browse mDNS services: {}", e)))?;

        let running = self.running.clone();
        let state = self.state.clone();

        running.store(true, Ordering::SeqCst);

        let handle = thread::spawn(move || {
            for event in receiver.iter() {
                if !running.load(Ordering::SeqCst) {
                    break;
                }

                match event {
                    ServiceEvent::ServiceResolved(info) => {
                        let fullname = info.get_fullname().to_string();
                        let id_prop = info.get_property_val_str("id");
                        let id_is_stable = id_prop.is_some();
                        let id = id_prop
                            .unwrap_or_else(|| fullname.as_str())
                            .to_string();
                        let name = info
                            .get_property_val_str("fn")
                            .unwrap_or_else(|| id.as_str())
                            .to_string();
                        let model = info
                            .get_property_val_str("md")
                            .unwrap_or("Unknown")
                            .to_string();

                        // mdns-sd 0.21 scopes addresses by interface (ScopedIp);
                        // the pick only ever needed the bare IpAddr.
                        let addrs: std::collections::HashSet<IpAddr> =
                            info.get_addresses().iter().map(|a| a.to_ip_addr()).collect();
                        let ip = pick_ip(&addrs)
                            .unwrap_or_else(|| "127.0.0.1".to_string());

                        let device = DiscoveredDevice {
                            id: id.clone(),
                            name,
                            model,
                            ip,
                            port: info.get_port(),
                            id_is_stable,
                        };

                        if let Ok(mut state) = state.lock() {
                            state.fullname_to_id.insert(fullname, id.clone());
                            state.devices.insert(id, device);
                        }
                    }
                    ServiceEvent::ServiceRemoved(_, fullname) => {
                        if let Ok(mut state) = state.lock() {
                            if let Some(id) = state.fullname_to_id.remove(&fullname) {
                                state.devices.remove(&id);
                            }
                        }
                    }
                    _ => {}
                }
            }
        });

        self.daemon = Some(mdns);
        self.handle = Some(handle);
        Ok(())
    }

    /// Stop discovery and release resources
    pub fn stop_discovery(&mut self) -> Result<(), CastError> {
        self.running.store(false, Ordering::SeqCst);

        if let Some(daemon) = self.daemon.take() {
            daemon
                .shutdown()
                .map_err(|e| CastError::Discovery(format!("Failed to shutdown mDNS: {}", e)))?;
        }

        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }

        Ok(())
    }

    /// Return list of discovered devices
    pub fn get_discovered_devices(&self) -> Vec<DiscoveredDevice> {
        self.state
            .lock()
            .map(|state| state.devices.values().cloned().collect())
            .unwrap_or_default()
    }

    /// Get a specific device by ID
    pub fn get_device(&self, device_id: &str) -> Option<DiscoveredDevice> {
        self.state
            .lock()
            .ok()
            .and_then(|state| state.devices.get(device_id).cloned())
    }
}

impl Default for DeviceDiscovery {
    fn default() -> Self {
        Self::new()
    }
}

fn pick_ip(addresses: &std::collections::HashSet<IpAddr>) -> Option<String> {
    let ipv4 = addresses.iter().find(|addr| addr.is_ipv4());
    let ip = ipv4.or_else(|| addresses.iter().next())?;
    Some(ip.to_string())
}
