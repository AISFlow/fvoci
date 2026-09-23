//! Resource bounds for untrusted Yjs updateV1 / state-vector bytes.
//!
//! Grounded in original SHA `393795261322b916e588043cf94feca999175843`:
//! `STATE_OVERSIZE_FACTOR = 8` × default `DOCUMENT_MAX_BODY_BYTES = 1048576`
//! → 8 MiB per persistable completeV1 / candidate blob. Load may carry a
//! committed snapshot plus a tail (aggregate 32 MiB). Product HTTP
//! `DOCUMENT_MAX_BODY_BYTES` (1 MiB JSON) remains a later parent concern.
//! This crate never talks to a database or network.

/// Per-blob CRDT cap (candidate update, committed snapshot, sync SV, output).
/// `8 * 1048576` from collab-http `STATE_OVERSIZE_FACTOR`.
pub const MAX_INPUT_BYTES: u64 = 8 * 1024 * 1024;

/// Maximum encoded completeV1 / sync update. Equal to [`MAX_INPUT_BYTES`] so
/// an accepted snapshot can be `load`ed into a fresh child.
pub const MAX_OUTPUT_BYTES: u64 = MAX_INPUT_BYTES;

/// Aggregate decoded `load` snapshot + tail. Each blob still ≤ [`MAX_INPUT_BYTES`].
pub const MAX_LOAD_BYTES: u64 = 32 * 1024 * 1024;

/// JSON frame cap: base64 of a max-load payload plus envelope (~4/3 + keys).
pub const MAX_FRAME_BYTES: u64 = 48 * 1024 * 1024;

/// Maximum tail updates applied after a committed snapshot in one `load`.
pub const MAX_TAIL_UPDATES: usize = 64;

/// Maximum framed operations a single child will service before refusing.
pub const MAX_OPS: u32 = 256;

/// Default per-request wall-clock timeout (covers serialize+write+read).
pub const DEFAULT_TIMEOUT_MS: u64 = 8_000;

/// Child address-space ceiling via Linux `RLIMIT_AS` (virtual size, not RSS).
pub const MAX_CHILD_AS_BYTES: u64 = 256 * 1024 * 1024;

/// Parent-observed `/proc/pid/status` VmRSS kill ceiling (measured RSS).
pub const MAX_OBSERVED_RSS_BYTES: u64 = 256 * 1024 * 1024;

/// Child `RLIMIT_STACK` ceiling.
pub const MAX_CHILD_STACK_BYTES: u64 = 8 * 1024 * 1024;

/// Bounded stderr retained from the child for crash classification.
pub const MAX_CHILD_STDERR_BYTES: u64 = 64 * 1024;

/// Modest global live-child cap. Distinct document rooms may run together.
/// Per-document uniqueness is the future parent room map, not this crate.
/// Admission is immediate [`crate::outcome::EngineStatus::ResourceLimit`], not a wait.
pub const MAX_CHILD_CONCURRENCY: usize = 8;

/// RSS poll interval while waiting on a child request.
pub const RSS_POLL_MS: u64 = 50;

pub const MIN_TIMEOUT_MS: u64 = 1;
pub const MAX_TIMEOUT_MS: u64 = 60_000;
pub const MIN_CHILD_AS_BYTES: u64 = 16 * 1024 * 1024;
pub const MIN_CHILD_STACK_BYTES: u64 = 128 * 1024;

/// Minimum JSON frame size that can hold `decoded_bytes` as standard base64.
pub fn min_frame_bytes_for_input(decoded_bytes: u64) -> u64 {
    (decoded_bytes.saturating_mul(4).saturating_add(2) / 3).saturating_add(4096)
}

#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// Per-blob update / snapshot / state-vector cap.
    pub max_input_bytes: u64,
    /// Reloadable completeV1 / sync output. Must equal [`Self::max_input_bytes`].
    pub max_output_bytes: u64,
    /// Aggregate decoded load (snapshot + tail). ≥ [`Self::max_input_bytes`].
    pub max_load_bytes: u64,
    pub max_frame_bytes: u64,
    pub max_tail_updates: usize,
    pub max_ops: u32,
    pub timeout_ms: u64,
    /// `RLIMIT_AS` virtual-size ceiling.
    pub max_child_as_bytes: u64,
    /// Parent VmRSS poll kill ceiling (not `RLIMIT_AS`).
    pub max_observed_rss_bytes: u64,
    pub max_child_stack_bytes: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_input_bytes: MAX_INPUT_BYTES,
            max_output_bytes: MAX_OUTPUT_BYTES,
            max_load_bytes: MAX_LOAD_BYTES,
            max_frame_bytes: MAX_FRAME_BYTES,
            max_tail_updates: MAX_TAIL_UPDATES,
            max_ops: MAX_OPS,
            timeout_ms: DEFAULT_TIMEOUT_MS,
            max_child_as_bytes: MAX_CHILD_AS_BYTES,
            max_observed_rss_bytes: MAX_OBSERVED_RSS_BYTES,
            max_child_stack_bytes: MAX_CHILD_STACK_BYTES,
        }
    }
}

impl Limits {
    pub fn for_tests() -> Self {
        Self {
            timeout_ms: 4_000,
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
        if self.max_output_bytes != self.max_input_bytes {
            return Err(format!(
                "max_output_bytes {} must equal max_input_bytes {} so snapshots reload",
                self.max_output_bytes, self.max_input_bytes
            ));
        }
        if self.max_load_bytes < self.max_input_bytes || self.max_load_bytes > MAX_LOAD_BYTES {
            return Err(format!(
                "max_load_bytes {} outside {}..={}",
                self.max_load_bytes, self.max_input_bytes, MAX_LOAD_BYTES
            ));
        }
        let min_frame = min_frame_bytes_for_input(self.max_load_bytes);
        if self.max_frame_bytes < min_frame || self.max_frame_bytes > MAX_FRAME_BYTES {
            return Err(format!(
                "max_frame_bytes {} outside {}..={}",
                self.max_frame_bytes, min_frame, MAX_FRAME_BYTES
            ));
        }
        if self.max_tail_updates == 0 || self.max_tail_updates > MAX_TAIL_UPDATES {
            return Err(format!(
                "max_tail_updates {} outside 1..={}",
                self.max_tail_updates, MAX_TAIL_UPDATES
            ));
        }
        if self.max_ops == 0 || self.max_ops > MAX_OPS {
            return Err(format!("max_ops {} outside 1..={}", self.max_ops, MAX_OPS));
        }
        if self.max_child_as_bytes < MIN_CHILD_AS_BYTES {
            return Err(format!(
                "max_child_as_bytes {} below {}",
                self.max_child_as_bytes, MIN_CHILD_AS_BYTES
            ));
        }
        if self.max_observed_rss_bytes < MIN_CHILD_AS_BYTES {
            return Err(format!(
                "max_observed_rss_bytes {} below {}",
                self.max_observed_rss_bytes, MIN_CHILD_AS_BYTES
            ));
        }
        if self.max_child_stack_bytes < MIN_CHILD_STACK_BYTES
            || self.max_child_stack_bytes > MAX_CHILD_STACK_BYTES
        {
            return Err(format!(
                "max_child_stack_bytes {} outside {}..={}",
                self.max_child_stack_bytes, MIN_CHILD_STACK_BYTES, MAX_CHILD_STACK_BYTES
            ));
        }
        Ok(())
    }
}
