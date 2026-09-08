//! Shared, stamp-aware projection of QConnect authority into the LAN wire.

use std::sync::{Arc, Mutex, MutexGuard};

use qconnect_lan::LanProjection;

use crate::{AuthorityCell, AuthorityOrigin, AuthorityStamp};

/// Serializes runtime authority changes with the synchronous LAN projection.
///
/// A stale runtime can therefore neither overwrite the public session of its
/// replacement nor clear that replacement after an asynchronous task returns.
#[derive(Clone, Default)]
pub struct LanProjectionSlot {
    inner: Arc<Mutex<ProjectionSlotState>>,
}

#[derive(Default)]
struct ProjectionSlotState {
    projection: Option<LanProjection>,
    installed: Option<InstalledSessionProjection>,
}

struct InstalledSessionProjection {
    stamp: AuthorityStamp,
    session_id: Option<String>,
}

impl LanProjectionSlot {
    fn lock(&self) -> MutexGuard<'_, ProjectionSlotState> {
        self.inner.lock().unwrap_or_else(|poisoned| {
            log::error!("[QConnect LAN] projection slot recovered from a poisoned lock");
            let guard = poisoned.into_inner();
            self.inner.clear_poison();
            guard
        })
    }

    fn write_projection(state: &ProjectionSlotState, session_id: Option<String>) {
        if let Some(projection) = state.projection.as_ref() {
            projection.set_current_session_id(session_id);
        }
    }

    /// Install a runtime and its already-confirmed public session atomically.
    /// `session_id` is `None` for an owner awaiting its first SESSION_STATE.
    pub fn install_authority(
        &self,
        authority: &AuthorityCell,
        stamp: AuthorityStamp,
        session_id: Option<&str>,
    ) -> bool {
        let mut state = self.lock();

        // Fail closed during the tiny install boundary: the old session may no
        // longer be current, while exposing the candidate before install would
        // violate the handoff transaction.
        Self::write_projection(&state, None);
        if !authority.install(stamp) {
            let current = authority.current();
            let restore = state
                .installed
                .as_ref()
                .filter(|installed| Some(installed.stamp) == current)
                .and_then(|installed| installed.session_id.clone());
            if current != state.installed.as_ref().map(|installed| installed.stamp) {
                state.installed = None;
            }
            Self::write_projection(&state, restore);
            return false;
        }

        let session_id = session_id.map(str::to_string);
        state.installed = Some(InstalledSessionProjection {
            stamp,
            session_id: session_id.clone(),
        });
        Self::write_projection(&state, session_id);
        true
    }

    /// Publish a cloud-confirmed owner session from the exact installed
    /// runtime. Delegated session ids enter only through `install_authority`.
    pub fn confirm_owner_session(
        &self,
        authority: &AuthorityCell,
        stamp: AuthorityStamp,
        session_id: &str,
    ) -> bool {
        if stamp.origin() != AuthorityOrigin::Owner || session_id.is_empty() {
            return false;
        }

        let mut state = self.lock();
        if !authority.is_current(stamp)
            || state.installed.as_ref().map(|installed| installed.stamp) != Some(stamp)
        {
            return false;
        }

        let session_id = session_id.to_string();
        if let Some(installed) = state.installed.as_mut() {
            installed.session_id = Some(session_id.clone());
        }
        Self::write_projection(&state, Some(session_id));
        true
    }

    /// Snapshot the installed session for listener construction.
    pub fn current_session_id(
        &self,
        authority: &AuthorityCell,
        expected: AuthorityStamp,
    ) -> Option<String> {
        let state = self.lock();
        if !authority.is_current(expected) {
            return None;
        }
        state
            .installed
            .as_ref()
            .filter(|installed| installed.stamp == expected)
            .and_then(|installed| installed.session_id.clone())
    }

    pub fn attach(&self, authority: &AuthorityCell, projection: LanProjection) {
        let mut state = self.lock();
        let current = authority.current();
        let session_id = state
            .installed
            .as_ref()
            .filter(|installed| Some(installed.stamp) == current)
            .and_then(|installed| installed.session_id.clone());
        projection.set_current_session_id(session_id);
        state.projection = Some(projection);
    }

    pub fn detach(&self) {
        self.lock().projection.take();
    }

    pub fn clear_authority(&self, authority: &AuthorityCell) -> Option<AuthorityStamp> {
        let mut state = self.lock();
        Self::write_projection(&state, None);
        let previous = authority.clear();
        state.installed = None;
        previous
    }

    pub fn clear_if_current(&self, authority: &AuthorityCell, expected: AuthorityStamp) -> bool {
        let mut state = self.lock();
        if !authority.is_current(expected) {
            return false;
        }

        Self::write_projection(&state, None);
        if authority.clear_if_current(expected) {
            state.installed = None;
            true
        } else {
            let restore = state
                .installed
                .as_ref()
                .filter(|installed| authority.is_current(installed.stamp))
                .and_then(|installed| installed.session_id.clone());
            Self::write_projection(&state, restore);
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use qconnect_lan::{ConnectInfo, DeviceType, DisplayInfo, LanProjection, MaxAudioQuality};

    use super::*;

    fn projection() -> LanProjection {
        LanProjection::new(
            DisplayInfo {
                friendly_name: "QBZ".to_string(),
                serial_number: "device-id".to_string(),
                brand_display_name: "QBZ".to_string(),
                model_display_name: "QBZ Desktop".to_string(),
                max_audio_quality: MaxAudioQuality::UpToCd,
                device_type: DeviceType::Computer,
                software_version: "qbz/test".to_string(),
            },
            ConnectInfo {
                app_id: "app-id".to_string(),
                current_session_id: None,
            },
        )
    }

    #[test]
    fn publishes_only_the_exact_installed_authority() {
        let authority = AuthorityCell::new();
        let slot = LanProjectionSlot::default();
        let projection = projection();
        slot.attach(&authority, projection.clone());

        let owner = authority.reserve(AuthorityOrigin::Owner);
        assert!(slot.install_authority(&authority, owner, None));
        assert!(slot.confirm_owner_session(&authority, owner, "owner-session"));

        let guest = authority.reserve(AuthorityOrigin::Delegated { generation: 7 });
        assert_eq!(
            projection.connect_info().current_session_id.as_deref(),
            Some("owner-session")
        );
        assert!(slot.install_authority(&authority, guest, Some("guest-session")));

        assert!(!slot.confirm_owner_session(&authority, owner, "stale-owner"));
        assert!(!slot.confirm_owner_session(&authority, guest, "guest-event"));
        assert_eq!(
            projection.connect_info().current_session_id.as_deref(),
            Some("guest-session")
        );
    }

    #[test]
    fn stale_install_and_clear_preserve_the_replacement_projection() {
        let authority = AuthorityCell::new();
        let slot = LanProjectionSlot::default();
        let projection = projection();
        slot.attach(&authority, projection.clone());

        let first = authority.reserve(AuthorityOrigin::Owner);
        assert!(slot.install_authority(&authority, first, Some("first")));
        let replacement = authority.reserve(AuthorityOrigin::Owner);
        assert!(slot.install_authority(&authority, replacement, Some("replacement")));

        assert!(!slot.install_authority(&authority, first, Some("stale")));
        assert!(!slot.clear_if_current(&authority, first));
        assert_eq!(
            slot.current_session_id(&authority, replacement).as_deref(),
            Some("replacement")
        );
        assert_eq!(
            projection.connect_info().current_session_id.as_deref(),
            Some("replacement")
        );
    }

    #[test]
    fn attach_seeds_current_session_and_detach_stops_projection_writes() {
        let authority = AuthorityCell::new();
        let slot = LanProjectionSlot::default();
        let owner = authority.reserve(AuthorityOrigin::Owner);
        assert!(slot.install_authority(&authority, owner, Some("owner-session")));

        let projection = projection();
        slot.attach(&authority, projection.clone());
        assert_eq!(
            projection.connect_info().current_session_id.as_deref(),
            Some("owner-session")
        );

        slot.detach();
        assert_eq!(slot.clear_authority(&authority), Some(owner));
        assert_eq!(
            projection.connect_info().current_session_id.as_deref(),
            Some("owner-session")
        );
    }
}
