//! Hocuspocus 4.6.0 wire codec only — not an active `/collab` product route.
//!
//! Typed, bounded encode/decode for the installed provider/server framing layer.
//! Y-protocol payloads are preserved opaquely; this module does not authenticate
//! sessions or interpret CRDT client IDs beyond routing-key parsing.

pub mod wire;

pub use wire::{
    decode, encode, AuthMessage, AuthMessageType, CollabKind, CollabRoomName, ConnectionMessage,
    DocumentMessage, Limits, MessageType, SyncMessage, SyncStep, WireError, WireFrame,
};
