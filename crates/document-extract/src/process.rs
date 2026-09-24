//! Killable child client is defined in `document-extract-client`.
//! `extract_in_process` stays on this parser crate.

pub use document_extract_client::process::{apply_rlimits_now, extract_killable, ExtractRequest};
#[cfg(feature = "test-hang")]
pub use document_extract_client::process::{take_last_spawn, SpawnTrace};

use crate::limits::Limits;
use crate::outcome::ExtractReport;
use crate::parse::extract_bytes;

pub fn extract_in_process(bytes: &[u8], name: &str, limits: &Limits) -> ExtractReport {
    extract_bytes(bytes, name, limits)
}
