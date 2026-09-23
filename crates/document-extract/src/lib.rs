//! Native HWP 5.0 / HWPX body extraction.
//!
//! Parser: `edwardkim/rhwp` git rev [`RHWP_REV`], default-features off
//! (`native-skia` / `gpu` not enabled). This crate does not run Node, Bun,
//! WASM, or a browser. Attachment HTTP is out of scope.

pub const RHWP_REV: &str = "e8800c8def63449808a4092798442652ed460552";
pub const RHWP_REPO: &str = "https://github.com/edwardkim/rhwp";
pub const RHWP_LICENSE: &str = "MIT";

pub mod classify;
pub mod gen;
pub mod limits;
pub mod outcome;
pub mod parse;
pub mod process;
pub mod walk;

pub use limits::Limits;
pub use outcome::{
    DocFormat, ExtractReport, ExtractStatus, LimitKind, UnsupportedReason, WorkerFailureReason,
};
pub use parse::extract_bytes;
pub use process::{extract_in_process, extract_killable, ExtractRequest};
#[cfg(feature = "test-hang")]
pub use process::{take_last_spawn, SpawnTrace};
