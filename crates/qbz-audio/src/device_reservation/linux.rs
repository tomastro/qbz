//! Linux implementation of `DeviceReservation`.
//!
//! Implements a client of the `org.freedesktop.ReserveDevice1` D-Bus protocol
//! (specified by the PulseAudio project, also implemented by PipeWire,
//! WirePlumber, JACK, MPD, Roon Bridge, etc.). For an ALSA device string of
//! the form `hw:N,M` (or `plughw:`, or `hw:CARD=Name`), we map the *card*
//! index `N` to the well-known bus name `org.freedesktop.ReserveDevice1.AudioN`
//! and request ownership of it.
//!
//! Acquisition algorithm (matches the spec at
//! `qbz-nix-docs/specs/2026-05-07-alsa-exclusive-hardening-design.md`):
//!
//! 1. `RequestName` with `DO_NOT_QUEUE`.
//! 2. `PrimaryOwner` / `AlreadyOwner` -> we own it. Done.
//! 3. `Exists` / `InQueue` -> someone else holds it. Read their `Priority`
//!    property; if our priority is higher, call `RequestRelease(our_priority)`
//!    on the holder. If that returns `true`, retry `RequestName` with
//!    `DO_NOT_QUEUE | REPLACE_EXISTING`.
//! 4. Refusal or equal-or-greater priority -> `HigherPriorityHolder` (the
//!    only fatal outcome). Any other failure (bus unavailable, name denied,
//!    holder unreachable, lost re-acquire race) -> degraded no-op guard;
//!    the PCM open + EBUSY backoff ladder arbitrate the device (#508/#534).
//!
//! On `Drop`, an active guard releases the bus name. A *degraded* guard
//! (returned when the session bus is unavailable) is a no-op on `Drop`.

use std::fmt;

use zbus::blocking::fdo::DBusProxy;
use zbus::blocking::{Connection, Proxy};
use zbus::fdo::{ReleaseNameReply, RequestNameFlags, RequestNameReply};
use zbus::names::WellKnownName;

/// Priority QBZ takes when acquiring a `ReserveDevice1` reservation.
///
/// Rationale (from the design spec): PulseAudio and PipeWire run at `0`, pro
/// audio software (Ardour, Bitwig, Roon Bridge) runs at `10`-`30`. We pick `5`:
/// above the system mixer so we can pre-empt PipeWire when the user toggles
/// exclusive mode, well below pro DAW software so we never stomp on a
/// recording session.
pub(crate) const QBZ_PRIORITY: i32 = 5;

/// Application name advertised over D-Bus when QBZ publishes the
/// `ReserveDevice1` interface as a server. Deferred to a future commit — see
/// `qbz-nix-docs/specs/2026-05-07-alsa-exclusive-hardening-design.md`,
/// section "The org.freedesktop.ReserveDevice1 protocol", subsections
/// "Note on `app_device_name`" / "Note on `ApplicationName`".
#[allow(dead_code)]
pub(crate) const QBZ_APPLICATION_NAME: &str = "QBZ";

/// D-Bus interface every `ReserveDevice1` holder publishes under
/// `/org/freedesktop/ReserveDevice1/AudioN`.
const RESERVE_DEVICE1_INTERFACE: &str = "org.freedesktop.ReserveDevice1";

#[derive(Debug)]
pub struct DeviceReservation {
    state: ReservationState,
}

#[derive(Debug)]
enum ReservationState {
    /// We own the bus name `bus_name` on `connection`. `Drop` releases it.
    /// `app_device_name` is stashed for Task 5 (status payload) — kept private.
    Active {
        connection: Connection,
        bus_name: String,
        #[allow(dead_code)] // Surfaced via Tauri status command in Task 5.
        app_device_name: String,
    },
    /// D-Bus session bus was unreachable, or some other graceful-degrade
    /// path. `is_active()` reports `false`; `Drop` is a no-op.
    Degraded,
}

impl DeviceReservation {
    /// Acquire a D-Bus device reservation for the given ALSA `hw:` device.
    ///
    /// Returns:
    /// - `Ok(active_guard)` once we own the bus name.
    /// - `Ok(degraded_guard)` for every non-cooperative failure: unparseable
    ///   device strings, an unreachable session bus, a denied `RequestName`
    ///   (confined sandbox bus, #534), or contention resolution failing for
    ///   any reason other than the holder refusing. The caller treats these
    ///   as "no reservation, but proceed normally" — the PCM open and its
    ///   EBUSY backoff ladder arbitrate the device (#508).
    /// - `Err(HigherPriorityHolder { .. })` — the only fatal outcome: a
    ///   cooperative holder refused to release, or holds at
    ///   equal-or-greater priority.
    ///
    /// # Tight coupling rule (load-bearing — do not violate)
    ///
    /// A `DeviceReservation` MUST always be created in tight coupling with an
    /// immediate consumer of the underlying ALSA device:
    ///
    /// - Lifetime A: held inside [`AlsaDirectStream`] for as long as the PCM
    ///   is open. Acquired before `PCM::new`, dropped after the PCM is closed.
    /// - Lifetime B: held inside the application's `AppState` for as long as
    ///   the QBZ process is running, gated by the `reserve_dac_while_running`
    ///   setting. Acquired at startup or on toggle, dropped at process exit.
    ///
    /// **Never construct a `DeviceReservation` in isolation, hold it briefly,
    /// and drop it without a real device consumer in between.** The pattern
    ///
    /// ```ignore
    /// let r = DeviceReservation::acquire("hw:1,0", "test")?;
    /// std::thread::sleep(Duration::from_secs(2));
    /// drop(r);
    /// ```
    ///
    /// triggers WirePlumber to release-and-reacquire the device over an idle
    /// PCM, and some USB DACs (Cambridge DacMagic Plus confirmed, others
    /// suspected) get stranded by that transition and require a hardware
    /// power-cycle to recover. Validated 2026-05-07. Tests must always go
    /// through `AlsaDirectStream::new()` (Lifetime A) or hold the reservation
    /// for the entire process lifetime (Lifetime B).
    ///
    /// # Connection lifecycle
    ///
    /// `acquire` opens a fresh `zbus::blocking::Connection::session()` per
    /// call. zbus 4.4 does not internally pool session-bus connections, so
    /// each call pays a SASL handshake cost (~1-5 ms on a healthy bus). For
    /// per-stream (Lifetime A) acquisition this is acceptable. For the
    /// nested-inside-Lifetime-B pattern landing in Task 5, prefer reusing an
    /// existing connection via the future `acquire_with_connection` overload.
    pub fn acquire(hw_device: &str, app_device_name: &str) -> Result<Self, ReservationError> {
        // Parse failures must not propagate — the caller (AlsaDirectStream)
        // will treat any Err as fatal and abort stream creation, regressing
        // playback for devices we can't introspect. Names that don't target
        // a single card (`default`, `pulse`) and any future plugin alias we
        // can't decode degrade to a no-op guard so PCM open proceeds.
        let card = match parse_card_index(hw_device) {
            Ok(idx) => idx,
            Err(e) => {
                log::warn!(
                    "[reservation] cannot determine ALSA card index for '{}': {}. \
                     Proceeding without D-Bus reservation (degraded).",
                    hw_device,
                    e
                );
                return Ok(Self {
                    state: ReservationState::Degraded,
                });
            }
        };
        let bus_name = bus_name_for_card(card);
        let object_path = object_path_for_card(card);

        // Connect to the session bus. Failure here is *not* an error from the
        // caller's perspective — we degrade and let playback proceed.
        let connection = match Connection::session() {
            Ok(c) => c,
            Err(e) => {
                log::warn!(
                    "[reservation] D-Bus session bus unavailable, degrading: {}",
                    e
                );
                return Ok(Self {
                    state: ReservationState::Degraded,
                });
            }
        };

        // RequestName itself can be denied by the bus policy — under Snap
        // confinement the mediated session bus refuses ownership of the
        // org.freedesktop.ReserveDevice1.* names (#534). That must not be
        // fatal: degrade to no reservation and let the PCM open proceed,
        // exactly like the bus-unavailable case above.
        let reply = match try_acquire_name(&connection, &bus_name, false) {
            Ok(reply) => reply,
            Err(e) => {
                log::warn!(
                    "[reservation] could not request name {} (confined session \
                     bus?): {}. Proceeding without D-Bus reservation (degraded).",
                    bus_name,
                    e
                );
                return Ok(Self {
                    state: ReservationState::Degraded,
                });
            }
        };

        match reply {
            // Either we just took ownership, or we already owned this name on
            // this same connection (idempotent for Lifetime-A nested under
            // Lifetime-B in Task 5).
            RequestNameReply::PrimaryOwner | RequestNameReply::AlreadyOwner => {
                log::debug!("[reservation] acquired {}", bus_name);
                Ok(Self {
                    state: ReservationState::Active {
                        connection,
                        bus_name,
                        app_device_name: app_device_name.to_string(),
                    },
                })
            }
            // Someone else holds it (or is queued). Check their priority and
            // ask them to step aside.
            //
            // DO_NOT_QUEUE makes InQueue unreachable in practice, but
            // RequestNameReply is exhaustively matched and the contention
            // logic handles it identically to Exists.
            RequestNameReply::Exists | RequestNameReply::InQueue => {
                match resolve_contention(&connection, &bus_name, &object_path, app_device_name) {
                    Ok(res) => Ok(res),
                    // A cooperative higher-priority holder (DAW / pro-audio app)
                    // refused to release: honor it — the one deliberate fatal case.
                    Err(e @ ReservationError::HigherPriorityHolder { .. }) => Err(e),
                    // Anything else (holder proxy unreachable, RequestRelease call
                    // failed, lost re-acquire race): degrade — the PCM open plus its
                    // EBUSY backoff ladder is the real arbiter, exactly like the
                    // request_name-failure path above (#508/#534).
                    Err(e) => {
                        log::warn!(
                            "[reservation] contention resolution for {} failed: {}. \
                             Proceeding without D-Bus reservation (degraded).",
                            bus_name,
                            e
                        );
                        Ok(Self {
                            state: ReservationState::Degraded,
                        })
                    }
                }
            }
        }
    }

    /// Whether this guard currently holds an active D-Bus reservation.
    pub fn is_active(&self) -> bool {
        matches!(self.state, ReservationState::Active { .. })
    }
}

impl Drop for DeviceReservation {
    fn drop(&mut self) {
        if let ReservationState::Active {
            connection,
            bus_name,
            ..
        } = &self.state
        {
            match release_name(connection, bus_name) {
                Ok(ReleaseNameReply::Released) => {
                    log::debug!("[reservation] released {}", bus_name);
                }
                Ok(ReleaseNameReply::NonExistent) => {
                    // We thought we owned it but the bus daemon disagrees.
                    // Almost always indicates a logic bug in our state
                    // tracking — surface loudly.
                    log::warn!(
                        "[reservation] release_name returned NonExistent for {} \
                         (we believed we owned it)",
                        bus_name
                    );
                }
                Ok(ReleaseNameReply::NotOwner) => {
                    log::warn!(
                        "[reservation] release_name returned NotOwner for {} \
                         (we believed we owned it)",
                        bus_name
                    );
                }
                Err(e) => {
                    log::warn!("[reservation] release_name failed for {}: {}", bus_name, e);
                }
            }
        }
    }
}

#[derive(Debug)]
pub enum ReservationError {
    InvalidDevice(String),
    HigherPriorityHolder {
        holder_name: String,
        holder_priority: i32,
    },
    DbusError(String),
    AlsaError(String),
}

impl fmt::Display for ReservationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidDevice(s) => write!(f, "invalid ALSA device string: {}", s),
            Self::HigherPriorityHolder {
                holder_name,
                holder_priority,
            } => write!(
                f,
                "device reserved by '{}' at priority {}",
                holder_name, holder_priority
            ),
            Self::DbusError(s) => write!(f, "D-Bus error: {}", s),
            Self::AlsaError(s) => write!(f, "ALSA error: {}", s),
        }
    }
}

impl std::error::Error for ReservationError {}

/// Issue `RequestName` for `bus_name`. Always sets `DO_NOT_QUEUE`; sets
/// `REPLACE_EXISTING` when `replace` is true (used after a successful
/// `RequestRelease` on the previous holder).
///
/// Returns the `RequestNameReply` on success. zbus errors are surfaced as
/// `ReservationError::DbusError`.
fn try_acquire_name(
    conn: &Connection,
    bus_name: &str,
    replace: bool,
) -> Result<RequestNameReply, ReservationError> {
    let proxy = DBusProxy::new(conn)
        .map_err(|e| ReservationError::DbusError(format!("DBusProxy::new failed: {}", e)))?;
    let well_known: WellKnownName<'_> = bus_name.try_into().map_err(|e| {
        ReservationError::DbusError(format!("invalid bus name '{}': {}", bus_name, e))
    })?;
    let flags = if replace {
        RequestNameFlags::DoNotQueue | RequestNameFlags::ReplaceExisting
    } else {
        RequestNameFlags::DoNotQueue.into()
    };
    proxy
        .request_name(well_known, flags)
        .map_err(|e| ReservationError::DbusError(format!("request_name failed: {}", e)))
}

/// Release the bus name. Pure forward of the zbus reply variant; the caller
/// (`Drop`) decides what to log. Returns `zbus::fdo::Error` which already
/// implements `Display`.
fn release_name(conn: &Connection, bus_name: &str) -> zbus::fdo::Result<ReleaseNameReply> {
    let proxy = DBusProxy::new(conn)?;
    // .map_err(zbus::Error::from) bridges names crate -> zbus::Error;
    // ? then bridges zbus::Error -> fdo::Error.
    let well_known: WellKnownName<'_> = bus_name.try_into().map_err(zbus::Error::from)?;
    proxy.release_name(well_known)
}

/// We tried to acquire a held bus name and got `Exists` (or `InQueue`).
/// Inspect the holder's `Priority`, decide whether to ask them to release,
/// and either retry or return a `HigherPriorityHolder` error.
fn resolve_contention(
    conn: &Connection,
    bus_name: &str,
    object_path: &str,
    app_device_name: &str,
) -> Result<DeviceReservation, ReservationError> {
    // One Proxy serves all reads + the RequestRelease call against the
    // current holder. zbus's Proxy is cheap to keep alive, but constructing
    // it twice for back-to-back property reads has been observed to cost an
    // extra GetAll round trip on some bus daemons; one proxy is both faster
    // and clearer.
    let holder_proxy = open_holder_proxy(conn, bus_name, object_path)
        .map_err(|e| ReservationError::DbusError(format!("Proxy::new for holder failed: {}", e)))?;

    // Default to 0 if the holder is uncooperative or doesn't expose Priority.
    // Rationale: PulseAudio/PipeWire are the most common holders and run at
    // priority 0; treating an unreadable priority as 0 lets us still pre-empt
    // them. Pro apps that *do* publish at higher priority will refuse via
    // RequestRelease anyway, which we honour below.
    let holder_priority = read_priority(&holder_proxy).unwrap_or(0);

    if QBZ_PRIORITY <= holder_priority {
        let holder_name = read_application_name(&holder_proxy)
            .unwrap_or_else(|| "another application".to_string());
        log::info!(
            "[reservation] {} held by {} at priority {} (>= ours {}); refusing",
            bus_name,
            holder_name,
            holder_priority,
            QBZ_PRIORITY
        );
        return Err(ReservationError::HigherPriorityHolder {
            holder_name,
            holder_priority,
        });
    }

    log::debug!(
        "[reservation] {} held at priority {}; calling RequestRelease({})",
        bus_name,
        holder_priority,
        QBZ_PRIORITY
    );

    let released = request_release(&holder_proxy, QBZ_PRIORITY)?;
    if !released {
        let holder_name = read_application_name(&holder_proxy)
            .unwrap_or_else(|| "another application".to_string());
        log::info!(
            "[reservation] {} held by {} refused RequestRelease",
            bus_name,
            holder_name
        );
        return Err(ReservationError::HigherPriorityHolder {
            holder_name,
            holder_priority,
        });
    }

    // Holder agreed to release. Retry with REPLACE_EXISTING.
    match try_acquire_name(conn, bus_name, true)? {
        RequestNameReply::PrimaryOwner | RequestNameReply::AlreadyOwner => {
            log::debug!("[reservation] acquired {} after RequestRelease", bus_name);
            Ok(DeviceReservation {
                state: ReservationState::Active {
                    connection: conn.clone(),
                    bus_name: bus_name.to_string(),
                    app_device_name: app_device_name.to_string(),
                },
            })
        }
        RequestNameReply::Exists | RequestNameReply::InQueue => {
            // Someone slipped in between the holder releasing and us
            // re-requesting. Rare; surfaces as a generic D-Bus error.
            Err(ReservationError::DbusError(format!(
                "lost race after holder released {}",
                bus_name
            )))
        }
    }
}

/// Open a `Proxy` for the current holder's `ReserveDevice1` interface. Used
/// for property reads (`Priority`, `ApplicationName`) and the `RequestRelease`
/// call; the same proxy handle serves all three so we only pay one
/// construction cost per contention case.
fn open_holder_proxy<'a>(
    conn: &'a Connection,
    bus_name: &'a str,
    object_path: &'a str,
) -> zbus::Result<Proxy<'a>> {
    Proxy::new(conn, bus_name, object_path, RESERVE_DEVICE1_INTERFACE)
}

/// Read the holder's `Priority` property via `org.freedesktop.DBus.Properties.Get`.
/// Returns `None` if the holder is uncooperative (no such property, type
/// mismatch, etc.); the caller treats that as priority 0.
fn read_priority(proxy: &Proxy<'_>) -> Option<i32> {
    proxy.get_property::<i32>("Priority").ok()
}

/// Read the holder's `ApplicationName` property. Used purely for human-readable
/// error messages in `HigherPriorityHolder`.
fn read_application_name(proxy: &Proxy<'_>) -> Option<String> {
    proxy.get_property::<String>("ApplicationName").ok()
}

/// Call `RequestRelease(priority)` on the current holder via the shared
/// proxy. Returns the holder's reply (`true` = will release, `false` =
/// refuses).
fn request_release(proxy: &Proxy<'_>, priority: i32) -> Result<bool, ReservationError> {
    proxy
        .call::<_, _, bool>("RequestRelease", &(priority,))
        .map_err(|e| ReservationError::DbusError(format!("RequestRelease failed: {}", e)))
}

/// Parse an ALSA device string and return the kernel card index.
///
/// Accepts any plugin prefix (`hw:`, `plughw:`, `front:`, `surround*:`,
/// `iec958:`, `hdmi:`, etc.) followed by either a positional numeric card
/// index (`hw:1,0`) or a `CARD=<name>` argument in any position
/// (`front:CARD=C20,DEV=0`, `hw:DEV=0,CARD=DacMagic`). The plugin prefix
/// itself is irrelevant for our purpose: D-Bus reservation is per-card,
/// not per-plugin alias.
///
/// Strings with no colon (`default`, `pulse`, etc.) and strings whose
/// args contain neither a `CARD=` argument nor a numeric first arg are
/// rejected as `InvalidDevice`. The caller (`acquire`) downgrades these
/// to a degraded guard rather than propagating the error.
pub(crate) fn parse_card_index(hw_device: &str) -> Result<u32, ReservationError> {
    let trimmed = hw_device.trim();

    // Find the first colon. Everything before is the plugin name (hw,
    // plughw, front, surround*, iec958, hdmi, etc.); we only care about
    // the args after it. Strings with no colon are not plugin-prefixed
    // device names and cannot identify a card.
    let args = match trimmed.find(':') {
        Some(idx) => trimmed[idx + 1..].trim(),
        None => return Err(ReservationError::InvalidDevice(hw_device.to_string())),
    };

    if args.is_empty() {
        return Err(ReservationError::InvalidDevice(hw_device.to_string()));
    }

    // Args are comma-separated. Look for `CARD=<name>` in any position;
    // if none, fall back to the first arg parsed as a numeric card index.
    let mut card_name: Option<&str> = None;
    let mut first_arg: Option<&str> = None;

    for (i, arg) in args.split(',').enumerate() {
        let arg = arg.trim();
        if let Some(name) = arg.strip_prefix("CARD=") {
            card_name = Some(name);
        }
        if i == 0 {
            first_arg = Some(arg);
        }
    }

    if let Some(name) = card_name {
        if name.is_empty() {
            return Err(ReservationError::InvalidDevice(hw_device.to_string()));
        }
        return resolve_card_index_by_name(name);
    }

    if let Some(arg) = first_arg {
        if let Ok(n) = arg.parse::<u32>() {
            return Ok(n);
        }
    }

    Err(ReservationError::InvalidDevice(hw_device.to_string()))
}

/// Resolve a symbolic ALSA card name (e.g., `"C20"`, `"DacMagic"`, `"PCH"`)
/// to its kernel index by iterating over `alsa::card::Iter`.
///
/// ALSA's `CARD=` parameter takes the card's *id* (short identifier like
/// `"C20"`, `"PCH"`, `"Generic"`, `"HDMI"`) — NOT the long descriptive name
/// (`"Cambridge Audio USB Audio 2.0"`). Verified against `aplay -l` output
/// on the user's host:
///
/// ```text
///     card 1: C20 [Cambridge Audio USB Audio 2.0], device 0: ...
///              ^id ^long name
/// ```
///
/// In `alsa-rs` 0.10, the short id is exposed as `ctl::CardInfo::get_id()`
/// (a wrapper around `snd_ctl_card_info_get_id`). The convenience
/// `Card::get_name()` actually returns the long name (it wraps
/// `snd_card_get_name`, which despite the name returns the descriptive
/// "Cambridge Audio USB Audio 2.0" string), so we cannot use it here. We
/// open a `Ctl` per card and read the id from its `CardInfo`.
fn resolve_card_index_by_name(name: &str) -> Result<u32, ReservationError> {
    for card in alsa::card::Iter::new() {
        let card = card.map_err(|e| ReservationError::AlsaError(e.to_string()))?;
        let ctl = alsa::ctl::Ctl::from_card(&card, true)
            .map_err(|e| ReservationError::AlsaError(e.to_string()))?;
        let info = ctl
            .card_info()
            .map_err(|e| ReservationError::AlsaError(e.to_string()))?;
        let id = info
            .get_id()
            .map_err(|e| ReservationError::AlsaError(e.to_string()))?;
        if id == name {
            return Ok(card.get_index() as u32);
        }
    }
    Err(ReservationError::InvalidDevice(format!(
        "ALSA card '{}' not found",
        name
    )))
}

/// Format the well-known D-Bus bus name for a given ALSA card index.
pub(crate) fn bus_name_for_card(card_index: u32) -> String {
    format!("org.freedesktop.ReserveDevice1.Audio{}", card_index)
}

/// Format the D-Bus object path for a given ALSA card index.
pub(crate) fn object_path_for_card(card_index: u32) -> String {
    format!("/org/freedesktop/ReserveDevice1/Audio{}", card_index)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_card_index_basic() {
        assert_eq!(parse_card_index("hw:0").unwrap(), 0);
        assert_eq!(parse_card_index("hw:1,0").unwrap(), 1);
        assert_eq!(parse_card_index("plughw:2,0").unwrap(), 2);
        assert_eq!(parse_card_index("hw:99,3").unwrap(), 99);
    }

    #[test]
    fn parse_card_index_rejects_garbage() {
        assert!(matches!(
            parse_card_index("default"),
            Err(ReservationError::InvalidDevice(_))
        ));
        assert!(matches!(
            parse_card_index("hw:"),
            Err(ReservationError::InvalidDevice(_))
        ));
        assert!(matches!(
            parse_card_index(""),
            Err(ReservationError::InvalidDevice(_))
        ));
    }

    #[test]
    fn bus_name_format() {
        assert_eq!(
            bus_name_for_card(0),
            "org.freedesktop.ReserveDevice1.Audio0"
        );
        assert_eq!(
            bus_name_for_card(7),
            "org.freedesktop.ReserveDevice1.Audio7"
        );
        assert_eq!(
            bus_name_for_card(99),
            "org.freedesktop.ReserveDevice1.Audio99"
        );
    }

    #[test]
    fn object_path_format() {
        assert_eq!(
            object_path_for_card(0),
            "/org/freedesktop/ReserveDevice1/Audio0"
        );
    }

    #[test]
    fn parse_card_index_accepts_any_plugin_prefix() {
        // The plugin prefix is irrelevant; what matters is whether we can
        // extract a card identifier from the args. Use positional numeric
        // first args so the assertions don't depend on real ALSA cards
        // being present on the test host.
        assert_eq!(parse_card_index("front:0,0").unwrap(), 0);
        assert_eq!(parse_card_index("plughw:1,0").unwrap(), 1);
        assert_eq!(parse_card_index("surround51:2,0").unwrap(), 2);
        assert_eq!(parse_card_index("iec958:3,0").unwrap(), 3);
        assert_eq!(parse_card_index("hdmi:4,0").unwrap(), 4);
    }

    #[test]
    fn parse_card_index_rejects_card_alias_strings() {
        // Strings with no colon cannot identify a plugin+card pair. They
        // graceful-degrade in acquire() rather than block stream creation.
        assert!(matches!(
            parse_card_index("default"),
            Err(ReservationError::InvalidDevice(_))
        ));
        assert!(matches!(
            parse_card_index("pulse"),
            Err(ReservationError::InvalidDevice(_))
        ));
        assert!(matches!(
            parse_card_index(""),
            Err(ReservationError::InvalidDevice(_))
        ));
    }

    #[test]
    fn parse_card_index_rejects_empty_args() {
        // Plugin prefix with a colon but no args is unparseable.
        assert!(matches!(
            parse_card_index("hw:"),
            Err(ReservationError::InvalidDevice(_))
        ));
        assert!(matches!(
            parse_card_index("front:"),
            Err(ReservationError::InvalidDevice(_))
        ));
    }

    #[test]
    fn parse_card_index_card_in_any_position() {
        // CARD= can appear at any position. The user's actual device
        // string is `front:CARD=C20,DEV=0` (CARD= first); the parser
        // must also handle CARD= second or as the only arg. Full
        // resolution requires a real ALSA card on the host, so we just
        // verify the parser does not panic and returns Ok or
        // InvalidDevice (never DbusError or AlsaError unless ALSA
        // enumeration itself fails).
        for s in [
            "front:DEV=0,CARD=C20",
            "hw:CARD=DacMagic",
            "plughw:CARD=DacMagic,DEV=0",
            "front:CARD=C20,DEV=0",
        ] {
            match parse_card_index(s) {
                Ok(_) | Err(ReservationError::InvalidDevice(_)) => {}
                Err(other) => panic!("unexpected error variant for {:?}: {:?}", s, other),
            }
        }
    }

    #[test]
    fn degraded_guard_reports_inactive() {
        // Construct a degraded guard directly. We cannot rely on
        // `acquire("hw:0,0", "test")` here because once Task 2 wires the
        // real D-Bus client, that call may succeed (returning an *active*
        // guard) on a developer machine running PipeWire.
        let g = DeviceReservation {
            state: ReservationState::Degraded,
        };
        assert!(!g.is_active());
        // Drop must be a no-op for a degraded guard. Implicit via end-of-scope.
    }
}
