//! Per-scope writer counts and committed visibility revisions.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use super::Reach;
use crate::Store;

/// Writers of a part of the data, its revision, and until when its reads are not
/// cached after one of its writes was abandoned.
#[derive(Debug, Default, Clone, Copy)]
pub(super) struct Count {
    pub(super) revision: u64,
    pub(super) writers: usize,
    pub(super) unsettled_until: Option<Instant>,
}

impl Count {
    pub(super) fn start(&mut self) {
        self.writers += 1;
        self.revision = self.revision.wrapping_add(1);
    }

    pub(super) fn end(&mut self, unsettled_until: Option<Instant>) {
        self.end_many(1, unsettled_until);
    }

    pub(super) fn end_many(&mut self, count: usize, unsettled_until: Option<Instant>) {
        self.writers = self.writers.saturating_sub(count);
        self.revision = self.revision.wrapping_add(1);
        if unsettled_until.is_some() {
            self.unsettled_until = unsettled_until;
        }
    }

    pub(super) fn settled_at(self, now: Instant) -> bool {
        self.unsettled_until.is_none_or(|until| now >= until)
    }
}

/// One count behind a lock of its own: a dispatch holds it while it queues.
pub(super) type Guarded = Arc<Mutex<Count>>;

/// A committed authorization revision of what one organization reads (or of
/// everything), and whether its reads may be cached.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VisibilityVersion {
    pub(super) reach: Reach,
    pub(super) shared: u64,
    pub(super) revision: u64,
    pub(super) cacheable: bool,
}

impl VisibilityVersion {
    /// False for a while after an abandoned write whose outcome was not known: reads
    /// are made anew each time then.
    pub fn cacheable(self) -> bool {
        self.cacheable
    }

    /// What the version is of: two versions of one scope may share a cache slot.
    pub fn scope(self) -> VisibilityScope {
        VisibilityScope(self.reach)
    }
}

/// What a [`VisibilityVersion`] is of: one organization, or everything.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct VisibilityScope(pub(super) Reach);

impl Store {
    /// The revision of the authorization data this handle's organization reads, after
    /// its pending writers finish (of every write, for a handle of none). Cached reads
    /// are reusable only at the same cacheable revision.
    pub async fn visibility_version(&self) -> VisibilityVersion {
        self.inner.visibility.version(self.tenant).await
    }

    /// Runs a dispatch only while its checked revision is current and settled: no
    /// write of its scope started or ended since.
    pub fn if_visibility_current(&self, version: VisibilityVersion, send: impl FnOnce()) -> bool {
        self.inner.visibility.if_current(version, send)
    }
}
