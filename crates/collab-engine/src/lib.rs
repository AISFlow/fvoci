//! Isolated native Yrs child for FVOCI collaboration.
//!
//! This crate is its own compile graph. `fvoci-server` links its parent side,
//! and its room actor (`src/collab/room.rs`) keeps a FIFO per document: an
//! update is committed to the database before it is broadcast, and a rejected
//! or uncertain candidate is recovered by reloading committed state — never by
//! Yrs undo as a DB rollback. The crate opens no WebSockets and touches no
//! database.
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

pub mod archive_history;
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
pub mod seed;

#[cfg(feature = "worker")]
pub use engine::{
    new_doc, revision_snapshots_semantically_equal, CollabEngine, FRAGMENT as ENGINE_FRAGMENT,
};
pub use limits::Limits;
pub use outcome::{EngineReport, EngineStatus, LimitKind, UnsupportedReason, WorkerFailureReason};
pub use process::{
    apply_rlimits_now, max_child_concurrency, max_seed_child_concurrency,
    max_validator_child_concurrency, raise_nofile_to_hard_limit, set_max_child_concurrency,
    set_max_seed_child_concurrency, set_max_validator_child_concurrency,
    sum_live_children_rss_bytes, ChildSlotKind, EngineSession, SpawnPhaseTimings, SpawnRequest,
};
#[cfg(feature = "test-hang")]
pub use process::{take_last_spawn, SpawnTrace};
pub use protocol::Request;
