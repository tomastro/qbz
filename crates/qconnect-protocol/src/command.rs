use qconnect_core::QueueVersion;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum QueueCommandType {
    CtrlSrvrJoinSession,
    CtrlSrvrSetPlayerState,
    CtrlSrvrSetActiveRenderer,
    CtrlSrvrSetVolume,
    CtrlSrvrSetLoopMode,
    CtrlSrvrMuteVolume,
    CtrlSrvrSetMaxAudioQuality,
    CtrlSrvrAskForRendererState,
    CtrlSrvrQueueAddTracks,
    CtrlSrvrQueueLoadTracks,
    CtrlSrvrQueueInsertTracks,
    CtrlSrvrQueueRemoveTracks,
    CtrlSrvrQueueReorderTracks,
    CtrlSrvrClearQueue,
    CtrlSrvrSetShuffleMode,
    CtrlSrvrSetAutoplayMode,
    CtrlSrvrAutoplayLoadTracks,
    CtrlSrvrAutoplayRemoveTracks,
    CtrlSrvrSetQueueState,
    CtrlSrvrAskForQueueState,
}

impl QueueCommandType {
    pub const fn as_message_type(self) -> &'static str {
        match self {
            Self::CtrlSrvrJoinSession => "MESSAGE_TYPE_CTRL_SRVR_JOIN_SESSION",
            Self::CtrlSrvrSetPlayerState => "MESSAGE_TYPE_CTRL_SRVR_SET_PLAYER_STATE",
            Self::CtrlSrvrSetActiveRenderer => "MESSAGE_TYPE_CTRL_SRVR_SET_ACTIVE_RENDERER",
            Self::CtrlSrvrSetVolume => "MESSAGE_TYPE_CTRL_SRVR_SET_VOLUME",
            Self::CtrlSrvrSetLoopMode => "MESSAGE_TYPE_CTRL_SRVR_SET_LOOP_MODE",
            Self::CtrlSrvrMuteVolume => "MESSAGE_TYPE_CTRL_SRVR_MUTE_VOLUME",
            Self::CtrlSrvrSetMaxAudioQuality => "MESSAGE_TYPE_CTRL_SRVR_SET_MAX_AUDIO_QUALITY",
            Self::CtrlSrvrAskForRendererState => "MESSAGE_TYPE_CTRL_SRVR_ASK_FOR_RENDERER_STATE",
            Self::CtrlSrvrQueueAddTracks => "MESSAGE_TYPE_CTRL_SRVR_QUEUE_ADD_TRACKS",
            Self::CtrlSrvrQueueLoadTracks => "MESSAGE_TYPE_CTRL_SRVR_QUEUE_LOAD_TRACKS",
            Self::CtrlSrvrQueueInsertTracks => "MESSAGE_TYPE_CTRL_SRVR_QUEUE_INSERT_TRACKS",
            Self::CtrlSrvrQueueRemoveTracks => "MESSAGE_TYPE_CTRL_SRVR_QUEUE_REMOVE_TRACKS",
            Self::CtrlSrvrQueueReorderTracks => "MESSAGE_TYPE_CTRL_SRVR_QUEUE_REORDER_TRACKS",
            Self::CtrlSrvrClearQueue => "MESSAGE_TYPE_CTRL_SRVR_CLEAR_QUEUE",
            Self::CtrlSrvrSetShuffleMode => "MESSAGE_TYPE_CTRL_SRVR_SET_SHUFFLE_MODE",
            Self::CtrlSrvrSetAutoplayMode => "MESSAGE_TYPE_CTRL_SRVR_SET_AUTOPLAY_MODE",
            Self::CtrlSrvrAutoplayLoadTracks => "MESSAGE_TYPE_CTRL_SRVR_AUTOPLAY_LOAD_TRACKS",
            Self::CtrlSrvrAutoplayRemoveTracks => "MESSAGE_TYPE_CTRL_SRVR_AUTOPLAY_REMOVE_TRACKS",
            Self::CtrlSrvrSetQueueState => "MESSAGE_TYPE_CTRL_SRVR_SET_QUEUE_STATE",
            Self::CtrlSrvrAskForQueueState => "MESSAGE_TYPE_CTRL_SRVR_ASK_FOR_QUEUE_STATE",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueueCommand {
    pub command_type: QueueCommandType,
    pub action_uuid: String,
    pub queue_version_ref: QueueVersion,
    #[serde(default)]
    pub payload: Value,
}

impl QueueCommand {
    pub fn new(
        command_type: QueueCommandType,
        action_uuid: impl Into<String>,
        queue_version_ref: QueueVersion,
        payload: Value,
    ) -> Self {
        Self {
            command_type,
            action_uuid: action_uuid.into(),
            queue_version_ref,
            payload,
        }
    }

    pub const fn message_type(&self) -> &'static str {
        self.command_type.as_message_type()
    }
}
