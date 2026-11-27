//! Whether an account was given its welcome board. It is given one once, whatever
//! organizations it comes into: the record is the account's, not an organization's,
//! and is made with the board, in the transaction that stores it.

use fvoci_model::{
    ContainerMember, ContainersAndBlocks, DeleteContainersAndBlocks, DuplicateAs, FileRecord,
};

use crate::blocks::{CopyChanges, CopyTitles, insert_blocks};
use crate::containers;
use crate::containers_and_blocks::{delete_containers_and_blocks, duplicate_container};
use crate::db::{Id, LockRank, Scope};
use crate::file_refs::NewBodyFiles;
use crate::files;
use crate::loose_objects;
use crate::members;
use crate::notification_noting::Telling;
use crate::{Store, StoreError};

impl Store {
    /// Copies the welcome board `template_id` into the workspace as a private board of
    /// the user's ([`Store::duplicate_container`], with `titles` and `changes`) and
    /// records at `now` that their account is given it, in one transaction: an account
    /// that was given one before gets `None` and nothing is copied, of calls made at
    /// once one copies, and a copy that fails records nothing.
    ///
    /// # Errors
    ///
    /// As [`Store::duplicate_container`].
    pub async fn duplicate_welcome_board(
        &self,
        template_id: &str,
        user_id: &str,
        workspace_id: &str,
        titles: &CopyTitles,
        changes: CopyChanges<'_>,
        now: i64,
    ) -> Result<Option<(ContainersAndBlocks, Vec<ContainerMember>)>, StoreError> {
        self.transaction(async |scope| {
            // First: the calls made for an account at once wait here for one another.
            if !claim(scope, user_id, now).await? {
                return Ok(None);
            }
            duplicate_container(
                scope,
                template_id,
                user_id,
                Some(workspace_id),
                DuplicateAs::Copy,
                titles,
                changes,
            )
            .await
            .map(Some)
        })
        .await
    }

    /// Records at `now` that the account has its welcome board, whatever was recorded:
    /// for a board made for whoever asked, which is made whatever is recorded. The
    /// record is this board's from here on, and no call that recorded before takes it
    /// back with a board of its own ([`Store::take_back_welcome_board`]).
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the statement fails.
    pub async fn record_welcome(&self, user_id: &str, now: i64) -> Result<(), StoreError> {
        self.with_write_conn(async |scope| {
            let mut sql = scope.sql("UPDATE users SET ");
            sql.push_bind("welcomed_at = ", now)
                .push_bind(" WHERE id = ", Id(user_id));
            scope.execute(sql).await?;
            Ok(())
        })
        .await
    }

    /// Takes back a welcome board that [`Store::duplicate_welcome_board`] stored and
    /// recorded at `recorded_at`: deletes `copies` as
    /// [`Store::delete_containers_and_blocks`] does and takes back the record, in one
    /// transaction, so that the account is given its board the next time. A record
    /// made at another time is another board's, and stays.
    ///
    /// # Errors
    ///
    /// As [`Store::delete_containers_and_blocks`]; neither is taken back then.
    pub async fn take_back_welcome_board(
        &self,
        copies: &DeleteContainersAndBlocks,
        user_id: &str,
        recorded_at: i64,
    ) -> Result<(), StoreError> {
        self.transaction(async |scope| {
            delete_containers_and_blocks(scope, copies, user_id).await?;
            scope.record_lock(LockRank::Users, user_id);
            let mut sql = scope.sql("UPDATE users SET welcomed_at = NULL");
            sql.push_bind(" WHERE id = ", Id(user_id))
                .push_bind(" AND welcomed_at = ", recorded_at);
            scope.execute(sql).await?;
            Ok(())
        })
        .await
    }

    /// Whether `workspace_id` in this handle's organization has already received its
    /// welcome board (`workspaces.welcomed_at IS NOT NULL`).
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NoOrganization`] if the handle names none, and
    /// [`StoreError::Database`] if the query fails.
    pub async fn is_workspace_welcomed(&self, workspace_id: &str) -> Result<bool, StoreError> {
        self.with_conn(async |scope| {
            let mut sql = scope.sql("SELECT welcomed_at IS NOT NULL FROM workspaces WHERE id = ");
            sql.bind(Id(workspace_id))
                .push_bind(" AND org_id = ", scope.org()?)
                .push(" AND deleted_at IS NULL");
            match scope.fetch_optional(sql).await? {
                Some(row) => row.bool(0),
                None => Ok(true),
            }
        })
        .await
    }

    /// Marks `workspace_id` in this handle's organization as having received its welcome
    /// board at `now`.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NoOrganization`] if the handle names none, and
    /// [`StoreError::Database`] if the statement fails.
    pub async fn record_workspace_welcome(
        &self,
        workspace_id: &str,
        now: i64,
    ) -> Result<(), StoreError> {
        let stamp = now.max(1);
        self.with_write_conn(async |scope| {
            let mut sql = scope.sql("UPDATE workspaces SET welcomed_at = ");
            sql.changing_no_access()
                .bind(stamp)
                .push_bind(" WHERE id = ", Id(workspace_id))
                .push_bind(" AND org_id = ", scope.org()?);
            scope.execute(sql).await?;
            Ok(())
        })
        .await
    }

    /// Atomically claims `workspaces.welcomed_at` on `workspace_id` and commits the
    /// welcome board (`copies`) along with its pre-copied `files` and `user_id`'s
    /// administrator membership in a single transaction.
    ///
    /// If `workspace_id` was already welcomed (or does not exist in use), any pre-copied
    /// `files` are doomed for housekeeping cleanup and `Ok(None)` is returned.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::InvalidPath`] for a malformed file path,
    /// [`StoreError::NoOrganization`] if the handle names none, or any validation /
    /// database error; nothing is committed on error.
    pub async fn commit_workspace_welcome_board(
        &self,
        workspace_id: &str,
        copies: &mut ContainersAndBlocks,
        files: &[FileRecord],
        user_id: &str,
        now: i64,
    ) -> Result<Option<Vec<ContainerMember>>, StoreError> {
        for file in files {
            files::check_new_path(&file.path)?;
        }
        let stamp = now.max(1);
        self.transaction(async |scope| {
            scope.record_lock(LockRank::Workspace, workspace_id);
            let mut claim_ws = scope.sql("UPDATE workspaces SET welcomed_at = ");
            claim_ws
                .changing_no_access()
                .bind(stamp)
                .push_bind(" WHERE id = ", Id(workspace_id))
                .push_bind(" AND org_id = ", scope.org()?)
                .push(" AND deleted_at IS NULL AND welcomed_at IS NULL");
            if scope.execute(claim_ws).await? == 0 {
                if !files.is_empty() {
                    let doomed: Vec<_> =
                        files.iter().map(|file| (file.path.clone(), None)).collect();
                    loose_objects::doom(scope, &doomed).await?;
                }
                return Ok(None);
            }
            containers::lock_containers(scope, copies.containers.iter().map(|c| c.id.as_str()))
                .await?;
            for container in &mut copies.containers {
                containers::insert_container(scope, container, user_id).await?;
            }
            for file in files {
                if !loose_objects::record_written(scope, file).await? {
                    return Err(StoreError::NotFound(format!(
                        "mark of the welcome file {}",
                        file.path
                    )));
                }
            }
            insert_blocks(
                scope,
                &mut copies.blocks,
                user_id,
                NewBodyFiles::Recorded,
                Telling::Nobody,
            )
            .await?;
            let mut created_members = Vec::with_capacity(copies.containers.len());
            for container in &copies.containers {
                members::hold_creator(scope, &container.workspace_id, &container.created_by)
                    .await?;
                let member = ContainerMember::new(
                    &container.id,
                    &container.created_by,
                    fvoci_model::ContainerRole::Admin,
                );
                containers::save_member(scope, &member).await?;
                created_members.push(member);
            }
            Ok(Some(created_members))
        })
        .await
    }
}

/// Records at `now` that the account is given its welcome board, unless it was given
/// one before. Returns whether this recorded it: one statement, so that one of those
/// made at once does.
async fn claim(scope: &mut Scope<'_>, user_id: &str, now: i64) -> Result<bool, StoreError> {
    let mut sql = scope.sql("UPDATE users SET ");
    sql.push_bind("welcomed_at = ", now)
        .push_bind(" WHERE id = ", Id(user_id))
        .push(" AND welcomed_at IS NULL");
    Ok(scope.execute(sql).await? > 0)
}
