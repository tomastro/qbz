//! Thin qbzd adapter for the official QConnect LAN receiver surface.
//!
//! This module projects the daemon's existing QConnect identity into the LAN
//! wire, owns the listener/mDNS lifetime, and drains the bounded latest-wins
//! admission slot on one dedicated thread. Cloud validation and transactional
//! authority switching deliberately remain outside this adapter.

use std::error::Error;
use std::fmt;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use qbz_models::Quality;
use qconnect_app::LanProjectionSlot;
use qconnect_lan::{
    ConnectInfo, DeviceType, DisplayInfo, EndpointPolicy, HandoffCandidate, LanError,
    LanProjection, LanService, LanServiceConfig, MaxAudioQuality,
};

use super::transport::default_qconnect_device_info;

const ADMISSION_WAIT: Duration = Duration::from_secs(60);
const DAEMON_BRAND: &str = "QBZ";
const DAEMON_MODEL: &str = "QBZ Daemon";
const ADMISSION_THREAD_NAME: &str = "qbzd-qconnect-lan-admission";

pub type HandoffCallback = Arc<dyn Fn(HandoffCandidate) + Send + Sync + 'static>;

pub(crate) type DaemonLanProjectionSlot = LanProjectionSlot;

#[derive(Debug)]
pub enum DaemonLanError {
    ExistingIdentityUnavailable,
    Service(LanError),
    AdmissionThread(std::io::Error),
}

impl fmt::Display for DaemonLanError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ExistingIdentityUnavailable => {
                formatter.write_str("qconnect LAN identity is unavailable")
            }
            Self::Service(error) => write!(formatter, "qconnect LAN service failed: {error}"),
            Self::AdmissionThread(_) => {
                formatter.write_str("qconnect LAN admission thread could not start")
            }
        }
    }
}

impl Error for DaemonLanError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Service(error) => Some(error),
            Self::AdmissionThread(error) => Some(error),
            Self::ExistingIdentityUnavailable => None,
        }
    }
}

impl From<LanError> for DaemonLanError {
    fn from(error: LanError) -> Self {
        Self::Service(error)
    }
}

/// Map the daemon's effective Qobuz quality ceiling to the exact official LAN
/// enum. `UltraHiRes` is QBZ's 24-bit/>96 kHz tier and tops out at the Qobuz
/// 192 kHz format family; QBZ does not advertise the unused 384 kHz value.
pub const fn max_audio_quality_for_quality(quality: Quality) -> MaxAudioQuality {
    match quality {
        Quality::Mp3 => MaxAudioQuality::MP3,
        Quality::Lossless => MaxAudioQuality::UpToCd,
        Quality::HiRes => MaxAudioQuality::UpToHires96,
        Quality::UltraHiRes => MaxAudioQuality::UpToHires192,
    }
}

struct ExistingIdentity {
    device_uuid: String,
    friendly_name: String,
    software_version: String,
}

impl ExistingIdentity {
    fn resolve() -> Result<Self, DaemonLanError> {
        let device_info = default_qconnect_device_info();
        let (Some(device_uuid), Some(friendly_name), Some(software_version)) = (
            device_info.device_uuid,
            device_info.friendly_name,
            device_info.software_version,
        ) else {
            return Err(DaemonLanError::ExistingIdentityUnavailable);
        };
        if device_uuid.trim().is_empty()
            || friendly_name.trim().is_empty()
            || software_version.trim().is_empty()
        {
            return Err(DaemonLanError::ExistingIdentityUnavailable);
        }
        Ok(Self {
            device_uuid,
            friendly_name,
            software_version,
        })
    }
}

fn build_projection(
    identity: &ExistingIdentity,
    app_id: String,
    quality: Quality,
    current_session_id: Option<String>,
) -> LanProjection {
    LanProjection::new(
        DisplayInfo {
            friendly_name: identity.friendly_name.clone(),
            serial_number: identity.device_uuid.clone(),
            brand_display_name: DAEMON_BRAND.to_string(),
            model_display_name: DAEMON_MODEL.to_string(),
            max_audio_quality: max_audio_quality_for_quality(quality),
            device_type: DeviceType::Streamer,
            software_version: identity.software_version.clone(),
        },
        ConnectInfo {
            app_id,
            current_session_id,
        },
    )
}

/// Owns the LAN service and its blocking admission bridge.
///
/// The callback must enqueue or otherwise hand off the candidate promptly; it
/// runs serially on the dedicated bridge thread so callback invocation order is
/// the same order observed from the latest-wins admission slot.
pub struct DaemonLanRuntime {
    service: Option<LanService>,
    projection: LanProjection,
    admission_bridge: Option<JoinHandle<()>>,
}

impl DaemonLanRuntime {
    pub fn start(
        endpoint_policy: EndpointPolicy,
        app_id: String,
        quality: Quality,
        current_session_id: Option<String>,
        on_handoff: HandoffCallback,
    ) -> Result<Self, DaemonLanError> {
        let identity = ExistingIdentity::resolve()?;
        let projection = build_projection(&identity, app_id, quality, current_session_id);
        let config =
            LanServiceConfig::new(projection.clone(), endpoint_policy, identity.device_uuid);
        let (mut service, inbox) = LanService::start(config)?;

        let admission_bridge = match std::thread::Builder::new()
            .name(ADMISSION_THREAD_NAME.to_string())
            .spawn(move || loop {
                match inbox.take_timeout(ADMISSION_WAIT) {
                    Some(candidate) => on_handoff(candidate),
                    None if inbox.is_closed() => break,
                    None => {}
                }
            }) {
            Ok(thread) => thread,
            Err(error) => {
                service.shutdown();
                return Err(DaemonLanError::AdmissionThread(error));
            }
        };

        Ok(Self {
            service: Some(service),
            projection,
            admission_bridge: Some(admission_bridge),
        })
    }

    pub fn projection(&self) -> LanProjection {
        self.projection.clone()
    }

    pub fn port(&self) -> Option<u16> {
        self.service.as_ref().map(LanService::port)
    }

    /// Idempotent teardown in the normative order: withdraw discovery and stop
    /// HTTP admission first, then wait for the now-woken bridge to exit.
    pub fn shutdown(&mut self) {
        if let Some(mut service) = self.service.take() {
            service.shutdown();
        }
        if let Some(bridge) = self.admission_bridge.take() {
            let _ = bridge.join();
        }
    }
}

impl Drop for DaemonLanRuntime {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_identity() -> ExistingIdentity {
        ExistingIdentity {
            device_uuid: "de0c0d70-0000-4000-8000-000000000001".to_string(),
            friendly_name: "Living room".to_string(),
            software_version: "qbz/2.1.0".to_string(),
        }
    }

    #[test]
    fn quality_mapping_matches_official_lan_families() {
        assert_eq!(
            max_audio_quality_for_quality(Quality::Mp3),
            MaxAudioQuality::MP3
        );
        assert_eq!(
            max_audio_quality_for_quality(Quality::Lossless),
            MaxAudioQuality::UpToCd
        );
        assert_eq!(
            max_audio_quality_for_quality(Quality::HiRes),
            MaxAudioQuality::UpToHires96
        );
        assert_eq!(
            max_audio_quality_for_quality(Quality::UltraHiRes),
            MaxAudioQuality::UpToHires192
        );
    }

    #[test]
    fn daemon_projection_uses_existing_identity_and_official_role() {
        let projection = build_projection(
            &fixture_identity(),
            "app-id".to_string(),
            Quality::UltraHiRes,
            None,
        );

        assert_eq!(
            projection.display_info(),
            DisplayInfo {
                friendly_name: "Living room".to_string(),
                serial_number: "de0c0d70-0000-4000-8000-000000000001".to_string(),
                brand_display_name: "QBZ".to_string(),
                model_display_name: "QBZ Daemon".to_string(),
                max_audio_quality: MaxAudioQuality::UpToHires192,
                device_type: DeviceType::Streamer,
                software_version: "qbz/2.1.0".to_string(),
            }
        );
        assert_eq!(
            projection.connect_info(),
            ConnectInfo {
                app_id: "app-id".to_string(),
                current_session_id: None,
            }
        );
    }

    #[test]
    fn projection_session_id_changes_only_when_explicitly_updated() {
        let projection = build_projection(
            &fixture_identity(),
            "app-id".to_string(),
            Quality::Lossless,
            None,
        );
        assert_eq!(projection.connect_info().current_session_id, None);

        projection.set_current_session_id(Some("delegated-session".to_string()));
        assert_eq!(
            projection.connect_info().current_session_id.as_deref(),
            Some("delegated-session")
        );
        projection.set_current_session_id(None);
        assert_eq!(projection.connect_info().current_session_id, None);
    }
}
