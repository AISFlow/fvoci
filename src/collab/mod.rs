//! Authenticated Hocuspocus 4.6.0 `/collab` product transport.
//!
//! Enabled only when `FVOCI_COLLAB_ENGINE` points at a built `collab-engine`
//! helper binary. Without it the route returns 503 `collab_unavailable`.

pub mod awareness;
pub mod config;
pub mod engine_bridge;
pub mod hub;
pub mod origin;
pub mod room;
pub mod transport;
pub mod wire;
pub mod y_sync;

pub use config::CollabConfig;
pub use hub::CollabHub;
pub use wire::{
    decode, encode, AuthMessage, AuthMessageType, CollabKind, CollabRoomName, ConnectionMessage,
    DocumentMessage, Limits, MessageType, SyncMessage, SyncStep, WireError, WireFrame,
};
