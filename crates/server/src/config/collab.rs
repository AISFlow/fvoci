//! The settings of editing a body together: the worker processes and their limits
//! (`crate::collab`, `crate::collab_worker`).

use anyhow::bail;
use serde::Deserialize;

/// The variables of the environment of these keys ([`super::ENV_KEYS`]).
pub(super) const ENV_KEYS: [(&str, super::EnvKind); 15] = [
    ("collab_workers", super::EnvKind::Number),
    ("collab_documents_per_worker", super::EnvKind::Number),
    ("collab_state_bytes", super::EnvKind::Number),
    ("collab_update_bytes", super::EnvKind::Number),
    ("collab_worker_timeout_ms", super::EnvKind::Number),
    ("collab_worker_memory_bytes", super::EnvKind::Number),
    ("collab_recheck_seconds", super::EnvKind::Number),
    ("collab_documents_per_connection", super::EnvKind::Number),
    ("collab_connections_per_document", super::EnvKind::Number),
    ("collab_connections_per_user", super::EnvKind::Number),
    ("collab_frames_per_second", super::EnvKind::Number),
    ("collab_queue_messages", super::EnvKind::Number),
    ("collab_body_versions", super::EnvKind::Number),
    ("body_versions_max_bytes", super::EnvKind::Number),
    ("collab_version_minutes", super::EnvKind::Number),
];

/// The keys of editing a body together and its version limits, at the top level.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[cfg_attr(test, derive(serde::Serialize))]
#[serde(default)]
pub struct CollabSettings {
    /// How many worker processes hold the documents. 1 to 64.
    pub collab_workers: u32,
    /// How many documents one worker holds at once. At least 1. A worker also holds no
    /// more of their states, together, than its memory has room for
    /// ([`CollabSettings::held_bytes`]).
    pub collab_documents_per_worker: u32,
    /// The largest document, in bytes of its stored state. 16 MiB to 1 GiB.
    pub collab_state_bytes: u64,
    /// The largest update a client sends, in bytes. At least 1, at most
    /// `collab_state_bytes`.
    pub collab_update_bytes: u64,
    /// How long, in milliseconds, the server waits for a worker's answer before it
    /// kills the worker and starts another. At least 1.
    pub collab_worker_timeout_ms: u64,
    /// The address space a worker may take, in bytes, where the system limits it
    /// (Unix). At least 1 GiB, and room for a document of `collab_state_bytes`
    /// ([`CollabSettings::held_bytes`]).
    pub collab_worker_memory_bytes: u64,
    /// How often, in seconds, the server asks again whether each connection in the
    /// room of a body may be there, and how. 1 to 3600.
    pub collab_recheck_seconds: u64,
    /// In how many rooms one websocket connection may be at once. 1 to 1024.
    pub collab_documents_per_connection: u32,
    /// How many websocket connections may be in the room of one body. 1 to 10000.
    pub collab_connections_per_document: u32,
    /// How many of the websocket connections in the room of one body may be one
    /// user's. 1 to 10000.
    pub collab_connections_per_user: u32,
    /// How many frames of the rooms one websocket connection may send in a second,
    /// and at once after a pause; a `joinDoc` and a sync step 1 count as five. 10
    /// to 1000000.
    pub collab_frames_per_second: u32,
    /// How many messages of the rooms may wait for one websocket connection. 16 to
    /// 65536.
    pub collab_queue_messages: u32,
    /// How many versions of a card's body are kept, the newest. 2 to 1000: a write of
    /// a whole body keeps the body before and the body after.
    pub collab_body_versions: u32,
    /// Maximum total uncompressed serialized JSON bytes of kept body versions per card.
    /// At least 1; the newest version is always kept even if it alone exceeds this limit.
    pub body_versions_max_bytes: u64,
    /// How many minutes after the first change no version of a body holds the next
    /// version is kept, while the body is edited on. 1 to 1440.
    pub collab_version_minutes: u32,
    /// How a worker is started; `None` for this executable. Not a configuration file
    /// key: the tests, whose executable is no server, set it.
    #[serde(skip)]
    pub worker_command: Option<crate::collab::worker::WorkerCommand>,
}

/// The default of `collab_state_bytes`: 16 MiB. Fits the largest document of an
/// 8 MiB / 100,000-node body (including marked documents up to 786,432 total changes)
/// with room for edits and deletions before remaking at three quarters of this limit.
/// It is no larger because a worker pays for a state of many small changes in memory
/// far more than for its bytes ([`MEMORY_PER_STATE_BYTE`]).
pub(super) const DEFAULT_STATE_BYTES: u64 = 16 * 1024 * 1024;

/// The default of `collab_update_bytes`: 64 KiB. What an update of this size may be
/// made to cost a worker is held by the steps its changes take to place: changes
/// walked past and letters cut (`fvoci_collab::PLACING_STEPS`), not by its size.
pub(super) const DEFAULT_UPDATE_BYTES: u64 = 64 * 1024;

/// The most workers that `collab_workers` may ask for.
pub(super) const MOST_WORKERS: u32 = 64;

/// The least `collab_state_bytes`: a body may be 8 MiB of JSON (100,000 nodes), its
/// document is larger than that, and it grows by what is deleted from it until it is
/// made again.
pub(super) const LEAST_STATE_BYTES: u64 = 16 * 1024 * 1024;

/// The most `collab_state_bytes`: a frame between the server and a worker counts its
/// length in four bytes.
pub(super) const MOST_STATE_BYTES: u64 = 1024 * 1024 * 1024;

/// The least `collab_worker_memory_bytes`: [`UPDATE_MEMORY_BYTES`] (512 MiB) and a
/// document of the least `collab_state_bytes` (32 × 16 MiB = 512 MiB) = 1 GiB.
pub(super) const LEAST_WORKER_MEMORY_BYTES: u64 = 1024 * 1024 * 1024;

/// The least `collab_frames_per_second`: a connection comes into a room (which
/// counts as five) and writes at once.
pub(super) const LEAST_FRAMES_PER_SECOND: u32 = 10;

/// The least `collab_queue_messages`: a connection in the eight rooms of the default
/// is sent each document anew (two messages a room) with none of them put out.
pub(super) const LEAST_QUEUE_MESSAGES: u32 = 16;

/// The most `collab_recheck_seconds`: an hour. A room that asks less often lets who
/// was taken off a board write on for longer than anybody would expect.
pub(super) const MOST_RECHECK_SECONDS: u64 = 3600;

/// The least `collab_body_versions`: a write of a whole body keeps the body it
/// writes over and the body it writes, and with one version the first would go as the
/// second comes.
pub(super) const LEAST_BODY_VERSIONS: u32 = 2;

/// The most `collab_body_versions`: every version is a whole body, and the files
/// its images show are kept as long as it is.
pub(super) const MOST_BODY_VERSIONS: u32 = 1000;

/// The most `collab_version_minutes`: a day.
pub(super) const MOST_VERSION_MINUTES: u32 = 24 * 60;

/// How many bytes of a worker's memory every byte of the states of the documents it
/// holds takes: a document of nodes in memory is 20 to 26 times its state (53–60 MB for
/// the 2.2–2.5 MB of 100,000 nodes; a document of 8 MiB text is 11.7 MB, 1.4x its state),
/// as the program asks for it, and what the allocator keeps around that comes on top.
///
/// A state of many changes of a few bytes each takes more than this (a change is some
/// hundred bytes in memory however short), which is not counted: it is what a client
/// makes on purpose, and the worker that runs out of memory for it is started again.
pub(super) const MEMORY_PER_STATE_BYTE: u64 = 32;

/// The memory of a worker that is kept free of the documents it holds, for the one
/// update it takes at a time: an update is applied and repaired in place on the held
/// document with incremental block and reading caches (`+0.0 MB` heap spike on live edits
/// and deletion-only repairs across all 100,000-node / 8 MiB shapes; worst-case held
/// document is 210 MB for 100k `marks and formulas`, 11.7 MB state), and the held
/// document is dropped before reading the settled state back so at most one `yrs::Doc`
/// is live at once. Every body is so taken, by a worker that holds as many documents as
/// it counts room for (including un-folded rollback updates in `since`); the executable
/// and what the allocator keeps come on top.
///
/// A state grown by what was deleted from it can still take more if its allocator
/// fragments under adversarial input, which is not counted, as [`MEMORY_PER_STATE_BYTE`]
/// does not count it: the worker that runs out of memory for it is started again, and
/// the sender of the update it was taking is held back for a little while
/// (`collab::senders`).
pub(super) const UPDATE_MEMORY_BYTES: u64 = 512 * 1024 * 1024;

impl Default for CollabSettings {
    fn default() -> Self {
        Self {
            collab_workers: 1,
            collab_documents_per_worker: 500,
            collab_state_bytes: DEFAULT_STATE_BYTES,
            collab_update_bytes: DEFAULT_UPDATE_BYTES,
            collab_worker_timeout_ms: 10_000,
            collab_worker_memory_bytes: 1536 * 1024 * 1024,
            collab_recheck_seconds: 30,
            collab_documents_per_connection: 8,
            collab_connections_per_document: 50,
            collab_connections_per_user: 5,
            collab_frames_per_second: 50,
            collab_queue_messages: 512,
            collab_body_versions: fvoci_store::DEFAULT_BODY_VERSIONS_KEPT,
            body_versions_max_bytes: fvoci_store::DEFAULT_BODY_VERSIONS_MAX_BYTES,
            collab_version_minutes: 10,
            worker_command: None,
        }
    }
}

impl CollabSettings {
    /// The bytes of the states (and un-folded rollback updates) of all its documents a
    /// worker may hold together: what of its memory is not kept for the update it takes
    /// (512 MiB), divided by what a byte of a state takes in memory (32). 32 MiB with
    /// the memory of the default.
    #[must_use]
    pub fn held_bytes(&self) -> u64 {
        self.collab_worker_memory_bytes
            .saturating_sub(UPDATE_MEMORY_BYTES)
            / MEMORY_PER_STATE_BYTE
    }

    /// How long after the first change no version of a body holds the next is due.
    #[must_use]
    pub fn version_interval(&self) -> std::time::Duration {
        std::time::Duration::from_secs(u64::from(self.collab_version_minutes) * 60)
    }

    /// Checks the settings against their ranges.
    ///
    /// # Errors
    ///
    /// Returns an error naming the first setting out of its range.
    pub fn check(&self) -> anyhow::Result<()> {
        if self.collab_workers == 0 || self.collab_workers > MOST_WORKERS {
            bail!(
                "collab_workers must be 1 to {MOST_WORKERS}, not {}",
                self.collab_workers
            );
        }
        if self.collab_documents_per_worker == 0 {
            bail!("collab_documents_per_worker must be at least 1");
        }
        if !(LEAST_STATE_BYTES..=MOST_STATE_BYTES).contains(&self.collab_state_bytes) {
            bail!(
                "collab_state_bytes must be {LEAST_STATE_BYTES} to {MOST_STATE_BYTES}, not {}",
                self.collab_state_bytes
            );
        }
        if self.collab_update_bytes == 0 || self.collab_update_bytes > self.collab_state_bytes {
            bail!(
                "collab_update_bytes must be 1 to collab_state_bytes ({}), not {}",
                self.collab_state_bytes,
                self.collab_update_bytes
            );
        }
        for (key, value) in [
            ("collab_worker_timeout_ms", self.collab_worker_timeout_ms),
            ("body_versions_max_bytes", self.body_versions_max_bytes),
        ] {
            if value == 0 {
                bail!("{key} must be at least 1");
            }
        }
        if self.held_bytes() < self.collab_state_bytes {
            bail!(
                "collab_worker_memory_bytes must be at least {} ({UPDATE_MEMORY_BYTES} and \
                 {MEMORY_PER_STATE_BYTE} times collab_state_bytes, {}), not {}",
                self.collab_state_bytes
                    .saturating_mul(MEMORY_PER_STATE_BYTE)
                    .saturating_add(UPDATE_MEMORY_BYTES),
                self.collab_state_bytes,
                self.collab_worker_memory_bytes
            );
        }
        if self.collab_worker_memory_bytes < LEAST_WORKER_MEMORY_BYTES {
            bail!(
                "collab_worker_memory_bytes must be at least {LEAST_WORKER_MEMORY_BYTES}, not {}",
                self.collab_worker_memory_bytes
            );
        }
        self.check_rooms_and_versions()
    }

    /// Checks the room concurrency and body version limits.
    fn check_rooms_and_versions(&self) -> anyhow::Result<()> {
        if !(1..=MOST_RECHECK_SECONDS).contains(&self.collab_recheck_seconds) {
            bail!(
                "collab_recheck_seconds must be 1 to {MOST_RECHECK_SECONDS}, not {}",
                self.collab_recheck_seconds
            );
        }
        for (key, value, range) in [
            (
                "collab_documents_per_connection",
                self.collab_documents_per_connection,
                1..=1024,
            ),
            (
                "collab_connections_per_document",
                self.collab_connections_per_document,
                1..=10_000,
            ),
            (
                "collab_connections_per_user",
                self.collab_connections_per_user,
                1..=10_000,
            ),
            (
                "collab_frames_per_second",
                self.collab_frames_per_second,
                LEAST_FRAMES_PER_SECOND..=1_000_000,
            ),
            (
                "collab_queue_messages",
                self.collab_queue_messages,
                LEAST_QUEUE_MESSAGES..=65_536,
            ),
            (
                "collab_body_versions",
                self.collab_body_versions,
                LEAST_BODY_VERSIONS..=MOST_BODY_VERSIONS,
            ),
            (
                "collab_version_minutes",
                self.collab_version_minutes,
                1..=MOST_VERSION_MINUTES,
            ),
        ] {
            if !range.contains(&value) {
                bail!(
                    "{key} must be {} to {}, not {value}",
                    range.start(),
                    range.end()
                );
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_defaults_are_in_range_and_each_bound_is_checked() {
        CollabSettings::default().check().expect("the defaults");
        let refused = |change: fn(&mut CollabSettings), key: &str| {
            let mut settings = CollabSettings::default();
            change(&mut settings);
            let error = settings.check().expect_err(key).to_string();
            assert!(error.contains(key), "{error}");
        };
        refused(|s| s.collab_workers = 0, "collab_workers");
        refused(|s| s.collab_workers = MOST_WORKERS + 1, "collab_workers");
        refused(
            |s| s.collab_documents_per_worker = 0,
            "collab_documents_per_worker",
        );
        refused(
            |s| s.collab_state_bytes = LEAST_STATE_BYTES - 1,
            "collab_state_bytes",
        );
        refused(
            |s| s.collab_state_bytes = MOST_STATE_BYTES + 1,
            "collab_state_bytes",
        );
        refused(|s| s.collab_update_bytes = 0, "collab_update_bytes");
        refused(
            |s| s.collab_update_bytes = s.collab_state_bytes + 1,
            "collab_update_bytes",
        );
        refused(
            |s| s.collab_worker_timeout_ms = 0,
            "collab_worker_timeout_ms",
        );
        refused(
            |s| s.collab_worker_memory_bytes = LEAST_WORKER_MEMORY_BYTES - 1,
            "collab_worker_memory_bytes",
        );
        refused(|s| s.collab_recheck_seconds = 0, "collab_recheck_seconds");
    }

    /// A worker's memory has room for the update it takes and for a document at its
    /// limit: a state of the default is held with the least memory, and a larger one
    /// only with more.
    #[test]
    fn the_memory_of_a_worker_has_room_for_a_document_at_its_limit() {
        let defaults = CollabSettings::default();
        assert_eq!(defaults.held_bytes(), 32 * 1024 * 1024);
        let with = |state: u64, memory: u64| CollabSettings {
            collab_state_bytes: state,
            collab_worker_memory_bytes: memory,
            ..CollabSettings::default()
        };
        let least = with(LEAST_STATE_BYTES, LEAST_WORKER_MEMORY_BYTES);
        least.check().expect("the least of both");
        assert!(least.held_bytes() >= LEAST_STATE_BYTES);
        let needed = |state: u64| UPDATE_MEMORY_BYTES + MEMORY_PER_STATE_BYTE * state;
        let larger = 64 * 1024 * 1024;
        with(larger, needed(larger)).check().expect("room for it");
        let error = with(larger, needed(larger) - 1)
            .check()
            .expect_err("a byte short")
            .to_string();
        assert!(
            error.contains(&format!("at least {}", needed(larger))),
            "{error}"
        );
    }

    #[test]
    fn the_bounds_of_the_rooms_are_checked() {
        let refused = |change: fn(&mut CollabSettings), key: &str| {
            let mut settings = CollabSettings::default();
            change(&mut settings);
            let error = settings.check().expect_err(key).to_string();
            assert!(error.contains(key), "{error}");
        };
        refused(
            |s| s.collab_documents_per_connection = 0,
            "collab_documents_per_connection",
        );
        refused(
            |s| s.collab_documents_per_connection = 1025,
            "collab_documents_per_connection",
        );
        refused(
            |s| s.collab_connections_per_document = 0,
            "collab_connections_per_document",
        );
        refused(
            |s| s.collab_connections_per_document = 10_001,
            "collab_connections_per_document",
        );
        refused(
            |s| s.collab_connections_per_user = 0,
            "collab_connections_per_user",
        );
        refused(
            |s| s.collab_connections_per_user = 10_001,
            "collab_connections_per_user",
        );
        refused(
            |s| s.collab_frames_per_second = LEAST_FRAMES_PER_SECOND - 1,
            "collab_frames_per_second",
        );
        refused(
            |s| s.collab_frames_per_second = 1_000_001,
            "collab_frames_per_second",
        );
        refused(
            |s| s.collab_queue_messages = LEAST_QUEUE_MESSAGES - 1,
            "collab_queue_messages",
        );
        refused(
            |s| s.collab_queue_messages = 65_537,
            "collab_queue_messages",
        );
        refused(
            |s| s.collab_recheck_seconds = MOST_RECHECK_SECONDS + 1,
            "collab_recheck_seconds",
        );
    }

    #[test]
    fn the_bounds_of_the_versions_are_checked() {
        let settings = CollabSettings::default();
        assert_eq!(settings.collab_body_versions, 50);
        assert_eq!(settings.body_versions_max_bytes, 64 * 1024 * 1024);
        assert_eq!(
            settings.version_interval(),
            std::time::Duration::from_mins(10)
        );
        let refused = |change: fn(&mut CollabSettings), key: &str| {
            let mut settings = CollabSettings::default();
            change(&mut settings);
            let error = settings.check().expect_err(key).to_string();
            assert!(error.contains(key), "{error}");
        };
        refused(
            |s| s.collab_body_versions = LEAST_BODY_VERSIONS - 1,
            "collab_body_versions",
        );
        refused(
            |s| s.collab_body_versions = MOST_BODY_VERSIONS + 1,
            "collab_body_versions",
        );
        refused(|s| s.body_versions_max_bytes = 0, "body_versions_max_bytes");
        refused(|s| s.collab_version_minutes = 0, "collab_version_minutes");
        refused(
            |s| s.collab_version_minutes = MOST_VERSION_MINUTES + 1,
            "collab_version_minutes",
        );
    }
}
