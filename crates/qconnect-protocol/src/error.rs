use thiserror::Error;

#[derive(Debug, Error)]
pub enum ProtocolError {
    #[error("unknown queue event message type: {0}")]
    UnknownMessageType(String),
    #[error("unsupported queue command type for protobuf mapping: {0}")]
    UnsupportedQueueCommand(String),
    #[error("invalid queue command payload: {0}")]
    InvalidPayload(String),
    #[error("invalid uuid value: {0}")]
    InvalidUuid(String),
    #[error("protobuf decode error: {0}")]
    Decode(#[from] prost::DecodeError),
    #[error("serialization error: {0}")]
    Serialize(#[from] serde_json::Error),
}
