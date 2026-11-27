//! Removing the objects of the file storage that are doomed: their record went, or they
//! never had one (`fvoci_store::LooseObject`).
//!
//! The store lists an object as doomed in the transaction that removes its record. The
//! housekeeping removes it here, unless a record, a write or an upload session of any
//! organization names its place (`fvoci_store::Store::held_objects`), and takes it off
//! the list only once it is gone: a server that stops in between finds it listed again
//! and removes it again, which for an object that is gone already is nothing. A removal
//! that fails is tried again later, each time later still, up to [`RETRY_CAP_MILLIS`];
//! and a run takes the due ones the longest due first, so that objects that keep
//! failing do not hold back the others. One at a place no storage takes is taken off
//! the list with an error logged.

use std::collections::BTreeSet;
use std::time::Duration;

use fvoci_model::FileRecord;
use tokio::io::AsyncRead;

use super::App;
use super::uploads::UPLOAD_LIFETIME_MILLIS;
use crate::auth::now_millis;
use crate::error::AppError;
use crate::metrics::LooseObjects;
use crate::storage::StorageError;

/// How long writing an object may take: past it the housekeeping takes the writing for
/// stopped, and dooms the object. Twice as long as an upload session lasts, which no
/// write of a file takes, whether a request's body, a completion or an import; one that
/// takes longer all the same fails, and leaves nothing (`Store::record_written_file`).
pub const WRITING_GRACE_MILLIS: i64 = 2 * UPLOAD_LIFETIME_MILLIS;

/// How many doomed objects a run reads from the store at once.
const DOOMED_PER_BATCH: i64 = 200;

/// The most doomed objects a run tries to remove; the rest wait for the next run.
const DOOMED_PER_RUN: usize = 5_000;

/// How long after its first failure a removal is tried again: the interval of the
/// housekeeping, so that each further try, which doubles it up to [`RETRY_CAP_MILLIS`],
/// is a later run.
const RETRY_BASE_MILLIS: i64 = 60 * 60 * 1000;

/// The longest a doomed object waits between two tries.
const RETRY_CAP_MILLIS: i64 = 24 * 60 * 60 * 1000;

/// What a run of the removal of doomed objects did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RemovedObjects {
    /// The objects removed, or found gone, and taken off the list.
    pub removed: usize,
    /// The objects that could not be removed, or taken off the list, and wait for
    /// another try.
    pub failed: usize,
    /// The doomed objects that a record, a write or an upload session holds, which are
    /// kept and taken off the list: something doomed what it should not have, an error.
    pub kept: usize,
}

/// A doomed object to remove: its place, and how many of its removals failed.
#[derive(Debug, Clone, Copy)]
struct Doomed<'a> {
    path: &'a str,
    attempts: i64,
}

/// How far a write that is given up got: whether its object may yet appear.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Written {
    /// The write is over, and what is there is all there will be: the record failed.
    Settled,
    /// The write failed, and the storage may have taken it all the same, its answer
    /// lost; an object may yet appear.
    Unsettled,
}

/// How long the object of a write that failed is left before it is removed: what the
/// storage took with its answer lost is there by then.
const WRITE_SETTLE_MILLIS: i64 = 60 * 60 * 1000;

/// The longest a write or a copy of an object may take before it is given up: an upload
/// session's life, well within [`WRITING_GRACE_MILLIS`], so that no write goes on
/// after the housekeeping took it for stopped.
pub(crate) const WRITE_TIMEOUT: Duration =
    Duration::from_millis(UPLOAD_LIFETIME_MILLIS.unsigned_abs());

/// A storage key marked in `loose_objects` as being written, awaiting bytes or copy.
#[must_use = "a marked object place must be written or given up"]
pub(crate) struct MarkedPlace {
    app: App,
    path: String,
}

/// A storage key whose bytes were written under a `loose_objects` mark, awaiting its DB
/// record or batch transaction commit.
#[must_use = "a written object place must be recorded in the store or given up"]
pub(crate) struct WrittenPlace {
    app: App,
    path: String,
    size: u64,
}

impl MarkedPlace {
    /// Writes `reader` to the marked storage key within [`WRITE_TIMEOUT`]. On timeout or
    /// storage failure, dooms the mark as unsettled.
    pub(crate) async fn write(
        self,
        reader: &mut (dyn AsyncRead + Send + Unpin),
    ) -> Result<WrittenPlace, AppError> {
        let Self { app, path } = self;
        match tokio::time::timeout(WRITE_TIMEOUT, app.files().write(&path, reader)).await {
            Ok(Ok(size)) => Ok(WrittenPlace { app, path, size }),
            failed => {
                app.give_up_writing(&path, Written::Unsettled).await;
                let error = match failed {
                    Ok(Err(error)) => error.to_string(),
                    _ => "it took longer than a write may take".to_owned(),
                };
                Err(AppError::internal(format_args!(
                    "unable to store the file in the files storage: {error}"
                )))
            }
        }
    }

    /// Writes `reader` within `timeout`, dooming the mark as unsettled on failure or
    /// timeout and returning `Err(None)` on timeout or `Err(Some(StorageError))` on I/O
    /// error.
    pub(crate) async fn write_within(
        self,
        reader: &mut (dyn AsyncRead + Send + Unpin),
        timeout: Duration,
    ) -> Result<WrittenPlace, Option<StorageError>> {
        let Self { app, path } = self;
        match tokio::time::timeout(timeout, app.files().write(&path, reader)).await {
            Ok(Ok(size)) => Ok(WrittenPlace { app, path, size }),
            Ok(Err(error)) => {
                app.give_up_writing(&path, Written::Unsettled).await;
                Err(Some(error))
            }
            Err(_) => {
                app.give_up_writing(&path, Written::Unsettled).await;
                Err(None)
            }
        }
    }

    /// Copies `source_path` to this marked storage key within [`WRITE_TIMEOUT`]. On
    /// timeout or storage failure, dooms the mark as unsettled.
    pub(crate) async fn copy_from(
        self,
        source_path: &str,
        size: u64,
    ) -> Result<WrittenPlace, StorageError> {
        let Self { app, path } = self;
        let copied = tokio::time::timeout(WRITE_TIMEOUT, app.files().copy(source_path, &path))
            .await
            .unwrap_or_else(|_| Err(StorageError::Io(std::io::ErrorKind::TimedOut.into())));
        match copied {
            Ok(()) => Ok(WrittenPlace { app, path, size }),
            Err(error) => {
                app.give_up_writing(&path, Written::Unsettled).await;
                Err(error)
            }
        }
    }

    /// Releases a marked key before any write to storage was started.
    pub(crate) async fn give_up_unused(self) {
        self.app.give_up_writing(&self.path, Written::Settled).await;
    }
}

impl WrittenPlace {
    /// Number of bytes written to storage.
    #[must_use]
    pub(crate) fn size(&self) -> u64 {
        self.size
    }

    /// Storage key where the object was written.
    #[must_use]
    pub(crate) fn path(&self) -> &str {
        &self.path
    }

    /// Commits `record` via `Store::record_written_file`; if the DB write fails, dooms
    /// and cleans up the written object immediately.
    pub(crate) async fn record(self, record: &FileRecord) -> Result<(), AppError> {
        let Self { app, path, .. } = self;
        match app.store().record_written_file(record).await {
            Ok(true) => Ok(()),
            Ok(false) => Err(AppError::internal(
                "the file was given up while it was written",
            )),
            Err(error) => {
                app.give_up_writing(&path, Written::Settled).await;
                Err(error.into())
            }
        }
    }

    /// Dooms and cleans up the written object when validation or a batch transaction
    /// rolls back.
    pub(crate) async fn give_up(self) {
        self.app.give_up_writing(&self.path, Written::Settled).await;
    }

    /// Consumes the witness after a multi-file database transaction atomically recorded
    /// the file and cleared its `writing` mark (`loose_objects::record_written`).
    pub(crate) fn settled_in_transaction(self) {
        let _ = self.path;
    }
}

/// What became of one doomed object.
enum Removal {
    Removed,
    Failed,
    Kept,
}

impl App {
    /// Dooms the objects whose writing stopped long ago, then removes the doomed
    /// objects that are due at `now`, up to 5,000 a run, and counts what is listed
    /// into the metrics. What cannot be removed is noted with the kind of its error and
    /// tried again later.
    ///
    /// # Errors
    ///
    /// Returns a 503 error if this process does not hold the lock of the server (also
    /// once it lost it during the run), and the store's error if the list cannot be
    /// read, or counted for the metrics.
    pub async fn remove_doomed_objects(&self, now: i64) -> Result<RemovedObjects, AppError> {
        self.require_server_lock()?;
        // While rows name places otherwise than the one way, every place counts as held
        // (`Store::held_objects`): the doomed objects wait, listed, for those rows to be
        // written the one way, rather than be taken off the list as kept.
        if self.store().other_spellings_stored().await? {
            tracing::warn!("doomed objects are not removed while rows name places another way");
            self.count_loose_objects().await?;
            return Ok(RemovedObjects::default());
        }
        // What cannot be doomed now is doomed by a later run; the removals go on.
        match self.store().doom_stale_writing(now).await {
            Ok(0) => {}
            Ok(stale) => tracing::warn!(stale, "doomed objects whose writing stopped long ago"),
            Err(error) => {
                tracing::error!(%error, "unable to doom the objects whose writing stopped");
            }
        }
        let mut done = RemovedObjects::default();
        while done.removed + done.failed + done.kept < DOOMED_PER_RUN {
            self.require_server_lock()?;
            let due = self
                .store()
                .due_doomed_objects(now, DOOMED_PER_BATCH)
                .await?;
            let last_batch = i64::try_from(due.len()).unwrap_or(i64::MAX) < DOOMED_PER_BATCH;
            // A place two organizations doomed is one object, taken off both lists at
            // once.
            let mut paths: Vec<&str> = due.iter().map(|object| object.path.as_str()).collect();
            paths.sort_unstable();
            paths.dedup();
            // Asked of every organization before anything is removed, once a batch: a
            // record, a write or a session that names the place keeps the object,
            // whatever doomed it.
            let held = match self.store().held_objects(&paths, None).await {
                Ok(held) => held,
                Err(error) => {
                    tracing::warn!(%error, "whether the doomed objects are held could not be read");
                    done.failed += paths.len();
                    break;
                }
            };
            let mut progress = 0;
            let mut seen = BTreeSet::new();
            for object in due
                .iter()
                .filter(|object| seen.insert(object.path.as_str()))
            {
                // What was read before the lock was lost is not acted on: the next
                // server may have written at these places since.
                self.require_server_lock()?;
                let removal = if held.contains(&object.path) {
                    self.keep_held(&object.path, now).await
                } else {
                    let doomed = Doomed {
                        path: &object.path,
                        attempts: object.attempts,
                    };
                    self.remove_doomed_object(doomed, now).await
                };
                match removal {
                    Removal::Removed => done.removed += 1,
                    Removal::Failed => {
                        done.failed += 1;
                        continue;
                    }
                    Removal::Kept => done.kept += 1,
                }
                progress += 1;
            }
            // A batch of which nothing went off the list says that the storage or the
            // store is failing: the rest waits for the next run rather than fail too.
            if last_batch || progress == 0 {
                break;
            }
        }
        self.count_loose_objects().await?;
        Ok(done)
    }

    /// Counts the listed objects into the metrics, if they are kept.
    ///
    /// # Errors
    ///
    /// Returns the store's error if they cannot be counted; the metrics are left as
    /// they were, and the run that could not count them is a failed one.
    async fn count_loose_objects(&self) -> Result<(), AppError> {
        if self.metrics().is_kept() {
            let counts = self.store().loose_object_counts().await?;
            self.metrics().loose_objects_counted(&LooseObjects {
                writing: counts.writing,
                doomed: counts.doomed,
                failing: counts.failing,
            });
        }
        Ok(())
    }

    /// Keeps a doomed object that something holds, and takes it off the list: whatever
    /// doomed it was wrong, which is logged as an error.
    async fn keep_held(&self, path: &str, now: i64) -> Removal {
        tracing::error!(
            path,
            "a doomed object is held by a record, a write or an upload; it is kept"
        );
        if self.forget(path, now).await {
            Removal::Kept
        } else {
            Removal::Failed
        }
    }

    /// Removes one doomed object that nothing holds and takes it off the list. If it
    /// cannot go, the failure is noted on its row, or the row is left as it is.
    async fn remove_doomed_object(&self, doomed: Doomed<'_>, now: i64) -> Removal {
        let Doomed { path, attempts } = doomed;
        // The last look before the object goes: a process that knows it lost or let go
        // the lock of the server removes nothing more, and leaves the row to the next.
        if !self.holds_server_lock() {
            tracing::warn!(
                path,
                "a doomed object is left: this process does not hold the lock of the server"
            );
            return Removal::Failed;
        }
        let error_kind = match self.files().delete(path).await {
            // Gone already is removed: a run cut off after the removal comes here.
            Ok(()) => {
                return if self.forget(path, now).await {
                    Removal::Removed
                } else {
                    Removal::Failed
                };
            }
            // A place no storage takes is none to remove: tried again, it would fail
            // for ever.
            Err(error @ StorageError::InvalidPath(_)) => {
                tracing::error!(%error, "a doomed object has a place no storage takes; it is taken off the list");
                return if self.forget(path, now).await {
                    Removal::Removed
                } else {
                    Removal::Failed
                };
            }
            Err(error) => {
                tracing::warn!(%error, attempts = attempts + 1, "a doomed object could not be removed");
                error_kind(&error)
            }
        };
        let retry_at = now.saturating_add(retry_delay(attempts + 1));
        if let Err(error) = self
            .store()
            .doomed_object_failed(path, now, error_kind, retry_at)
            .await
        {
            tracing::warn!(%error, "a failed removal could not be noted");
        }
        Removal::Failed
    }

    /// Removes now an object just doomed at `path`, which nothing holds, and takes it
    /// off the list. Returns whether it went; if not, it stays doomed for the
    /// housekeeping.
    pub(crate) async fn remove_doomed_now(&self, path: &str) -> bool {
        let doomed = Doomed { path, attempts: 0 };
        matches!(
            self.remove_doomed_object(doomed, now_millis()).await,
            Removal::Removed
        )
    }

    /// Marks the object about to be written at `path` and returns a [`MarkedPlace`]
    /// witness that must be written and recorded or given up.
    ///
    /// # Errors
    ///
    /// Returns the store's error; nothing is to be written then.
    pub(crate) async fn mark_place(
        &self,
        path: impl Into<String>,
    ) -> Result<MarkedPlace, AppError> {
        let path = path.into();
        self.mark_writing(&path).await?;
        Ok(MarkedPlace {
            app: self.clone(),
            path,
        })
    }

    /// Marks the object about to be written at `path`.
    async fn mark_writing(&self, path: &str) -> Result<(), AppError> {
        let doom_at = now_millis().saturating_add(WRITING_GRACE_MILLIS);
        Ok(self.store().mark_writing(path, doom_at).await?)
    }

    /// Gives up the object being written at `path`, which will not be recorded, and
    /// dooms it.
    async fn give_up_writing(&self, path: &str, written: Written) {
        let now = now_millis();
        let remove_at = match written {
            Written::Settled => now,
            Written::Unsettled => now.saturating_add(WRITE_SETTLE_MILLIS),
        };
        match self.store().doom_writing(path, remove_at).await {
            Ok(true) if written == Written::Settled => {
                match self.store().held_objects(&[path], None).await {
                    Ok(held) if held.is_empty() => {
                        let doomed = Doomed { path, attempts: 0 };
                        self.remove_doomed_object(doomed, now).await;
                    }
                    // Held for an unknown spelling: left doomed, as the housekeeping
                    // leaves it.
                    Ok(_) if !matches!(self.store().other_spellings_stored().await, Ok(false)) => {}
                    Ok(_) => {
                        self.keep_held(path, now).await;
                    }
                    // Doomed: the housekeeping asks again.
                    Err(error) => {
                        tracing::warn!(%error, "whether a given-up object is held could not be read");
                    }
                }
            }
            Ok(_) => {}
            Err(error) => {
                tracing::warn!(%error, "an object that will not be recorded could not be doomed");
            }
        }
    }

    /// Takes a doomed object off the list, unless it was doomed again to be removed
    /// after `now`. Returns whether nothing was left to do.
    async fn forget(&self, path: &str, now: i64) -> bool {
        match self.store().forget_doomed_object(path, now).await {
            Ok(()) => true,
            Err(error) => {
                tracing::warn!(%error, "a doomed object could not be taken off the list");
                false
            }
        }
    }
}

/// The kind of a storage error, as a failed removal notes it and the metrics count it:
/// one of a few words, never a path or a key.
pub(crate) fn error_kind(error: &StorageError) -> &'static str {
    match error {
        StorageError::NotFound(_) => "not_found",
        StorageError::InvalidPath(_) => "invalid_path",
        StorageError::Config(_) => "config",
        StorageError::Io(_) => "io",
        StorageError::ObjectStore(_) => "object_store",
    }
}

/// How long to wait before the next try of a removal that failed `attempts` times.
fn retry_delay(attempts: i64) -> i64 {
    let doublings = u32::try_from(attempts.saturating_sub(1).clamp(0, 30)).unwrap_or(30);
    RETRY_BASE_MILLIS
        .saturating_mul(1_i64 << doublings)
        .min(RETRY_CAP_MILLIS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_kind_of_error_is_one_the_metrics_name() {
        let errors = [
            StorageError::NotFound(String::new()),
            StorageError::InvalidPath(String::new()),
            StorageError::Config(String::new()),
            StorageError::Io(std::io::Error::other("io")),
            StorageError::ObjectStore(object_store::Error::NotImplemented {
                operation: String::new(),
                implementer: String::new(),
            }),
        ];
        for error in &errors {
            assert!(
                crate::metrics::REMOVAL_ERRORS.contains(&error_kind(error)),
                "{error}"
            );
        }
    }

    #[test]
    fn a_removal_is_tried_again_later_each_time_up_to_a_day() {
        assert_eq!(retry_delay(1), RETRY_BASE_MILLIS, "after the first failure");
        assert_eq!(retry_delay(2), 2 * RETRY_BASE_MILLIS, "then twice as long");
        assert_eq!(retry_delay(3), 4 * RETRY_BASE_MILLIS, "and so on");
        assert_eq!(retry_delay(6), RETRY_CAP_MILLIS, "up to a day");
        assert_eq!(retry_delay(i64::MAX), RETRY_CAP_MILLIS, "however many");
        assert_eq!(retry_delay(0), RETRY_BASE_MILLIS, "never sooner");
    }
}
