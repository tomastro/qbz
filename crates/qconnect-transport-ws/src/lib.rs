//! qconnect-transport-ws
//!
//! WS transport trait + in-memory transport used during POC development.

mod config;
mod error;
mod transport;

pub use config::WsTransportConfig;
pub use error::WsTransportError;
pub use transport::{InMemoryWsTransport, NativeWsTransport, TransportEvent, WsTransport};
