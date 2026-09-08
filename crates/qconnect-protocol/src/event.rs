use qconnect_core::QueueVersion;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum QueueEventType {
    SrvrCtrlQueueState,
    SrvrCtrlQueueTracksAdded,
    SrvrCtrlQueueTracksLoaded,
    SrvrCtrlQueueTracksInserted,
    SrvrCtrlQueueTracksRemoved,
    SrvrCtrlQueueTracksReordered,
    SrvrCtrlQueueCleared,
    SrvrCtrlShuffleModeSet,
    SrvrCtrlAutoplayModeSet,
    SrvrCtrlAutoplayTracksLoaded,
    SrvrCtrlAutoplayTracksRemoved,
    SrvrCtrlQueueTracksAddedFromAutoplay,
    SrvrCtrlQueueErrorMessage,
    // Session management events (types 81-87, 97-101)
    SrvrCtrlSessionState,
    SrvrCtrlRendererStateUpdated,
    SrvrCtrlAddRenderer,
    SrvrCtrlUpdateRenderer,
    SrvrCtrlRemoveRenderer,
    SrvrCtrlActiveRendererChanged,
    SrvrCtrlVolumeChanged,
    SrvrCtrlLoopModeSet,
    SrvrCtrlVolumeMuted,
    SrvrCtrlMaxAudioQualityChanged,
    SrvrCtrlFileAudioQualityChanged,
    SrvrCtrlDeviceAudioQualityChanged,
}

impl QueueEventType {
    pub const fn as_message_type(self) -> &'static str {
        match self {
            Self::SrvrCtrlQueueState => "MESSAGE_TYPE_SRVR_CTRL_QUEUE_STATE",
            Self::SrvrCtrlQueueTracksAdded => "MESSAGE_TYPE_SRVR_CTRL_QUEUE_TRACKS_ADDED",
            Self::SrvrCtrlQueueTracksLoaded => "MESSAGE_TYPE_SRVR_CTRL_QUEUE_TRACKS_LOADED",
            Self::SrvrCtrlQueueTracksInserted => "MESSAGE_TYPE_SRVR_CTRL_QUEUE_TRACKS_INSERTED",
            Self::SrvrCtrlQueueTracksRemoved => "MESSAGE_TYPE_SRVR_CTRL_QUEUE_TRACKS_REMOVED",
            Self::SrvrCtrlQueueTracksReordered => "MESSAGE_TYPE_SRVR_CTRL_QUEUE_TRACKS_REORDERED",
            Self::SrvrCtrlQueueCleared => "MESSAGE_TYPE_SRVR_CTRL_QUEUE_CLEARED",
            Self::SrvrCtrlShuffleModeSet => "MESSAGE_TYPE_SRVR_CTRL_SHUFFLE_MODE_SET",
            Self::SrvrCtrlAutoplayModeSet => "MESSAGE_TYPE_SRVR_CTRL_AUTOPLAY_MODE_SET",
            Self::SrvrCtrlAutoplayTracksLoaded => "MESSAGE_TYPE_SRVR_CTRL_AUTOPLAY_TRACKS_LOADED",
            Self::SrvrCtrlAutoplayTracksRemoved => "MESSAGE_TYPE_SRVR_CTRL_AUTOPLAY_TRACKS_REMOVED",
            Self::SrvrCtrlQueueTracksAddedFromAutoplay => {
                "MESSAGE_TYPE_SRVR_CTRL_QUEUE_TRACKS_ADDED_FROM_AUTOPLAY"
            }
            Self::SrvrCtrlQueueErrorMessage => "MESSAGE_TYPE_SRVR_CTRL_QUEUE_ERROR_MESSAGE",
            Self::SrvrCtrlSessionState => "MESSAGE_TYPE_SRVR_CTRL_SESSION_STATE",
            Self::SrvrCtrlRendererStateUpdated => "MESSAGE_TYPE_SRVR_CTRL_RENDERER_STATE_UPDATED",
            Self::SrvrCtrlAddRenderer => "MESSAGE_TYPE_SRVR_CTRL_ADD_RENDERER",
            Self::SrvrCtrlUpdateRenderer => "MESSAGE_TYPE_SRVR_CTRL_UPDATE_RENDERER",
            Self::SrvrCtrlRemoveRenderer => "MESSAGE_TYPE_SRVR_CTRL_REMOVE_RENDERER",
            Self::SrvrCtrlActiveRendererChanged => "MESSAGE_TYPE_SRVR_CTRL_ACTIVE_RENDERER_CHANGED",
            Self::SrvrCtrlVolumeChanged => "MESSAGE_TYPE_SRVR_CTRL_VOLUME_CHANGED",
            Self::SrvrCtrlLoopModeSet => "MESSAGE_TYPE_SRVR_CTRL_LOOP_MODE_SET",
            Self::SrvrCtrlVolumeMuted => "MESSAGE_TYPE_SRVR_CTRL_VOLUME_MUTED",
            Self::SrvrCtrlMaxAudioQualityChanged => {
                "MESSAGE_TYPE_SRVR_CTRL_MAX_AUDIO_QUALITY_CHANGED"
            }
            Self::SrvrCtrlFileAudioQualityChanged => {
                "MESSAGE_TYPE_SRVR_CTRL_FILE_AUDIO_QUALITY_CHANGED"
            }
            Self::SrvrCtrlDeviceAudioQualityChanged => {
                "MESSAGE_TYPE_SRVR_CTRL_DEVICE_AUDIO_QUALITY_CHANGED"
            }
        }
    }
}

impl QueueEventType {
    /// Returns true for session management events that should NOT go through
    /// the queue reducer.
    pub const fn is_session_management(self) -> bool {
        matches!(
            self,
            Self::SrvrCtrlSessionState
                | Self::SrvrCtrlRendererStateUpdated
                | Self::SrvrCtrlAddRenderer
                | Self::SrvrCtrlUpdateRenderer
                | Self::SrvrCtrlRemoveRenderer
                | Self::SrvrCtrlActiveRendererChanged
                | Self::SrvrCtrlVolumeChanged
                | Self::SrvrCtrlLoopModeSet
                | Self::SrvrCtrlVolumeMuted
                | Self::SrvrCtrlMaxAudioQualityChanged
                | Self::SrvrCtrlFileAudioQualityChanged
                | Self::SrvrCtrlDeviceAudioQualityChanged
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueueServerEvent {
    pub event_type: QueueEventType,
    pub action_uuid: Option<String>,
    pub queue_version: Option<QueueVersion>,
    #[serde(default)]
    pub payload: Value,
}

impl QueueServerEvent {
    pub const fn message_type(&self) -> &'static str {
        self.event_type.as_message_type()
    }
}

/// Common.ErrorType classification for a renderer per-track playback failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ErrorType {
    Unknown,
    TrackNotFound,
    TrackNotStreamable,
    TrackMusicDataInvalid,
    ServiceError,
    NetworkError,
    OtherErrors,
}

impl ErrorType {
    pub fn from_wire(value: i32) -> Self {
        match value {
            1 => Self::TrackNotFound,
            2 => Self::TrackNotStreamable,
            3 => Self::TrackMusicDataInvalid,
            4 => Self::ServiceError,
            5 => Self::NetworkError,
            100 => Self::OtherErrors,
            _ => Self::Unknown,
        }
    }
}

/// Renderer per-track failure surfaced from `PlaybackErrorMessage`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlaybackErrorEvent {
    pub queue_item_id: u64,
    pub error_type: ErrorType,
    pub queue_version: Option<QueueVersion>,
}
