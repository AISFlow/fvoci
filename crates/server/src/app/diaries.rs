//! The diaries of a user's own space: making one, the entry of a day, the days that
//! are written, and the trash of a diary's entries. An entry is a block, and is changed
//! and deleted as one ([`App::patch_block`], [`App::delete_block`]); its day is not
//! changed at all.

use fvoci_model::{
    Block, BlockType, Container, DeletedDiaryEntry, DiaryDate, DiaryDay, DiaryEntry,
    DiaryEntryPatch, DiaryMaking, DiaryRun, ENTRIES_PER_DIARY, MonthDay, NewDiaryEntry, new_id,
};
use fvoci_store::DiaryMade;

use super::App;
use super::blocks::{ChangeAction, ChangeNotification, log_notification};
use super::list_page::ListPage;
use crate::collab::Deletion;
use crate::error::AppError;

impl App {
    /// Makes a diary in its workspace with `user_id` as its admin, as `making` says,
    /// tells websocket listeners of one that was made, and files it under the user's
    /// default category, as a board that is made. Where the space has a diary and only
    /// its first was asked for, that one is answered and nothing is done.
    ///
    /// # Errors
    ///
    /// Returns a bad request if the workspace is not of a user's own space, the space
    /// has as many diaries as it may (the reason `too_many_diaries`), or the diary may
    /// not be stored; and a not-found error if there is no such workspace.
    pub async fn create_diary(
        &self,
        mut diary: Container,
        user_id: &str,
        making: DiaryMaking,
    ) -> Result<DiaryMade, AppError> {
        self.require_makes(&diary)?;
        diary.id = new_id();
        let made = self.store().create_diary(diary, user_id, making).await?;
        let Some(creator) = made.made.clone() else {
            return Ok(made);
        };
        let app = self.clone();
        let created = made.diary.clone();
        self.spawn_background(async move {
            app.ws()
                .broadcast_container_change(&created.workspace_id, &created)
                .await;
            app.ws()
                .broadcast_member_change(&created.workspace_id, &created.id, &creator)
                .await;
        });
        self.add_containers_to_default_category(
            user_id,
            &made.diary.workspace_id,
            std::slice::from_ref(&made.diary.id),
        )
        .await?;
        Ok(made)
    }

    /// The entry of the diary, its block read once: a block that is no entry of this
    /// diary is no entry, whatever else it is, and one in the trash is not there.
    /// Whether the asker may read the diary is the caller's to have asked.
    ///
    /// # Errors
    ///
    /// Returns a not-found error for anything but a live entry of the diary, and the
    /// store's error if the block cannot be read.
    pub async fn diary_entry_of(&self, diary_id: &str, entry_id: &str) -> Result<Block, AppError> {
        const WHAT: &str = "entry";
        match self.block(entry_id).await {
            Ok(block) if block.container_id == diary_id && block.block_type == BlockType::Entry => {
                Ok(block)
            }
            Ok(_) => Err(AppError::not_found(WHAT)),
            Err(error) if error.is_not_found() => Err(AppError::not_found(WHAT)),
            Err(error) => Err(error),
        }
    }

    /// Makes the entry of the day `date` of the diary as `new` says and tells websocket
    /// listeners of it, as of any block that is written; where the day has an entry,
    /// that one is answered as it is and nothing is told. Says whether it made one.
    ///
    /// # Errors
    ///
    /// Returns a not-found error if there is no such diary, and a bad request with the
    /// reason `too_many_entries` if the diary has as many entries as one may.
    pub async fn create_diary_entry(
        &self,
        diary_id: &str,
        date: DiaryDate,
        new: NewDiaryEntry,
        user_id: &str,
    ) -> Result<(DiaryEntry, EntryAnswer), AppError> {
        let diary = self.store().get_container(diary_id).await?;
        let written = {
            let _structure = self.collab().changing_structure(self, diary_id).await;
            self.store()
                .create_diary_entry(diary_id, date, new, ENTRIES_PER_DIARY, user_id)
                .await?
        };
        if !written.made {
            return Ok((DiaryEntry::from(written.entry), EntryAnswer::ThereAlready));
        }
        log_notification(
            ChangeNotification::Send,
            ChangeAction::Add,
            &written.entry.id,
        );
        self.announce_entry(&diary.workspace_id, written.entry.clone());
        Ok((DiaryEntry::from(written.entry), EntryAnswer::Made))
    }

    /// The entries of the diary whose day is `at` in any year, the latest year first,
    /// each with its body.
    ///
    /// # Errors
    ///
    /// Returns the store's error if the entries cannot be read.
    pub async fn diary_entries_on(
        &self,
        diary_id: &str,
        at: MonthDay,
    ) -> Result<Vec<DiaryEntry>, AppError> {
        let entries = self.store().get_diary_entries_on(diary_id, at).await?;
        Ok(entries.into_iter().map(DiaryEntry::from).collect())
    }

    /// The days of the diary that have an entry, from `from` to `to` (each included),
    /// the earliest first. Both are asked for: nothing reads the days of a whole diary.
    ///
    /// # Errors
    ///
    /// Returns the store's error if the days cannot be read.
    pub async fn diary_days(
        &self,
        diary_id: &str,
        from: DiaryDate,
        to: DiaryDate,
    ) -> Result<Vec<DiaryDay>, AppError> {
        let days = self.store().get_diary_days(diary_id, Some(from), Some(to));
        Ok(days.await?)
    }

    /// The runs of the diary's written days, the earliest first: of every day of the
    /// diary, the day alone, for the client to count the days written in a row.
    ///
    /// # Errors
    ///
    /// Returns the store's error if the days cannot be read.
    pub async fn diary_runs(&self, diary_id: &str) -> Result<Vec<DiaryRun>, AppError> {
        Ok(self.store().get_diary_runs(diary_id).await?)
    }

    /// Changes an entry of `diary_id`, coordinating with its collaborative room when
    /// `patch` replaces, clears, or appends to the body, and announces the updated
    /// entry to websocket listeners and webhooks in the background.
    ///
    /// # Errors
    ///
    /// Returns a not-found error if the diary or entry does not exist, a bad request if
    /// the patch or combined appended body fails validation or body limits, a conflict
    /// if `readAt` predates the last body change, or `document_unavailable` if pending
    /// room edits cannot be flushed first.
    pub async fn patch_diary_entry(
        &self,
        diary_id: &str,
        entry_id: &str,
        patch: &DiaryEntryPatch,
        modified_by: &str,
        notification: ChangeNotification,
    ) -> Result<Block, AppError> {
        let entry = self.diary_entry_of(diary_id, entry_id).await?;
        let container = self.store().get_container(&entry.container_id).await?;
        let body_field = if patch.append_body.is_some() {
            "appendBody"
        } else {
            "body"
        };
        let patched = if patch.has_body_write() {
            let (diary_id, entry_id, patch, modified_by) = (
                diary_id.to_owned(),
                entry_id.to_owned(),
                patch.clone(),
                modified_by.to_owned(),
            );
            let cards = vec![(entry_id.clone(), diary_id.clone())];
            self.collab()
                .write_whole(self, cards, async move |app| {
                    app.store()
                        .patch_diary_entry(&diary_id, &entry_id, &patch, &modified_by)
                        .await
                        .map_err(|error| map_entry_patch_error(error, body_field))
                })
                .await?
        } else {
            let _structure = self.collab().changing_structure(self, diary_id).await;
            self.store()
                .patch_diary_entry(diary_id, entry_id, patch, modified_by)
                .await
                .map_err(|error| map_entry_patch_error(error, body_field))?
        };
        self.tell_of_update(&container.workspace_id, &patched, notification);
        Ok(patched)
    }

    /// Deletes an entry of `diary_id`: flushes any open collaborative room before
    /// deciding whether the entry is completely empty (purged without entering the
    /// trash) or carries content/notes (moved to the diary's trash), closes its room,
    /// and notifies websocket listeners in the background.
    ///
    /// # Errors
    ///
    /// Returns a not-found error if the entry does not exist on `diary_id`, or the
    /// store's error.
    pub async fn delete_diary_entry(
        &self,
        diary_id: &str,
        entry_id: &str,
        modified_by: &str,
        notification: ChangeNotification,
    ) -> Result<(), AppError> {
        let entry = self.diary_entry_of(diary_id, entry_id).await?;
        let container = self.store().get_container(&entry.container_id).await?;
        let (diary_id, removed, by) = (
            diary_id.to_owned(),
            entry_id.to_owned(),
            modified_by.to_owned(),
        );
        self.collab()
            .remove(self, Deletion::Block(removed.clone()), async move |app| {
                Ok(app
                    .store()
                    .delete_diary_entry(&diary_id, &removed, &by)
                    .await?)
            })
            .await?;
        log_notification(notification, ChangeAction::Delete, entry_id);
        let app = self.clone();
        self.spawn_background(async move {
            app.ws()
                .broadcast_block_delete(
                    &container.workspace_id,
                    &entry.id,
                    &entry.container_id,
                    BlockType::Entry,
                )
                .await;
        });
        Ok(())
    }

    /// The diary's entries in the trash, one page of the list ([`ListPage`]).
    ///
    /// # Errors
    ///
    /// Returns the store's error if the trash cannot be read.
    pub async fn deleted_diary_entries(
        &self,
        diary_id: &str,
        page: ListPage,
    ) -> Result<Vec<DeletedDiaryEntry>, AppError> {
        Ok(self
            .store()
            .get_deleted_diary_entries(diary_id, Some(page.paging()))
            .await?)
    }

    /// Brings an entry of the diary back from the trash to its day, and tells websocket
    /// listeners of it.
    ///
    /// # Errors
    ///
    /// Returns a not-found error if the diary's trash has no such entry, a conflict
    /// with the reason `day_taken` if the day has an entry since, and a bad request
    /// with the reason `too_many_entries` if the diary has as many entries as one may.
    pub async fn undelete_diary_entry(
        &self,
        diary_id: &str,
        entry_id: &str,
        user_id: &str,
    ) -> Result<DiaryEntry, AppError> {
        let diary = self.store().get_container(diary_id).await?;
        let restored = {
            let _structure = self.collab().changing_structure(self, diary_id).await;
            self.store()
                .undelete_diary_entry(diary_id, entry_id, ENTRIES_PER_DIARY, user_id)
                .await
        };
        let entry = restored.map_err(|error| {
            if error.is_not_found() {
                AppError::not_found("entry")
            } else {
                AppError::from(error)
            }
        })?;
        log_notification(ChangeNotification::Send, ChangeAction::Add, entry_id);
        self.announce_entry(&diary.workspace_id, entry.clone());
        Ok(DiaryEntry::from(entry))
    }

    /// Tells websocket listeners in the background of the entry. Nothing of a user's own
    /// space goes to the operator's webhooks ([`App::tell_webhooks`]), which is asked
    /// all the same: it is the one door to them.
    fn announce_entry(&self, workspace_id: &str, entry: Block) {
        let app = self.clone();
        let workspace_id = workspace_id.to_owned();
        self.spawn_background(async move {
            app.ws().broadcast_block_change(&workspace_id, &entry).await;
            app.tell_webhooks(&entry);
        });
    }
}

/// Maps `BodyFileNotOfContainer` to `body_field` (`"body"` or `"appendBody"`).
fn map_entry_patch_error(error: fvoci_store::StoreError, body_field: &'static str) -> AppError {
    if matches!(
        &error,
        fvoci_store::StoreError::InvalidBlock {
            source: fvoci_model::BlockError::BodyFileNotOfContainer,
            ..
        }
    ) {
        return AppError::bad_request(fvoci_model::BlockError::BodyFileNotOfContainer)
            .with_field(body_field);
    }
    AppError::from(error)
}

/// What a request for the entry of a day was answered with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryAnswer {
    /// The entry was made for the request.
    Made,
    /// The day had an entry, which is answered as it is.
    ThereAlready,
}
