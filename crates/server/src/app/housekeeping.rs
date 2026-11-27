//! The reach of the server's housekeeping: the one place of the server that asks for
//! the rows of every organization; and the sweep of the temporary directories at the
//! start.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};

use fvoci_model::{ContainerKinds, TimeZone, add_days, day_at};
use fvoci_store::{DueSoonCardCandidate, Tenant};
use tokio::fs;

use super::App;
use crate::auth::now_millis;
use crate::error::AppError;
use crate::storage::WRITING_DIR;

/// Maximum candidate reviews read per round in [`App::notify_due_reviews`].
const REVIEW_NOTIFY_ROUND: i64 = 200;

/// Maximum rounds per [`App::notify_due_reviews`] invocation.
const REVIEW_NOTIFY_MAX_ROUNDS: usize = 10;

/// Maximum notifications kept per person (matches `notifications.rs`).
const MOST_NOTIFICATIONS_PER_USER: i64 = 10_000;

impl App {
    /// The server as its housekeeping uses it: reaching the rows of every organization.
    /// What the server throws away on nobody's behalf (sessions, uploads and temporary
    /// files that ended, what was in the trash too long) is looked for in all of them;
    /// as one organization, a clean-up would take what it does not see of the others
    /// for gone.
    ///
    /// Each job of the housekeeping takes this reach itself, whatever handle it is
    /// started with. Nothing that answers a request works through this.
    #[must_use]
    pub fn housekeeping(&self) -> Self {
        self.scoped(Tenant::System)
    }

    /// Removes the former providers whose time ran out (`oidc_former_provider_days`),
    /// of every organization, each recorded in its organization's audit log as the
    /// server's own doing. Until this runs such a one is read as none already. It
    /// takes the housekeeping's reach itself; what fails is logged, and left for the
    /// next run.
    pub async fn end_lapsed_former_providers(&self) {
        let app = self.housekeeping();
        if let Err(error) = app.require_server_lock() {
            tracing::warn!(%error, "the former providers that lapsed are not removed");
            return;
        }
        match app
            .store()
            .end_lapsed_oidc_former_providers(now_millis())
            .await
        {
            Ok(0) => {}
            Ok(ended) => tracing::info!(ended, "removed former providers whose time ran out"),
            Err(error) => {
                tracing::error!(%error, "unable to remove the former providers that lapsed");
            }
        }
    }

    /// Removes for good the workspaces of every organization that have been in the
    /// trash since before `deleted_before`, each in its organization, with what is in
    /// them. Returns how many boards and blocks went, how many files were doomed, and
    /// how many workspaces could not be removed: one that fails is logged and left for
    /// the next run, and the others go. For the housekeeping, whose reach this is.
    ///
    /// # Errors
    ///
    /// Returns the store's error if the workspaces to remove cannot be read.
    pub async fn purge_workspaces(&self, deleted_before: i64) -> Result<TrashPurged, AppError> {
        let mut done = TrashPurged::default();
        for (org_id, workspace_id) in self.store().workspaces_to_purge(deleted_before).await? {
            let purged = match self.store().for_org(&org_id) {
                Ok(store) => store.purge_workspace(&workspace_id, deleted_before).await,
                Err(error) => Err(error),
            };
            match purged {
                Ok(purged) => {
                    done.records += purged.records;
                    done.files += purged.files.len();
                }
                Err(error) => {
                    tracing::error!(%error, org_id, workspace_id, "unable to purge a workspace");
                    done.failed += 1;
                }
            }
        }
        Ok(done)
    }

    /// Deletes the sessions that have ended by `now`, unused for the idle limit
    /// (`session_idle_days`) or past their absolute limit, and closes what each still has
    /// open. Returns how many ended.
    ///
    /// # Errors
    ///
    /// Returns the store's error if the sessions cannot be deleted.
    pub async fn clean_up_sessions(&self, now: i64) -> Result<usize, AppError> {
        let idle_since = now.saturating_sub(self.config().session_idle_millis());
        let ended = self.store().clean_up_sessions(now, idle_since).await?;
        let count = ended.session_ids().len();
        self.ws().close(ended.into_scope());
        Ok(count)
    }

    /// Removes for good the boards and blocks that have been in the trash since before
    /// `deleted_before`, of every organization the handle reaches, and dooms the objects
    /// of their files.
    ///
    /// # Errors
    ///
    /// Returns the store's error if the trash cannot be purged; nothing went then.
    pub async fn purge_trash(&self, deleted_before: i64) -> Result<TrashPurged, AppError> {
        let purged = self.store().purge_trash(deleted_before).await?;
        Ok(TrashPurged {
            records: purged.records,
            files: purged.files.len(),
            failed: 0,
        })
    }

    /// Removes the records of the files that no block, live or in the trash, has shown
    /// since before `before`, and dooms their objects. Returns how many went.
    ///
    /// # Errors
    ///
    /// Returns the store's error if the files cannot be collected.
    pub async fn collect_unreferenced_files(&self, before: i64) -> Result<usize, AppError> {
        Ok(self.store().collect_unreferenced_files(before).await?.len())
    }

    /// Closes the connections of the database, waiting for those in use to be given back.
    pub async fn close(&self) {
        self.store().close().await;
    }

    /// Removes what the temporary directories hold that nothing will use: the parts of
    /// sessions that are gone, the directories of completions that were cut off, the
    /// archives being exported, and the files the local storage was writing. Only for
    /// when no request is under way, as at the start, and only by the process that holds
    /// the lock of the server, with one server to the files directory and the
    /// temporary directory (`docs/configuration.md`). Returns how many it removed.
    ///
    /// # Errors
    ///
    /// Returns a 503 error if this process does not hold the lock of the server, and
    /// the store's error if the sessions cannot be read.
    pub async fn sweep_temporary_files(&self) -> Result<usize, AppError> {
        self.require_server_lock()?;
        let sessions: std::collections::HashSet<String> =
            self.store().upload_ids().await?.into_iter().collect();
        let mut removed = 0;
        let uploads = self.config().tmp_dir().join("uploads");
        for (name, path) in entries(&uploads).await {
            // A session's directory is named by its identifier; anything else, such as a
            // directory a completion moved aside, is left over.
            self.require_server_lock()?;
            if !sessions.contains(&name) && remove_entry(&path).await {
                removed += 1;
            }
        }
        // Archives being written or sent when the server stopped.
        let exports = self.config().tmp_dir().join(super::export::EXPORTS_DIR);
        for (_, path) in entries(&exports).await {
            self.require_server_lock()?;
            if remove_entry(&path).await {
                removed += 1;
            }
        }
        if self.config().file_storage == "local" {
            let writing = Path::new(&self.config().file_storage_path).join(WRITING_DIR);
            for (_, path) in entries(&writing).await {
                self.require_server_lock()?;
                if remove_entry(&path).await {
                    removed += 1;
                }
            }
        }
        Ok(removed)
    }

    /// Creates `review_due` notifications for pending, un-notified reviews whose
    /// `scheduled_date` has reached each learner's local calendar day at `now`, marking
    /// `notified_at = now` atomically so each review notifies its owner at most once.
    ///
    /// Returns the number of notifications created. Runs under housekeeping's system
    /// scope and requires the server lock.
    ///
    /// # Errors
    ///
    /// Returns an error if the server lock is not held or a database query fails.
    pub async fn notify_due_reviews(&self, now: i64) -> Result<u64, AppError> {
        let app = self.housekeeping();
        app.require_server_lock()?;
        let utc_day = day_at(now, &TimeZone::default())
            .ok_or_else(|| AppError::internal("current timestamp is outside supported years"))?;
        let max_date = add_days(&utc_day, 1).unwrap_or(utc_day);

        let mut user_days: HashMap<String, String> = HashMap::new();
        let mut cursor: Option<(String, i64, String)> = None;
        let mut sent = 0_u64;

        for _ in 0..REVIEW_NOTIFY_MAX_ROUNDS {
            let after = cursor
                .as_ref()
                .map(|(date, created_at, id)| (date.as_str(), *created_at, id.as_str()));
            let batch = app
                .store()
                .due_unnotified_reviews(&max_date, after, REVIEW_NOTIFY_ROUND)
                .await?;
            if batch.is_empty() {
                break;
            }
            let reached_limit =
                i64::try_from(batch.len()).unwrap_or(i64::MAX) >= REVIEW_NOTIFY_ROUND;
            for candidate in &batch {
                let user_today = if let Some(day) = user_days.get(&candidate.user_id) {
                    day.as_str()
                } else {
                    let computed = app.user_today(&candidate.user_id, now).await?;
                    user_days
                        .entry(candidate.user_id.clone())
                        .or_insert(computed)
                        .as_str()
                };
                if candidate.scheduled_date.as_str() > user_today {
                    continue;
                }
                let enrolled = app
                    .in_organization(&candidate.org_id)?
                    .in_workspace(&candidate.workspace_id)
                    .reaching(ContainerKinds::COURSES)
                    .container_access(&candidate.user_id, &candidate.container_id)
                    .await?
                    .is_some_and(|access| {
                        access.has_membership()
                            && access.grant.reads()
                            && !access.container.is_template
                    });
                if !enrolled {
                    continue;
                }
                if app
                    .store()
                    .notify_due_review(candidate, now, MOST_NOTIFICATIONS_PER_USER)
                    .await?
                {
                    sent += 1;
                    app.ws()
                        .notifications_changed(&candidate.user_id, &[&candidate.org_id])
                        .await;
                }
            }
            if !reached_limit {
                break;
            }
            if let Some(last) = batch.last() {
                cursor = Some((
                    last.scheduled_date.clone(),
                    last.created_at,
                    last.id.clone(),
                ));
            }
        }
        Ok(sent)
    }

    /// Housekeeping entry point for [`App::notify_due_reviews`]: logs failures and leaves
    /// remaining work for the next run.
    pub async fn notify_due_reviews_upkeep(&self) {
        if self.config().notifications_by_hand || !self.holds_server_lock() {
            return;
        }
        match self.notify_due_reviews(now_millis()).await {
            Ok(0) => {}
            Ok(sent) => tracing::info!(sent, "sent due unit review notifications"),
            Err(error) => tracing::error!(%error, "unable to send due unit review notifications"),
        }
    }

    /// Creates `due_soon` notifications for incomplete assigned cards due today, due
    /// tomorrow, or overdue in each assignee's local calendar day (`App::user_today`),
    /// across both shared organizations and personal spaces (`OrgKind::Personal`),
    /// deduplicating via `card_due_notices(org_id, card_id, user_id, due_key)`.
    ///
    /// # Errors
    ///
    /// Returns an error if the server lock is not held or a database query fails.
    pub async fn notify_due_soon_cards(&self, now: i64) -> Result<u64, AppError> {
        let app = self.housekeeping();
        app.require_server_lock()?;
        let utc_day = day_at(now, &TimeZone::default())
            .ok_or_else(|| AppError::internal("current timestamp is outside supported years"))?;
        let max_due_at = add_days(&utc_day, 2).unwrap_or(utc_day);

        let mut cursor: Option<(String, String, String)> = None;
        let mut sent = 0_u64;

        for _ in 0..REVIEW_NOTIFY_MAX_ROUNDS {
            let after = cursor
                .as_ref()
                .map(|(due_at, card, user)| (due_at.as_str(), card.as_str(), user.as_str()));
            let batch = app
                .store()
                .due_soon_card_candidates(&max_due_at, after, REVIEW_NOTIFY_ROUND)
                .await?;
            let Some(last) = batch.last() else {
                break;
            };
            // The next round starts after the last one asked of: who is passed over now
            // (not yet due in their own day, or not reading the board) holds up no one.
            cursor = Some((
                last.due_at.clone(),
                last.card_id.clone(),
                last.user_id.clone(),
            ));
            let reached_limit =
                i64::try_from(batch.len()).unwrap_or(i64::MAX) >= REVIEW_NOTIFY_ROUND;
            for candidate in &batch {
                if app.notify_due_soon_card(candidate, now).await? {
                    sent += 1;
                }
            }
            if !reached_limit {
                break;
            }
        }
        Ok(sent)
    }

    /// Tells `candidate.user_id` that the card is due soon, if it is due by tomorrow in
    /// their own day (the zone the candidate carries) and they read its board. Whether
    /// they were told.
    async fn notify_due_soon_card(
        &self,
        candidate: &DueSoonCardCandidate,
        now: i64,
    ) -> Result<bool, AppError> {
        let tz: TimeZone = candidate.user_time_zone.parse().unwrap_or_default();
        let today = day_at(now, &tz)
            .or_else(|| day_at(now, &TimeZone::default()))
            .ok_or_else(|| AppError::internal("current timestamp is outside supported years"))?;
        let soon_day = add_days(&today, 1).unwrap_or(today);
        let effective_due = match candidate.due_millis {
            Some(ms) => day_at(ms, &tz)
                .or_else(|| day_at(ms, &TimeZone::default()))
                .unwrap_or_else(|| candidate.due_at.clone()),
            None => candidate.due_at.clone(),
        };
        if effective_due > soon_day {
            return Ok(false);
        }
        let readable = self
            .in_organization(&candidate.org_id)?
            .in_workspace(&candidate.workspace_id)
            .reaching(ContainerKinds::BOARDS)
            .container_access(&candidate.user_id, &candidate.board_id)
            .await?
            .is_some_and(|access| access.grant.reads_on_joining() && !access.container.is_template);
        if !readable
            || !self
                .store()
                .notify_due_soon_card(candidate, now, MOST_NOTIFICATIONS_PER_USER)
                .await?
        {
            return Ok(false);
        }
        self.ws()
            .notifications_changed(&candidate.user_id, &[&candidate.org_id])
            .await;
        Ok(true)
    }

    /// Housekeeping entry point for [`App::notify_due_soon_cards`].
    pub async fn notify_due_soon_cards_upkeep(&self) {
        if self.config().notifications_by_hand || !self.holds_server_lock() {
            return;
        }
        match self.notify_due_soon_cards(now_millis()).await {
            Ok(0) => {}
            Ok(sent) => tracing::info!(sent, "sent due-soon card notifications"),
            Err(error) => tracing::error!(%error, "unable to send due-soon card notifications"),
        }
    }
}

/// The entries of a directory with their names; none if it cannot be read.
async fn entries(directory: &Path) -> Vec<(String, PathBuf)> {
    let mut found = Vec::new();
    let mut listing = match fs::read_dir(directory).await {
        Ok(listing) => listing,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return found,
        Err(error) => {
            tracing::warn!(%error, "a temporary directory could not be read");
            return found;
        }
    };
    loop {
        match listing.next_entry().await {
            Ok(Some(entry)) => {
                found.push((
                    entry.file_name().to_string_lossy().into_owned(),
                    entry.path(),
                ));
            }
            Ok(None) => break,
            Err(error) => {
                tracing::warn!(%error, "a temporary directory could not be read to its end");
                break;
            }
        }
    }
    found
}

/// Removes a file or a directory that is left over, with what is in it. Returns whether
/// it was removed.
async fn remove_entry(path: &Path) -> bool {
    let removed = match fs::symlink_metadata(path).await {
        Ok(metadata) if metadata.is_dir() => fs::remove_dir_all(path).await,
        Ok(_) => fs::remove_file(path).await,
        Err(error) => Err(error),
    };
    match removed {
        Ok(()) => true,
        Err(error) if error.kind() == io::ErrorKind::NotFound => false,
        Err(error) => {
            tracing::warn!(%error, "a left-over temporary file could not be removed");
            false
        }
    }
}

/// What a purge of the trash came to.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TrashPurged {
    /// The boards and blocks that went for good.
    pub records: u64,
    /// The files whose objects were doomed.
    pub files: usize,
    /// The workspaces that could not be removed, left for the next run.
    pub failed: usize,
}
