//! Official Qobuz Connect LAN receiver surface.
//!
//! This crate owns only local discovery, HTTP wire validation and bounded
//! admission. It deliberately has no Qobuz client, player, Qt or daemon
//! dependency; credential validation and activation belong to the coordinator.

mod admission;
mod mdns;
mod model;
mod projection;
mod server;
mod validation;

pub use admission::{admission_channel, AdmissionInbox, AdmissionSender, SubmitError};
pub use model::{
    ConnectInfo, DeviceType, DisplayInfo, HandoffCandidate, LanJwtToken, MaxAudioQuality,
};
pub use projection::LanProjection;
pub use server::{
    LanError, LanHttpMethod, LanHttpRoute, LanRequestObservation, LanService, LanServiceConfig,
    SERVICE_TYPE,
};
pub use validation::{EndpointPolicy, ValidationError, MAX_BODY_BYTES};
