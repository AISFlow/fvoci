//! Isolated native Yrs child for FVOCI collaboration.
//!
//! This crate is its own compile graph. It is **not** wired into `fvoci-server`,
//! does not open WebSockets, and does not touch the database. A future room
//! actor should keep a FIFO per document, apply candidates here, persist
//! completeV1, then broadcast — never Yrs-undo as a DB rollback.
//!
//! Parent code should depend with `default-features = false` and talk to the
//! child through [`process`] and [`protocol`] only. Feature `worker` pulls in
//! Yrs and the helper binary.
//!
//! Linux x86_64 and aarch64 are in scope. Other OS refuse closed.

#![allow(clippy::result_large_err)]

pub const YRS_VERSION: &str = "0.28.0";
pub const YJS_VERSION: &str = "13.6.32";
pub const FRAGMENT: &str = "prosemirror";
pub const ENCODING_V1: u8 = 1;

pub mod b64;
pub mod frame;
pub mod limits;
pub mod outcome;
pub mod process;
pub mod protocol;

#[cfg(feature = "worker")]
pub mod engine;
#[cfg(feature = "worker")]
mod project;

#[cfg(feature = "worker")]
pub use engine::{new_doc, CollabEngine, FRAGMENT as ENGINE_FRAGMENT};
pub use limits::Limits;
pub use outcome::{EngineReport, EngineStatus, LimitKind, UnsupportedReason, WorkerFailureReason};
pub use process::{
    apply_rlimits_now, max_child_concurrency, max_validator_child_concurrency,
    raise_nofile_to_hard_limit, set_max_child_concurrency, set_max_validator_child_concurrency,
    sum_live_children_rss_bytes, ChildSlotKind, EngineSession, SpawnPhaseTimings, SpawnRequest,
};
#[cfg(feature = "test-hang")]
pub use process::{take_last_spawn, SpawnTrace};
pub use protocol::Request;
