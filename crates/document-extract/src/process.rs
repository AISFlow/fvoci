//! Killable child client is defined in `document-extract-client`.
//! `extract_in_process` stays on this parser crate.

pub use document_extract_client::process::{
    apply_rlimits_now, extract_killable, extract_killable_with_cancel, Cancelled, ExtractRequest,
};
#[cfg(feature = "test-hang")]
pub use document_extract_client::process::{
    is_slot_waiter, peek_last_spawn, run_parent_death_driver, take_last_spawn, SpawnTrace,
};

use crate::limits::Limits;
use crate::outcome::ExtractReport;
use crate::parse::extract_bytes;

pub fn extract_in_process(bytes: &[u8], name: &str, limits: &Limits) -> ExtractReport {
    extract_bytes(bytes, name, limits)
}

#[cfg(test)]
mod metadata_pin {
    #[test]
    fn parser_metadata_rhwp_rev_equals_canonical_constant() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        let toml = std::fs::read_to_string(&path).expect("parser Cargo.toml");
        let rev =
            metadata_rhwp_rev(&toml).expect("rhwp_rev in [package.metadata.document-extract]");
        assert_eq!(rev, crate::RHWP_REV);
    }

    fn metadata_rhwp_rev(toml: &str) -> Option<&str> {
        let mut in_section = false;
        for line in toml.lines() {
            let line = line.trim();
            if line.starts_with('[') && line.ends_with(']') {
                in_section = line == "[package.metadata.document-extract]";
                continue;
            }
            if !in_section {
                continue;
            }
            let Some(rest) = line.strip_prefix("rhwp_rev") else {
                continue;
            };
            let rest = rest.trim().strip_prefix('=')?.trim();
            return Some(rest.trim_matches('"'));
        }
        None
    }
}
