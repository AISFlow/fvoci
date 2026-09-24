//! Resource bounds for untrusted HWP/HWPX input.
//!
//! Numbers are stated here so callers and tests do not guess. Product attachment
//! HTTP is out of scope; these bounds apply to the native extract boundary.

/// Maximum accepted input bytes (FVOCI `EXTRACT_MAX_BYTES`).
pub const MAX_INPUT_BYTES: u64 = 20 * 1024 * 1024;

/// Maximum extracted Unicode scalar values (FVOCI `EXTRACT_PLAIN_MAX_CHARS`).
pub const MAX_OUTPUT_CHARS: usize = 500_000;

/// Default child-process watchdog (FVOCI `EXTRACT_TEXT_WATCHDOG_MS`).
pub const DEFAULT_TIMEOUT_MS: u64 = 120_000;

/// Child RSS ceiling (FVOCI `ISOLATE_CHILD_MAX_RSS_BYTES`).
pub const MAX_CHILD_RSS_BYTES: u64 = 1536 * 1024 * 1024;

/// Zip central-directory entry cap before the parser (FVOCI `unzipStore`).
pub const MAX_ZIP_ENTRIES: usize = 10_000;

/// Sum of zip uncompressed sizes before the parser (FVOCI `unzipStore`).
pub const MAX_ZIP_UNCOMPRESSED_BYTES: u64 = 200 * 1024 * 1024;

/// Maximum stdout JSON bytes accepted from the child.
/// 500_000 output scalars may JSON-escape as `\uXXXX` (6 bytes) plus wrapper.
pub const MAX_CHILD_STDOUT_BYTES: u64 = (MAX_OUTPUT_CHARS as u64) * 6 + 4096;

/// Table/control walk nesting cap (rhwp `table_extract::MAX_NEST_DEPTH`).
pub const MAX_WALK_NEST_DEPTH: usize = 8;

/// Distinct walk warning kinds retained on the report (counts collapse per kind).
pub const MAX_WARNING_ENTRIES: usize = 32;

/// Maximum concurrent extract children. Synchronous parse cannot be cancelled
/// in-thread, so the parent admits only one child.
pub const MAX_CHILD_CONCURRENCY: usize = 1;

/// RSS poll interval while a child is running.
pub const RSS_POLL_MS: u64 = 250;

/// Smallest accepted wall-clock timeout (zero is invalid).
pub const MIN_TIMEOUT_MS: u64 = 1;

/// Largest accepted wall-clock timeout (24h). Larger values fail closed.
pub const MAX_TIMEOUT_MS: u64 = 24 * 60 * 60 * 1000;

/// Smallest accepted child address-space ceiling.
pub const MIN_CHILD_RSS_BYTES: u64 = 1024 * 1024;

/// Upstream rhwp HWP5 single-stream decompressed cap (documented, not ours).
pub const RHWP_HWP5_STREAM_OUTPUT_BYTES: u64 = 256 * 1024 * 1024;

/// Upstream rhwp HWP5 cumulative decompressed cap (documented, not ours).
pub const RHWP_HWP5_TOTAL_OUTPUT_BYTES: u64 = 512 * 1024 * 1024;

/// Upstream rhwp HWPX XML entry cap (documented, not ours).
pub const RHWP_HWPX_XML_ENTRY_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub max_input_bytes: u64,
    pub max_output_chars: usize,
    pub timeout_ms: u64,
    pub max_child_rss_bytes: u64,
    pub max_zip_entries: usize,
    pub max_zip_uncompressed_bytes: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_input_bytes: MAX_INPUT_BYTES,
            max_output_chars: MAX_OUTPUT_CHARS,
            timeout_ms: DEFAULT_TIMEOUT_MS,
            max_child_rss_bytes: MAX_CHILD_RSS_BYTES,
            max_zip_entries: MAX_ZIP_ENTRIES,
            max_zip_uncompressed_bytes: MAX_ZIP_UNCOMPRESSED_BYTES,
        }
    }
}

impl Limits {
    pub fn for_tests() -> Self {
        Self {
            timeout_ms: 8_000,
            ..Self::default()
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.timeout_ms < MIN_TIMEOUT_MS || self.timeout_ms > MAX_TIMEOUT_MS {
            return Err(format!(
                "timeout_ms {} outside {}..={}",
                self.timeout_ms, MIN_TIMEOUT_MS, MAX_TIMEOUT_MS
            ));
        }
        if self.max_input_bytes == 0 || self.max_input_bytes > MAX_INPUT_BYTES {
            return Err(format!(
                "max_input_bytes {} outside 1..={}",
                self.max_input_bytes, MAX_INPUT_BYTES
            ));
        }
        if self.max_output_chars == 0 || self.max_output_chars > MAX_OUTPUT_CHARS {
            return Err(format!(
                "max_output_chars {} outside 1..={}",
                self.max_output_chars, MAX_OUTPUT_CHARS
            ));
        }
        if self.max_child_rss_bytes < MIN_CHILD_RSS_BYTES {
            return Err(format!(
                "max_child_rss_bytes {} below {}",
                self.max_child_rss_bytes, MIN_CHILD_RSS_BYTES
            ));
        }
        if self.max_zip_entries == 0 || self.max_zip_uncompressed_bytes == 0 {
            return Err("zip limits must be positive".to_string());
        }
        Ok(())
    }
}
