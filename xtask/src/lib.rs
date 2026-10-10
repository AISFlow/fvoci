//! FVOCI development tasks. The binary in `main.rs` dispatches to these modules.

pub mod args;
pub mod ci_fixture;
pub mod host;
pub mod process;
pub mod rust_binaries;
pub mod rust_binaries_archive;
pub mod rust_binaries_cohort;
pub mod schema_baseline;
pub mod selected_install;
pub mod shell;
pub mod sqlite;
pub mod sqlite_build;
pub mod sqlite_ci;
pub mod sqlite_zip;
