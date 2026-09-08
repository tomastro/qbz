use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use mdns_sd::{
    DaemonEvent, IfKind, Receiver, RecvTimeoutError, ServiceDaemon, ServiceInfo, UnregisterStatus,
};

use crate::server::{LanError, SERVICE_TYPE};

const ANNOUNCE_TIMEOUT: Duration = Duration::from_secs(2);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);
const RECONCILE_INTERVAL: Duration = Duration::from_secs(30);
const MONITOR_WAKE_INTERVAL: Duration = Duration::from_millis(500);
const GOODBYE_RESEND_DELAY: Duration = Duration::from_millis(150);
const DAEMON_SHUTDOWN_RESERVE: Duration = Duration::from_millis(250);

pub(crate) struct MdnsRegistration {
    control: Arc<MdnsControl>,
    monitor: Option<Receiver<DaemonEvent>>,
    monitor_thread: Option<JoinHandle<()>>,
    initial_addresses: Option<Vec<IpAddr>>,
}

#[derive(Clone)]
pub(crate) struct MdnsShutdownHandle {
    control: Arc<MdnsControl>,
}

#[derive(Debug, Clone)]
enum AddressSource {
    Dynamic,
    Fixed(Vec<IpAddr>),
}

struct MdnsControl {
    daemon: ServiceDaemon,
    fullname: String,
    device_uuid: String,
    port: u16,
    sdk_version: String,
    address_source: AddressSource,
    state: Mutex<RegistrationState>,
}

#[derive(Debug, Default)]
struct RegistrationState {
    stopped: bool,
    published_addresses: Option<Vec<IpAddr>>,
    pending_interface_changes: Vec<InterfaceChange>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ReconcileAction {
    Unchanged,
    Withdraw,
    Publish(Vec<IpAddr>),
    Replace(Vec<IpAddr>),
}

#[derive(Debug, Clone, Copy)]
enum InterfaceChange {
    Added(IpAddr),
    Removed(IpAddr),
}

impl MdnsRegistration {
    /// Prepare discovery without publishing it. The server installs the
    /// shutdown handle before its HTTP threads start, so a fatal thread exit
    /// can always fence discovery even while the first announcement is in
    /// flight.
    pub(crate) fn prepare(
        device_uuid: &str,
        port: u16,
        sdk_version: &str,
        addresses: Option<Vec<IpAddr>>,
    ) -> Result<Self, LanError> {
        let (address_source, initial_addresses) = match addresses {
            Some(addresses) => {
                let addresses = normalize_addresses(addresses);
                (AddressSource::Fixed(addresses.clone()), addresses)
            }
            None => (
                AddressSource::Dynamic,
                normalize_addresses(lan_ipv4_addresses()?),
            ),
        };
        if initial_addresses.is_empty() {
            return Err(LanError::NoLanAddresses);
        }

        let info = service_info(device_uuid, port, sdk_version, &initial_addresses)?;
        let fullname = info.get_fullname().to_string();
        let daemon = ServiceDaemon::new()?;
        let monitor = daemon.monitor()?;
        let control = Arc::new(MdnsControl {
            daemon,
            fullname,
            device_uuid: device_uuid.to_string(),
            port,
            sdk_version: sdk_version.to_string(),
            address_source,
            state: Mutex::new(RegistrationState::default()),
        });

        Ok(Self {
            control,
            monitor: Some(monitor),
            monitor_thread: None,
            initial_addresses: Some(initial_addresses),
        })
    }

    pub(crate) fn shutdown_handle(&self) -> MdnsShutdownHandle {
        MdnsShutdownHandle {
            control: Arc::clone(&self.control),
        }
    }

    /// Publish only after the HTTP acceptor and workers exist. The monitor
    /// supervises daemon health and reconciles the complete operational IPv4
    /// interface set.
    pub(crate) fn publish<F>(&mut self, on_fatal: F) -> Result<(), LanError>
    where
        F: Fn() + Send + Sync + 'static,
    {
        let monitor = self.monitor.take().ok_or(LanError::Mdns)?;
        let initial_addresses = self.initial_addresses.take().ok_or(LanError::Mdns)?;
        self.control
            .reconcile(initial_addresses, &monitor, Some(ANNOUNCE_TIMEOUT))?;
        if matches!(&self.control.address_source, AddressSource::Dynamic) {
            // The initial acknowledgement may have shared the monitor queue
            // with an interface event. Re-read the interface set once before the
            // long-lived monitor takes over; runtime publishes do not consume
            // monitor events while waiting for an acknowledgement.
            self.control
                .reconcile(self.control.observed_addresses(), &monitor, None)?;
        }

        let control = Arc::clone(&self.control);
        let monitor_control = Arc::clone(&control);
        let on_fatal = Arc::new(on_fatal);
        let monitor_fatal = Arc::clone(&on_fatal);
        let thread = std::thread::Builder::new()
            .name("qconnect-lan-mdns".to_string())
            .spawn(move || {
                let mut exit_guard =
                    MonitorExitGuard::new(Arc::clone(&monitor_control), Arc::clone(&monitor_fatal));
                if monitor_loop(&monitor_control, &monitor) {
                    return;
                }
                exit_guard.disarm();
            })
            .map_err(|_| LanError::Mdns);

        match thread {
            Ok(thread) => {
                self.monitor_thread = Some(thread);
                Ok(())
            }
            Err(error) => {
                control.stop();
                Err(error)
            }
        }
    }

    pub(crate) fn shutdown(&mut self) {
        self.control.stop();
        if let Some(thread) = self.monitor_thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for MdnsRegistration {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl MdnsShutdownHandle {
    pub(crate) fn shutdown(&self) {
        self.control.stop();
    }
}

impl MdnsControl {
    fn reconcile(
        &self,
        observed_addresses: Vec<IpAddr>,
        monitor: &Receiver<DaemonEvent>,
        announce_timeout: Option<Duration>,
    ) -> Result<(), LanError> {
        let observed_addresses = normalize_addresses(observed_addresses);
        let mut state = recover_lock(&self.state);
        let action = reconcile_action(
            state.stopped,
            state.published_addresses.as_deref(),
            observed_addresses,
        );
        match action {
            ReconcileAction::Unchanged => Ok(()),
            ReconcileAction::Withdraw => {
                self.withdraw_locked(&mut state, Instant::now() + SHUTDOWN_TIMEOUT)
            }
            ReconcileAction::Publish(addresses) => {
                self.publish_locked(&mut state, addresses, monitor, announce_timeout)
            }
            ReconcileAction::Replace(addresses) => {
                self.withdraw_locked(&mut state, Instant::now() + SHUTDOWN_TIMEOUT)?;
                self.publish_locked(&mut state, addresses, monitor, announce_timeout)
            }
        }
    }

    fn publish_locked(
        &self,
        state: &mut RegistrationState,
        addresses: Vec<IpAddr>,
        monitor: &Receiver<DaemonEvent>,
        announce_timeout: Option<Duration>,
    ) -> Result<(), LanError> {
        if state.stopped {
            return Ok(());
        }
        let info = service_info(&self.device_uuid, self.port, &self.sdk_version, &addresses)?;
        self.daemon.register(info).map_err(|_| LanError::Mdns)?;
        // Record the command immediately. If acknowledgement fails, stop()
        // still knows that it must unregister before shutting the daemon down.
        state.published_addresses = Some(addresses);
        match announce_timeout {
            Some(timeout) => {
                state.pending_interface_changes.extend(wait_for_announce(
                    monitor,
                    &self.fullname,
                    timeout,
                )?);
                Ok(())
            }
            None => Ok(()),
        }
    }

    fn withdraw_locked(
        &self,
        state: &mut RegistrationState,
        deadline: Instant,
    ) -> Result<(), LanError> {
        if state.published_addresses.is_none() {
            return Ok(());
        }
        let receiver = self
            .daemon
            .unregister(&self.fullname)
            .map_err(|_| LanError::Mdns)?;
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or(LanError::Mdns)?;
        match receiver.recv_timeout(remaining) {
            Ok(UnregisterStatus::OK) | Ok(UnregisterStatus::NotFound) => {
                state.published_addresses = None;
            }
            Err(_) => return Err(LanError::Mdns),
        }

        // mdns-sd schedules a second goodbye after its acknowledgement. Keep
        // the daemon alive for it, but preserve a bounded shutdown reserve.
        if let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
            let goodbye_budget = remaining.saturating_sub(DAEMON_SHUTDOWN_RESERVE);
            std::thread::sleep(GOODBYE_RESEND_DELAY.min(goodbye_budget));
        }
        Ok(())
    }

    fn stop(&self) {
        let deadline = Instant::now() + SHUTDOWN_TIMEOUT;
        let mut state = recover_lock(&self.state);
        if state.stopped {
            return;
        }
        // This is the linearization point. Reconciliation uses the same lock
        // and checks this flag before every register command, so publication
        // cannot restart after shutdown has won the lock.
        state.stopped = true;
        let _ = self.withdraw_locked(&mut state, deadline);
        if let Ok(receiver) = self.daemon.shutdown() {
            if let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
                let _ = receiver.recv_timeout(remaining);
            }
        }
    }

    fn is_stopped(&self) -> bool {
        recover_lock(&self.state).stopped
    }

    fn observed_addresses(&self) -> Vec<IpAddr> {
        match &self.address_source {
            AddressSource::Dynamic => lan_ipv4_addresses().unwrap_or_default(),
            AddressSource::Fixed(_) => recover_lock(&self.state)
                .published_addresses
                .clone()
                .unwrap_or_default(),
        }
    }

    fn addresses_after_event(&self, added: bool, address: IpAddr) -> Option<Vec<IpAddr>> {
        match &self.address_source {
            AddressSource::Dynamic => Some(self.observed_addresses()),
            AddressSource::Fixed(configured) => {
                let state = recover_lock(&self.state);
                fixed_addresses_after_event(
                    configured,
                    state.published_addresses.as_deref(),
                    added,
                    address,
                )
            }
        }
    }

    fn take_pending_interface_change(&self) -> Option<InterfaceChange> {
        let mut state = recover_lock(&self.state);
        (!state.pending_interface_changes.is_empty())
            .then(|| state.pending_interface_changes.remove(0))
    }
}

struct MonitorExitGuard<F>
where
    F: Fn() + Send + Sync + 'static,
{
    control: Arc<MdnsControl>,
    on_fatal: Arc<F>,
    armed: bool,
}

impl<F> MonitorExitGuard<F>
where
    F: Fn() + Send + Sync + 'static,
{
    fn new(control: Arc<MdnsControl>, on_fatal: Arc<F>) -> Self {
        Self {
            control,
            on_fatal,
            armed: true,
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl<F> Drop for MonitorExitGuard<F>
where
    F: Fn() + Send + Sync + 'static,
{
    fn drop(&mut self) {
        if self.armed {
            self.control.stop();
            (self.on_fatal)();
        }
    }
}

/// Returns true for an unexpected exit. The receiver blocks between actual
/// interface events. Its bounded wake only observes shutdown; interface discovery
/// remains a 30-second missed-event repair pass, not a fast polling loop.
fn monitor_loop(control: &MdnsControl, monitor: &Receiver<DaemonEvent>) -> bool {
    let mut next_reconcile = Instant::now() + RECONCILE_INTERVAL;
    loop {
        if control.is_stopped() {
            return false;
        }
        if let Some(change) = control.take_pending_interface_change() {
            let result = match change {
                InterfaceChange::Added(address) => {
                    reconcile_interface_event(control, monitor, true, address)
                }
                InterfaceChange::Removed(address) => {
                    reconcile_interface_event(control, monitor, false, address)
                }
            };
            if result.is_err() {
                return true;
            }
            next_reconcile = Instant::now() + RECONCILE_INTERVAL;
            continue;
        }
        let wait = next_reconcile
            .saturating_duration_since(Instant::now())
            .min(MONITOR_WAKE_INTERVAL);
        match monitor.recv_timeout(wait) {
            Ok(DaemonEvent::IpAdd(address)) => {
                if reconcile_interface_event(control, monitor, true, address).is_err() {
                    return true;
                }
                next_reconcile = Instant::now() + RECONCILE_INTERVAL;
            }
            Ok(DaemonEvent::IpDel(address)) => {
                if reconcile_interface_event(control, monitor, false, address).is_err() {
                    return true;
                }
                next_reconcile = Instant::now() + RECONCILE_INTERVAL;
            }
            Err(RecvTimeoutError::Timeout) => {
                if Instant::now() >= next_reconcile {
                    if matches!(&control.address_source, AddressSource::Dynamic)
                        && control
                            .reconcile(control.observed_addresses(), monitor, None)
                            .is_err()
                    {
                        return true;
                    }
                    next_reconcile = Instant::now() + RECONCILE_INTERVAL;
                }
            }
            Ok(DaemonEvent::Error(_) | DaemonEvent::NameChange(_)) => return true,
            Err(RecvTimeoutError::Disconnected) => return !control.is_stopped(),
            Ok(_) => {}
        }
    }
}

fn reconcile_interface_event(
    control: &MdnsControl,
    monitor: &Receiver<DaemonEvent>,
    added: bool,
    address: IpAddr,
) -> Result<(), LanError> {
    match control.addresses_after_event(added, address) {
        Some(addresses) => control.reconcile(addresses, monitor, None),
        None => Ok(()),
    }
}

fn wait_for_announce(
    monitor: &Receiver<DaemonEvent>,
    fullname: &str,
    timeout: Duration,
) -> Result<Vec<InterfaceChange>, LanError> {
    let deadline = Instant::now() + timeout;
    let mut interface_changes = Vec::new();
    loop {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or(LanError::Mdns)?;
        match monitor.recv_timeout(remaining) {
            Ok(DaemonEvent::Announce(announced, _)) if announced.eq_ignore_ascii_case(fullname) => {
                return Ok(interface_changes)
            }
            Ok(DaemonEvent::IpAdd(address)) => {
                interface_changes.push(InterfaceChange::Added(address))
            }
            Ok(DaemonEvent::IpDel(address)) => {
                interface_changes.push(InterfaceChange::Removed(address))
            }
            Ok(DaemonEvent::Error(_) | DaemonEvent::NameChange(_)) | Err(_) => {
                return Err(LanError::Mdns)
            }
            Ok(_) => {}
        }
    }
}

fn reconcile_action(
    stopped: bool,
    published_addresses: Option<&[IpAddr]>,
    observed_addresses: Vec<IpAddr>,
) -> ReconcileAction {
    if stopped {
        return ReconcileAction::Unchanged;
    }
    match (published_addresses, observed_addresses.is_empty()) {
        (None, true) => ReconcileAction::Unchanged,
        (Some(_), true) => ReconcileAction::Withdraw,
        (None, false) => ReconcileAction::Publish(observed_addresses),
        (Some(published), false) if published == observed_addresses.as_slice() => {
            ReconcileAction::Unchanged
        }
        (Some(_), false) => ReconcileAction::Replace(observed_addresses),
    }
}

fn fixed_addresses_after_event(
    configured: &[IpAddr],
    published: Option<&[IpAddr]>,
    added: bool,
    address: IpAddr,
) -> Option<Vec<IpAddr>> {
    if !configured.contains(&address) {
        return None;
    }
    let mut observed = published.unwrap_or_default().to_vec();
    if added {
        observed.push(address);
    } else {
        observed.retain(|candidate| *candidate != address);
    }
    Some(normalize_addresses(observed))
}

fn normalize_addresses(addresses: Vec<IpAddr>) -> Vec<IpAddr> {
    let mut addresses = addresses
        .into_iter()
        .filter(|ip| ip.is_ipv4() && !ip.is_loopback() && !ip.is_unspecified())
        .collect::<Vec<_>>();
    addresses.sort_unstable();
    addresses.dedup();
    addresses
}

fn service_info(
    device_uuid: &str,
    port: u16,
    sdk_version: &str,
    addresses: &[IpAddr],
) -> Result<ServiceInfo, mdns_sd::Error> {
    let short = short_id(device_uuid);
    let instance = format!("QBZ-{short}");
    let hostname = format!("qbz-{short}.local.");
    let properties = HashMap::from([
        ("device_uuid".to_string(), device_uuid.to_string()),
        ("sdk_version".to_string(), sdk_version.to_string()),
        ("path".to_string(), String::new()),
    ]);
    let mut info = ServiceInfo::new(
        SERVICE_TYPE,
        &instance,
        &hostname,
        addresses,
        port,
        properties,
    )?;
    info.set_interfaces(addresses.iter().copied().map(IfKind::Addr).collect());
    Ok(info)
}

fn lan_ipv4_addresses() -> Result<Vec<IpAddr>, LanError> {
    // The official receiver is reachable from every attached LAN. Advertising
    // only the interface selected by one multicast route strands controllers
    // on a second physical NIC. Point-to-point interfaces are excluded because
    // they are normally VPN/tunnel links rather than mDNS LAN segments.
    let addresses = if_addrs::get_if_addrs()
        .map_err(|_| LanError::AddressDiscovery)?
        .into_iter()
        .filter(|interface| {
            interface.is_oper_up() && !interface.is_loopback() && !interface.is_p2p()
        })
        .map(|interface| interface.ip())
        .collect::<Vec<_>>();
    let addresses = normalize_addresses(addresses);
    if addresses.is_empty() {
        return Err(LanError::NoLanAddresses);
    }
    Ok(addresses)
}

fn short_id(device_uuid: &str) -> String {
    let short = device_uuid
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .take(8)
        .collect::<String>()
        .to_ascii_lowercase();
    if short.is_empty() {
        "renderer".to_string()
    } else {
        short
    }
}

fn recover_lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(value: &str) -> IpAddr {
        value.parse().unwrap()
    }

    #[test]
    fn instance_short_id_is_dns_safe_and_stable() {
        assert_eq!(short_id("550e8400-e29b-41d4-a716-446655440000"), "550e8400");
        assert_eq!(short_id("---"), "renderer");
    }

    #[test]
    fn service_info_has_official_type_and_required_txt_union() {
        let uuid = "550e8400-e29b-41d4-a716-446655440000";
        let info = service_info(uuid, 43210, "0.9.5", &[ip("192.0.2.10")]).unwrap();

        assert_eq!(info.get_type(), SERVICE_TYPE);
        assert_eq!(info.get_port(), 43210);
        assert_eq!(info.get_property_val_str("device_uuid"), Some(uuid));
        assert_eq!(info.get_property_val_str("sdk_version"), Some("0.9.5"));
        assert_eq!(info.get_property_val_str("path"), Some(""));
        assert!(!info.is_addr_auto());
        assert_eq!(
            info.get_addresses(),
            &[ip("192.0.2.10")].into_iter().collect()
        );
    }

    #[test]
    fn address_normalization_is_ipv4_only_sorted_and_deduplicated() {
        assert_eq!(
            normalize_addresses(vec![
                ip("192.0.2.20"),
                ip("::1"),
                ip("127.0.0.1"),
                ip("0.0.0.0"),
                ip("192.0.2.10"),
                ip("192.0.2.20"),
            ]),
            vec![ip("192.0.2.10"), ip("192.0.2.20")]
        );
    }

    #[test]
    fn address_reconciliation_withdraws_recovers_and_replaces_in_order() {
        let first = vec![ip("192.0.2.10")];
        let second = vec![ip("192.0.2.20")];

        assert_eq!(
            reconcile_action(false, Some(&first), vec![]),
            ReconcileAction::Withdraw
        );
        assert_eq!(
            reconcile_action(false, None, first.clone()),
            ReconcileAction::Publish(first.clone())
        );
        assert_eq!(
            reconcile_action(false, Some(&first), second.clone()),
            ReconcileAction::Replace(second)
        );
        assert_eq!(
            reconcile_action(false, Some(&first), first.clone()),
            ReconcileAction::Unchanged
        );
    }

    #[test]
    fn stopped_reconciliation_never_republishes() {
        assert_eq!(
            reconcile_action(true, None, vec![ip("192.0.2.10")]),
            ReconcileAction::Unchanged
        );
        assert_eq!(
            reconcile_action(true, Some(&[ip("192.0.2.10")]), vec![ip("192.0.2.20")]),
            ReconcileAction::Unchanged
        );
    }

    #[test]
    fn fixed_address_events_withdraw_and_restore_only_configured_ipv4() {
        let configured = vec![ip("192.0.2.10")];

        assert_eq!(
            fixed_addresses_after_event(&configured, Some(&configured), false, ip("192.0.2.10")),
            Some(vec![])
        );
        assert_eq!(
            fixed_addresses_after_event(&configured, None, true, ip("192.0.2.10")),
            Some(configured.clone())
        );
        assert_eq!(
            fixed_addresses_after_event(&configured, Some(&configured), false, ip("192.0.2.99")),
            None
        );
    }
}
