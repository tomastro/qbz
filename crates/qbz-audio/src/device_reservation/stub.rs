//! Non-Linux stub for `DeviceReservation`.
//!
//! The org.freedesktop.ReserveDevice1 protocol is a Linux/D-Bus convention.
//! On macOS and Windows, `acquire()` always succeeds with a degraded guard
//! so that call sites stay portable; `is_active()` always returns `false`.

#[derive(Debug)]
pub struct DeviceReservation;

impl DeviceReservation {
    /// Acquire a reservation for the given ALSA hw: device string.
    ///
    /// On non-Linux platforms this always returns a degraded guard (no-op).
    pub fn acquire(_hw_device: &str, _app_device_name: &str) -> Result<Self, ReservationError> {
        Ok(Self)
    }

    /// Whether this guard holds an active D-Bus reservation.
    ///
    /// Always `false` on non-Linux platforms.
    pub fn is_active(&self) -> bool {
        false
    }
}

impl Drop for DeviceReservation {
    /// Empty drop on non-Linux platforms; preserves RAII shape symmetry
    /// with the Linux variant so callers can rely on the same lifetime model.
    fn drop(&mut self) {}
}

/// Stub: variants exist for cross-platform pattern-matching parity with
/// the Linux `ReservationError`, but are never constructed on this target
/// (`acquire()` always returns a degraded `Ok`). The `#[allow(dead_code)]`
/// is intentional and load-bearing for downstream `match` blocks (e.g.,
/// the `DacReservationStatus` mapping introduced in Task 5) which must
/// compile on every platform.
#[derive(Debug)]
#[allow(dead_code)]
pub enum ReservationError {
    InvalidDevice(String),
    HigherPriorityHolder {
        holder_name: String,
        holder_priority: i32,
    },
    DbusError(String),
    AlsaError(String),
}

impl std::fmt::Display for ReservationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
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
