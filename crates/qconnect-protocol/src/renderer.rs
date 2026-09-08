use qconnect_core::QueueVersion;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// QConnect renderer buffer-state wire values observed in the official
/// clients. Keep these separate from `playing_state`: pause/play and buffer
/// readiness are independent axes.
#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RendererBufferState {
    Buffering = 1,
    Ok = 2,
    Error = 3,
    Underrun = 4,
}

impl RendererBufferState {
    pub const fn as_i32(self) -> i32 {
        self as i32
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RendererCommandType {
    SrvrRndrSetState,
    SrvrRndrSetVolume,
    SrvrRndrSetActive,
    SrvrRndrSetMaxAudioQuality,
    SrvrRndrSetLoopMode,
    SrvrRndrSetShuffleMode,
    SrvrRndrMuteVolume,
}

impl RendererCommandType {
    pub const fn as_message_type(self) -> &'static str {
        match self {
            Self::SrvrRndrSetState => "MESSAGE_TYPE_SRVR_RNDR_SET_STATE",
            Self::SrvrRndrSetVolume => "MESSAGE_TYPE_SRVR_RNDR_SET_VOLUME",
            Self::SrvrRndrSetActive => "MESSAGE_TYPE_SRVR_RNDR_SET_ACTIVE",
            Self::SrvrRndrSetMaxAudioQuality => "MESSAGE_TYPE_SRVR_RNDR_SET_MAX_AUDIO_QUALITY",
            Self::SrvrRndrSetLoopMode => "MESSAGE_TYPE_SRVR_RNDR_SET_LOOP_MODE",
            Self::SrvrRndrSetShuffleMode => "MESSAGE_TYPE_SRVR_RNDR_SET_SHUFFLE_MODE",
            Self::SrvrRndrMuteVolume => "MESSAGE_TYPE_SRVR_RNDR_MUTE_VOLUME",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RendererServerCommand {
    pub command_type: RendererCommandType,
    #[serde(default)]
    pub payload: Value,
}

impl RendererServerCommand {
    pub const fn message_type(&self) -> &'static str {
        self.command_type.as_message_type()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RendererReportType {
    RndrSrvrJoinSession,
    RndrSrvrDeviceInfoUpdated,
    RndrSrvrStateUpdated,
    RndrSrvrVolumeChanged,
    RndrSrvrVolumeMuted,
    RndrSrvrFileAudioQualityChanged,
    RndrSrvrDeviceAudioQualityChanged,
    RndrSrvrMaxAudioQualityChanged,
}

impl RendererReportType {
    pub const fn as_message_type(self) -> &'static str {
        match self {
            Self::RndrSrvrJoinSession => "MESSAGE_TYPE_RNDR_SRVR_JOIN_SESSION",
            Self::RndrSrvrDeviceInfoUpdated => "MESSAGE_TYPE_RNDR_SRVR_DEVICE_INFO_UPDATED",
            Self::RndrSrvrStateUpdated => "MESSAGE_TYPE_RNDR_SRVR_STATE_UPDATED",
            Self::RndrSrvrVolumeChanged => "MESSAGE_TYPE_RNDR_SRVR_VOLUME_CHANGED",
            Self::RndrSrvrVolumeMuted => "MESSAGE_TYPE_RNDR_SRVR_VOLUME_MUTED",
            Self::RndrSrvrFileAudioQualityChanged => {
                "MESSAGE_TYPE_RNDR_SRVR_FILE_AUDIO_QUALITY_CHANGED"
            }
            Self::RndrSrvrDeviceAudioQualityChanged => {
                "MESSAGE_TYPE_RNDR_SRVR_DEVICE_AUDIO_QUALITY_CHANGED"
            }
            Self::RndrSrvrMaxAudioQualityChanged => {
                "MESSAGE_TYPE_RNDR_SRVR_MAX_AUDIO_QUALITY_CHANGED"
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RendererReport {
    pub report_type: RendererReportType,
    pub action_uuid: String,
    pub queue_version_ref: QueueVersion,
    #[serde(default)]
    pub payload: Value,
}

impl RendererReport {
    pub fn new(
        report_type: RendererReportType,
        action_uuid: impl Into<String>,
        queue_version_ref: QueueVersion,
        payload: Value,
    ) -> Self {
        Self {
            report_type,
            action_uuid: action_uuid.into(),
            queue_version_ref,
            payload,
        }
    }

    pub const fn message_type(&self) -> &'static str {
        self.report_type.as_message_type()
    }
}
