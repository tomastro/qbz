use thiserror::Error;

#[derive(Debug, Error)]
pub enum WsTransportError {
    #[error("transport is already connected")]
    AlreadyConnected,
    #[error("transport is already running")]
    AlreadyRunning,
    #[error("transport is not connected")]
    NotConnected,
    #[error("transport event channel is closed")]
    EventChannelClosed,
    #[error("transport task channel is closed")]
    TransportChannelClosed,
    #[error("transport task join error: {0}")]
    Join(String),
    #[error("websocket protocol error: {0}")]
    Protocol(String),
    #[error("serialization error: {0}")]
    Serialization(String),
    #[error("internal transport error: {0}")]
    Internal(String),
    #[error("missing AUTHENTICATE JWT for an endpoint that requires it")]
    MissingAuthenticateJwt,
}
