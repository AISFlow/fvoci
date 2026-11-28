//! The diaries of a user's own space: making one, the entry of a day, the days that
//! are written, and the trash of a diary's entries. An entry is a block, and is changed
//! and deleted as one (`App::patch_block`, `App::delete_block`); its day is not
//! changed at all.

use fvoci_model::{
    Block, BlockError, BlockType, Container, DeletedDiaryEntry, DiaryDate, DiaryDay, DiaryEntry,
    DiaryEntryPatch, DiaryMaking, DiaryRun, ENTRIES_PER_DIARY, MonthDay, NewDiaryEntry, new_id,
};
use fvoci_store::DiaryMade;

use super::blocks::{ChangeAction, ChangeNotification, log_notification};
use super::list_page::ListPage;
use super::{App, Changes, Reached};
use crate::collab::Deletion;
use crate::error::{AppError, OrMissing};
use crate::permissions::ContainerPermission;

/// What who may not write in the diary is refused making an entry with.
pub const CREATE_ENTRY_DENIED: &str = "access denied to create entry";
/// What who may not write in the diary is refused changing an entry with.
pub const PATCH_ENTRY_DENIED: &str = "access denied to patch entry";
/// What who may not write in the diary is refused deleting an entry with.
pub const DELETE_ENTRY_DENIED: &str = "access denied to delete entry";
/// What who may not write in the diary is refused its trash with.
pub const ENTRY_TRASH_DENIED: &str = "access denied to the trash of the diary";
/// What who may not write in the diary is refused bringing an entry back with.
pub const UNDELETE_ENTRY_DENIED: &str = "access denied to undelete entry";

/// Checks that the caller may write in the diary `on` reached, refusing in `denial`,
/// and that `entry`, which the request found by its identifier, is an entry of it.
fn writing_entry<'on>(
    on: &'on Reached,
    entry: &Block,
    denial: &str,
) -> Result<&'on Container, AppError> {
    let diary = on.require(ContainerPermission::ManageCards, denial)?;
    if on.holds(entry) && entry.block_type == BlockType::Entry {
        Ok(diary)
    } else {
        Err(AppError::not_found("entry"))
    }
}

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
        let block = self.block(entry_id).await.or_missing(WHAT)?;
        if block.container_id == diary_id && block.block_type == BlockType::Entry {
            Ok(block)
        } else {
            Err(AppError::not_found(WHAT))
        }
    }

    /// Makes the entry of the day `date` of the diary `on` reached as `new` says, for who
    /// may write in it, and tells websocket listeners of it, as of any block that is
    /// written; where the day has an entry, that one is answered as it is and nothing is
    /// told. Says whether it made one.
    ///
    /// # Errors
    ///
    /// Returns [`CREATE_ENTRY_DENIED`] as [`Reached::require`] does, a not-found error if
    /// there is no such diary, and a bad request with the reason `too_many_entries` if
    /// the diary has as many entries as one may.
    pub async fn create_diary_entry(
        &self,
        on: &Reached,
        date: DiaryDate,
        new: NewDiaryEntry,
    ) -> Result<(DiaryEntry, EntryAnswer), AppError> {
        let diary = on.require(ContainerPermission::ManageCards, CREATE_ENTRY_DENIED)?;
        let (diary_id, user_id) = (on.container_id(), on.user_id());
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
        self.tell_of_entry(&diary.workspace_id, written.entry.clone());
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

    /// Changes an entry of the diary `on` reached, for who may write in it, coordinating
    /// with its collaborative room when `patch` replaces, clears, or appends to the
    /// body, and announces the updated entry to websocket listeners and webhooks in the
    /// background.
    ///
    /// # Errors
    ///
    /// Returns [`PATCH_ENTRY_DENIED`] as [`Reached::require`] does, a not-found error if
    /// the block is no entry of the diary, a bad request if the patch or combined
    /// appended body fails validation or body limits or names a file the diary does not
    /// have, a conflict if `readAt` predates the last body change, or
    /// `document_unavailable` if pending room edits cannot be flushed first.
    pub async fn patch_diary_entry(
        &self,
        on: &Reached,
        entry: &Block,
        patch: &DiaryEntryPatch,
        notification: ChangeNotification,
    ) -> Result<Block, AppError> {
        let container = writing_entry(on, entry, PATCH_ENTRY_DENIED)?;
        patch.check_request()?;
        let (diary_id, entry_id, modified_by) =
            (on.container_id(), entry.id.as_str(), on.user_id());
        if let Some(body) = patch.body.new_body() {
            self.check_body_files(diary_id, Some(body)).await?;
        } else if let Some(extra) = patch.append_body.as_ref()
            && !self.has_body_files(diary_id, Some(extra)).await?
        {
            return Err(
                AppError::bad_request(BlockError::BodyFileNotOfContainer).with_field("appendBody")
            );
        }
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
        log_notification(notification, ChangeAction::Update, &patched.id);
        self.tell(Changes::in_workspace(&container.workspace_id).changed([patched.clone()]));
        Ok(patched)
    }

    /// Deletes an entry of the diary `on` reached, for who may write in it: flushes any
    /// open collaborative room before deciding whether the entry is completely empty
    /// (purged without entering the trash) or carries content/notes (moved to the
    /// diary's trash), closes its room, and notifies websocket listeners in the
    /// background.
    ///
    /// # Errors
    ///
    /// Returns [`DELETE_ENTRY_DENIED`] as [`Reached::require`] does, a not-found error if
    /// the block is no entry of the diary, or the store's error.
    pub async fn delete_diary_entry(
        &self,
        on: &Reached,
        entry: &Block,
        notification: ChangeNotification,
    ) -> Result<(), AppError> {
        let workspace_id = writing_entry(on, entry, DELETE_ENTRY_DENIED)?
            .workspace_id
            .clone();
        let entry_id = entry.id.as_str();
        let (diary_id, removed, by) = (
            on.container_id().to_owned(),
            entry_id.to_owned(),
            on.user_id().to_owned(),
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
        self.tell(Changes::in_workspace(&workspace_id).deleted([entry.clone()]));
        Ok(())
    }

    /// The entries in the trash of the diary `on` reached, for who may write in it, one
    /// page of the list ([`ListPage`]).
    ///
    /// # Errors
    ///
    /// Returns [`ENTRY_TRASH_DENIED`] as [`Reached::require`] does, and the store's error
    /// if the trash cannot be read.
    pub async fn deleted_diary_entries(
        &self,
        on: &Reached,
        page: ListPage,
    ) -> Result<Vec<DeletedDiaryEntry>, AppError> {
        on.require(ContainerPermission::ManageCards, ENTRY_TRASH_DENIED)?;
        Ok(self
            .store()
            .get_deleted_diary_entries(on.container_id(), Some(page.paging()))
            .await?)
    }

    /// Brings an entry of the diary `on` reached back from the trash to its day, for who
    /// may write in it, and tells websocket listeners of it.
    ///
    /// # Errors
    ///
    /// Returns [`UNDELETE_ENTRY_DENIED`] as [`Reached::require`] does, a not-found error
    /// if the diary's trash has no such entry, a conflict with the reason `day_taken` if
    /// the day has an entry since, and a bad request with the reason `too_many_entries`
    /// if the diary has as many entries as one may.
    pub async fn undelete_diary_entry(
        &self,
        on: &Reached,
        entry_id: &str,
    ) -> Result<DiaryEntry, AppError> {
        let diary = on.require(ContainerPermission::ManageCards, UNDELETE_ENTRY_DENIED)?;
        let (diary_id, user_id) = (on.container_id(), on.user_id());
        let restored = {
            let _structure = self.collab().changing_structure(self, diary_id).await;
            self.store()
                .undelete_diary_entry(diary_id, entry_id, ENTRIES_PER_DIARY, user_id)
                .await
        };
        let entry = restored.or_missing("entry")?;
        log_notification(ChangeNotification::Send, ChangeAction::Add, entry_id);
        self.tell_of_entry(&diary.workspace_id, entry.clone());
        Ok(DiaryEntry::from(entry))
    }

    /// Tells websocket listeners in the background of the entry. Nothing of a user's own
    /// space goes to the operator's webhooks ([`App::tell_webhooks`]), which are asked
    /// all the same: they are told as of any block.
    fn tell_of_entry(&self, workspace_id: &str, entry: Block) {
        self.tell(Changes::in_workspace(workspace_id).changed([entry]));
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
