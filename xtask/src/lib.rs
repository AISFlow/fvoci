//! FVOCI development tasks. The binary in `main.rs` dispatches to these modules.

pub mod args;
pub mod host;
pub mod process;
pub mod rustup_ci_metadata;
pub mod shell;
pub mod sqlite;
pub mod sqlite_build;
pub mod sqlite_ci;
pub mod sqlite_zip;
