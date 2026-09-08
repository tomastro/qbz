use qconnect_core::QueueVersion;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{ProtocolError, QueueCommand, QueueEventType, QueueServerEvent};

pub const QCONNECT_SERVICE: &str = "QConnect";
pub const QCONNECT_BACKEND_DESTINATION: &str = "Backend";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutboundEnvelope {
    pub service: String,
    pub destination: String,
    pub message_type: String,
    pub action_uuid: String,
    pub queue_version_ref: QueueVersion,
    #[serde(default)]
    pub payload: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload_bytes: Option<Vec<u8>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InboundEnvelope {
    pub service: String,
    pub source: String,
    pub message_type: String,
    pub action_uuid: Option<String>,
    pub queue_version: Option<QueueVersion>,
    #[serde(default)]
    pub payload: Value,
}

pub fn build_outbound_envelope(command: QueueCommand) -> OutboundEnvelope {
    OutboundEnvelope {
        service: QCONNECT_SERVICE.to_string(),
        destination: QCONNECT_BACKEND_DESTINATION.to_string(),
        message_type: command.message_type().to_string(),
        action_uuid: command.action_uuid,
        queue_version_ref: command.queue_version_ref,
        payload: command.payload,
        payload_bytes: None,
    }
}

pub fn parse_inbound_event(envelope: InboundEnvelope) -> Result<QueueServerEvent, ProtocolError> {
    Ok(QueueServerEvent {
        event_type: parse_event_type(&envelope.message_type)?,
        action_uuid: envelope.action_uuid,
        queue_version: envelope.queue_version,
        payload: envelope.payload,
    })
}

pub fn encode_outbound_json(envelope: &OutboundEnvelope) -> Result<Vec<u8>, ProtocolError> {
    Ok(serde_json::to_vec(envelope)?)
}

pub fn encode_outbound_payload_bytes(
    envelope: &OutboundEnvelope,
) -> Result<Vec<u8>, ProtocolError> {
    if let Some(payload_bytes) = &envelope.payload_bytes {
        return Ok(payload_bytes.clone());
    }
    Ok(serde_json::to_vec(envelope)?)
}

pub fn decode_inbound_json(bytes: &[u8]) -> Result<InboundEnvelope, ProtocolError> {
    Ok(serde_json::from_slice::<InboundEnvelope>(bytes)?)
}

fn parse_event_type(message_type: &str) -> Result<QueueEventType, ProtocolError> {
    match message_type {
        "MESSAGE_TYPE_SRVR_CTRL_QUEUE_STATE" => Ok(QueueEventType::SrvrCtrlQueueState),
        "MESSAGE_TYPE_SRVR_CTRL_QUEUE_TRACKS_ADDED" => Ok(QueueEventType::SrvrCtrlQueueTracksAdded),
        "MESSAGE_TYPE_SRVR_CTRL_QUEUE_TRACKS_LOADED" => {
            Ok(QueueEventType::SrvrCtrlQueueTracksLoaded)
        }
        "MESSAGE_TYPE_SRVR_CTRL_QUEUE_TRACKS_INSERTED" => {
            Ok(QueueEventType::SrvrCtrlQueueTracksInserted)
        }
        "MESSAGE_TYPE_SRVR_CTRL_QUEUE_TRACKS_REMOVED" => {
            Ok(QueueEventType::SrvrCtrlQueueTracksRemoved)
        }
        "MESSAGE_TYPE_SRVR_CTRL_QUEUE_TRACKS_REORDERED" => {
            Ok(QueueEventType::SrvrCtrlQueueTracksReordered)
        }
        "MESSAGE_TYPE_SRVR_CTRL_QUEUE_CLEARED" => Ok(QueueEventType::SrvrCtrlQueueCleared),
        "MESSAGE_TYPE_SRVR_CTRL_SHUFFLE_MODE_SET" => Ok(QueueEventType::SrvrCtrlShuffleModeSet),
        "MESSAGE_TYPE_SRVR_CTRL_AUTOPLAY_MODE_SET" => Ok(QueueEventType::SrvrCtrlAutoplayModeSet),
        "MESSAGE_TYPE_SRVR_CTRL_AUTOPLAY_TRACKS_LOADED" => {
            Ok(QueueEventType::SrvrCtrlAutoplayTracksLoaded)
        }
        "MESSAGE_TYPE_SRVR_CTRL_AUTOPLAY_TRACKS_REMOVED" => {
            Ok(QueueEventType::SrvrCtrlAutoplayTracksRemoved)
        }
        "MESSAGE_TYPE_SRVR_CTRL_QUEUE_TRACKS_ADDED_FROM_AUTOPLAY" => {
            Ok(QueueEventType::SrvrCtrlQueueTracksAddedFromAutoplay)
        }
        "MESSAGE_TYPE_SRVR_CTRL_QUEUE_ERROR_MESSAGE" => {
            Ok(QueueEventType::SrvrCtrlQueueErrorMessage)
        }
        other => Err(ProtocolError::UnknownMessageType(other.to_string())),
    }
}
