//! The comparison of the file storage with the records: the objects nothing names, and
//! the places records name where the storage holds nothing.
//!
//! It runs over every organization: the objects of all of them share the storage. The
//! storage is read a page at a time and the records a page at a time, so that neither
//! is held in memory whole. What nothing names is doomed, if asked for, only at a place
//! of the server's making and only once it is older than [`UNRECORDED_MIN_AGE_MILLIS`];
//! the housekeeping removes it, asking again first whether something holds it. What
//! nothing names elsewhere in the storage is reported and never changed, nor is a
//! record without its object: what to do with those is not the server's to say.

use futures_util::{StreamExt as _, TryStreamExt as _};
use fvoci_model::FileKey;
use serde::Serialize;

use super::App;
use super::uploads::UPLOAD_LIFETIME_MILLIS;
use crate::error::AppError;
use crate::storage::{StorageError, StoredObject};

/// How old an object nothing names must be before a comparison dooms it: one written
/// just now may be about to be recorded. An upload session's life.
pub const UNRECORDED_MIN_AGE_MILLIS: i64 = UPLOAD_LIFETIME_MILLIS;

/// How many objects, or records, a comparison reads at once.
const PAGE: usize = 500;

/// The most places a report names of each kind; it counts them all.
const SHOWN: usize = 100;

/// How many objects a comparison looks up in the storage at once.
const LOOKUPS_AT_ONCE: usize = 16;

/// What a comparison does with the objects nothing names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unrecorded {
    /// Reports them, and changes nothing.
    Report,
    /// Reports them, and dooms those at places of the server's making that are older
    /// than [`UNRECORDED_MIN_AGE_MILLIS`].
    Doom,
}

/// Places a comparison found: how many, and the first hundred of them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Found {
    /// How many there are.
    pub count: u64,
    /// The first of them, in the order they were found.
    pub paths: Vec<String>,
}

impl Found {
    fn note(&mut self, path: &str) {
        self.count += 1;
        if self.paths.len() < SHOWN {
            self.paths.push(path.to_owned());
        }
    }
}

/// What a comparison of the file storage with the records found.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Reconciliation {
    /// The objects the storage holds.
    pub objects: u64,
    /// The objects at places of the server's making that nothing names: no record,
    /// live or in the trash, no write under way, no upload session.
    pub unrecorded: Found,
    /// The objects nothing names at places the server does not make, such as another
    /// application's in the same bucket or the server's own temporary files under the
    /// files directory: reported, and never doomed.
    pub foreign: Found,
    /// How many of them this comparison doomed.
    pub doomed: u64,
    /// The places the records name.
    pub records: u64,
    /// The places records name where the storage holds nothing.
    pub missing: Found,
}

impl App {
    /// Compares the file storage with the records of every organization the handle
    /// reaches (the housekeeping's: all of them), at `now`. With [`Unrecorded::Report`]
    /// it changes nothing.
    ///
    /// # Errors
    ///
    /// Returns the storage's or the store's error if a page cannot be read; nothing is
    /// reported then, and what was doomed before stays doomed.
    pub async fn reconcile_files(
        &self,
        unrecorded: Unrecorded,
        now: i64,
    ) -> Result<Reconciliation, AppError> {
        let mut report = Reconciliation::default();
        let old_enough = now.saturating_sub(UNRECORDED_MIN_AGE_MILLIS);
        let mut listing = self.files().list().try_chunks(PAGE);
        while let Some(page) = listing.next().await {
            let page: Vec<StoredObject> = page.map_err(|error| AppError::from(error.1))?;
            report.objects += u64::try_from(page.len()).unwrap_or(u64::MAX);
            // Listed first, asked after: an object recorded meanwhile is held by then.
            let paths: Vec<&str> = page.iter().map(|object| object.path.as_str()).collect();
            let held = self.store().held_objects(&paths, None).await?;
            let mut doomed = Vec::new();
            for object in page.iter().filter(|object| !held.contains(&object.path)) {
                if FileKey::parse(&object.path).is_none() {
                    report.foreign.note(&object.path);
                    continue;
                }
                report.unrecorded.note(&object.path);
                if object.modified_at <= old_enough {
                    doomed.push(object.path.as_str());
                }
            }
            if unrecorded == Unrecorded::Doom && !doomed.is_empty() {
                report.doomed += self.store().doom_unrecorded(&doomed).await?;
            }
        }

        let page_size = i64::try_from(PAGE).unwrap_or(i64::MAX);
        let mut after = String::new();
        loop {
            let page = self.store().recorded_paths_after(&after, page_size).await?;
            let Some(last) = page.last() else {
                break;
            };
            after.clone_from(last);
            report.records += u64::try_from(page.len()).unwrap_or(u64::MAX);
            let looked_up: Vec<(String, Result<Option<u64>, StorageError>)> =
                futures_util::stream::iter(page)
                    .map(|path| async move {
                        let size = self.files().size(&path).await;
                        (path, size)
                    })
                    .buffered(LOOKUPS_AT_ONCE)
                    .collect()
                    .await;
            for (path, size) in looked_up {
                match size {
                    Ok(Some(_)) => {}
                    // A place no storage takes holds nothing.
                    Ok(None) | Err(StorageError::InvalidPath(_)) => report.missing.note(&path),
                    Err(error) => return Err(error.into()),
                }
            }
        }
        tracing::info!(
            objects = report.objects,
            unrecorded = report.unrecorded.count,
            doomed = report.doomed,
            records = report.records,
            missing = report.missing.count,
            "compared the file storage with the records"
        );
        Ok(report)
    }
}
