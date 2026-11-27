//! Operations on several boards and blocks at once, all or none.

use std::collections::HashSet;

use fvoci_model::{
    BlockType, Container, ContainerMember, ContainerVisibility, ContainersAndBlocks,
    ContainersAndBlocksError, DeleteContainersAndBlocks, FileRecord, PatchContainersAndBlocks,
    renew_containers_and_blocks_ids,
};

use crate::Store;
use crate::blocks::{self, BlockFilter, CopyChanges, CopyTitles, Written};
use crate::body_versions::BodyWrite;
use crate::card_values;
use crate::container_membership;
use crate::containers;
use crate::db::{Dialect, Scope};
use crate::error::StoreError;
use crate::file_refs::NewBodyFiles;
use crate::files;
use crate::loose_objects;
use crate::members;
use crate::notification_noting::Telling;
use crate::org_admin;
use crate::trash;
use crate::workspace_membership;
use fvoci_model::DuplicateAs;

/// Message sent from a streaming block producer into
/// [`Store::create_containers_and_blocks_streamed`].
///
/// The producer must send [`StreamBatch::Done`] once all batches have been emitted; if the
/// channel closes without [`StreamBatch::Done`] (e.g., because the producer task panicked),
/// the transaction rolls back and returns [`StreamCreateError::Incomplete`].
#[derive(Debug)]
pub enum StreamBatch<E> {
    /// A batch of blocks to validate and insert in this transaction.
    Batch(Vec<fvoci_model::Block>),
    /// Clean end-of-stream marker allowing the transaction to commit.
    Done,
    /// Producer failure; causes an immediate rollback and returns [`StreamCreateError::Producer`].
    Err(E),
}

/// Failure returned by [`Store::create_containers_and_blocks_streamed`].
#[derive(Debug, thiserror::Error)]
pub enum StreamCreateError<E> {
    /// The database transaction or block validation failed.
    #[error(transparent)]
    Store(#[from] StoreError),
    /// The caller's batch producer returned an error; the transaction was rolled back.
    #[error("{0}")]
    Producer(E),
    /// The producer channel closed before sending [`StreamBatch::Done`]; the transaction
    /// was rolled back.
    #[error("block stream ended before signaling completion")]
    Incomplete,
}

enum StreamAbort<E> {
    Producer(E),
    Incomplete,
}

/// The key of the PostgreSQL advisory lock that the replacement of the built-in
/// templates holds. Any number that the migrator's lock does not use.
const BUILTIN_TEMPLATES_LOCK: i64 = 0x6676_6f63_6974_706c;

impl Store {
    /// Replaces the built-in templates with `templates`, stored on behalf of nobody, if
    /// `needed` says the stored ones must go. One transaction that holds the others off
    /// while it reads, decides, removes and stores: of two servers or requests that find
    /// the templates missing at once, the second finds them stored and keeps them.
    ///
    /// `files` are the records of the files the new templates show, whose objects were
    /// written at places marked with [`Store::mark_writing`]: they are recorded with the
    /// templates, which takes their marks away, so that no template is stored whose
    /// bodies name a file that is not there.
    ///
    /// Returns `None` if the templates were kept, or the records of the files the
    /// removed templates had, whose records went with them and whose objects are doomed
    /// in the same transaction.
    ///
    /// # Errors
    ///
    /// Returns the store's error, [`StoreError::InvalidBlock`] for a block that fails
    /// validation, [`StoreError::InvalidPath`] for a file at a place spelled otherwise
    /// than the one way, or [`StoreError::NotFound`] for a file whose place is no longer
    /// marked; nothing is removed or stored then.
    pub async fn replace_builtin_templates(
        &self,
        templates: &mut ContainersAndBlocks,
        files: &[FileRecord],
        needed: impl FnOnce(&[Container]) -> bool + Send,
    ) -> Result<Option<Vec<FileRecord>>, StoreError> {
        for file in files {
            files::check_new_path(&file.path)?;
        }
        self.transaction(async |scope| {
            // On SQLite the transaction holds the database from its start.
            if scope.dialect() == Dialect::Postgres {
                let mut sql = scope.sql("SELECT pg_advisory_xact_lock(");
                sql.bind(BUILTIN_TEMPLATES_LOCK).push(")");
                scope.execute(sql).await?;
            }
            // Under READ COMMITTED, the default, the read below sees what the transaction
            // that held the lock before committed. A stricter isolation level would take
            // its snapshot before the lock is granted, and miss that.
            let stored = containers::get_builtin_templates(scope).await?;
            if !needed(&stored) {
                return Ok(None);
            }
            let mut removed = Vec::new();
            for container in &stored {
                removed.extend(files::files_of_container(scope, &container.id).await?);
            }
            // The records go with the templates; their objects are doomed now.
            let doomed: Vec<_> = removed
                .iter()
                .map(|file| (file.path.clone(), None))
                .collect();
            loose_objects::doom(scope, &doomed).await?;
            containers::remove_builtin_templates(scope).await?;
            // The files their bodies name are recorded just below.
            create_containers_and_blocks(scope, templates, "", NewBodyFiles::Unchecked).await?;
            for file in files {
                if !loose_objects::record_written(scope, file).await? {
                    return Err(StoreError::NotFound(format!(
                        "mark of the template file {}",
                        file.path
                    )));
                }
            }
            Ok(Some(removed))
        })
        .await
    }

    /// Inserts or updates every board and then every block, as [`Store::insert_container`]
    /// and [`Store::insert_block`] do. `containers_and_blocks` is updated to what was stored.
    /// The files the blocks' bodies name newly are asked as `new_files` says.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::InvalidBlock`] for the first block that fails validation;
    /// nothing is written in that case.
    pub async fn create_containers_and_blocks(
        &self,
        containers_and_blocks: &mut ContainersAndBlocks,
        user_id: &str,
        new_files: NewBodyFiles,
    ) -> Result<(), StoreError> {
        self.transaction(async |scope| {
            create_containers_and_blocks(scope, containers_and_blocks, user_id, new_files).await
        })
        .await
    }

    /// Atomically inserts `containers` and streams block batches from `batches` inside
    /// a single database transaction without holding all blocks in memory at once.
    ///
    /// Every block receives the same creation timestamp so ordering by creation time and
    /// identifier remains deterministic. Cross-batch card relation references are verified
    /// after [`StreamBatch::Done`] before committing. Any store error, producer error, or
    /// premature channel close rolls back the entire transaction.
    ///
    /// # Errors
    ///
    /// Returns [`StreamCreateError::Store`] if inserting a container or block fails,
    /// [`StreamCreateError::Producer`] if `batches` yields [`StreamBatch::Err`], or
    /// [`StreamCreateError::Incomplete`] if `batches` closes before [`StreamBatch::Done`].
    pub async fn create_containers_and_blocks_streamed<E>(
        &self,
        containers: &mut [Container],
        user_id: &str,
        new_files: NewBodyFiles,
        batches: tokio::sync::mpsc::Receiver<StreamBatch<E>>,
    ) -> Result<(), StreamCreateError<E>>
    where
        E: Send,
    {
        let mut abort: Option<StreamAbort<E>> = None;
        let outcome = self
            .transaction(async |scope| {
                containers::lock_containers(
                    scope,
                    containers.iter().map(|container| container.id.as_str()),
                )
                .await?;
                for container in containers.iter_mut() {
                    containers::insert_container(scope, container, user_id).await?;
                }
                containers::lock_containers(
                    scope,
                    containers.iter().map(|container| container.id.as_str()),
                )
                .await?;
                stream_block_batches_in_tx(scope, user_id, new_files, batches, &mut abort).await
            })
            .await;
        map_stream_outcome(outcome, abort)
    }

    /// Atomically inserts every container, records every staged file (removing its
    /// `writing` mark), streams all block batches with [`NewBodyFiles::Recorded`], makes
    /// `user_id` an administrator of each container, and adds any `extra_members` not
    /// already present — all inside a single database transaction.
    ///
    /// # Errors
    ///
    /// Returns [`StreamCreateError::Store`] on validation, limit, path, or database
    /// refusal, [`StreamCreateError::Producer`] if `batches` yields [`StreamBatch::Err`],
    /// or [`StreamCreateError::Incomplete`] if `batches` closes before
    /// [`StreamBatch::Done`]. Nothing is committed on error.
    pub async fn create_imported_containers_blocks_files_and_members_streamed<E>(
        &self,
        containers: &mut [Container],
        files: &[FileRecord],
        extra_members: &[ContainerMember],
        user_id: &str,
        batches: tokio::sync::mpsc::Receiver<StreamBatch<E>>,
    ) -> Result<Vec<ContainerMember>, StreamCreateError<E>>
    where
        E: Send,
    {
        self.create_imported_containers_blocks_files_members_and_schedules_streamed(
            containers,
            files,
            extra_members,
            &[],
            user_id,
            batches,
        )
        .await
    }

    /// Same as [`Store::create_imported_containers_blocks_files_and_members_streamed`],
    /// plus atomically upserting every validated `(container_index, schedule)` in
    /// `schedules` inside the same database transaction.
    ///
    /// # Errors
    ///
    /// Returns the errors of
    /// [`Store::create_imported_containers_blocks_files_and_members_streamed`], or
    /// [`StoreError::NotFound`] if a schedule index does not name a course in `containers`.
    pub async fn create_imported_containers_blocks_files_members_and_schedules_streamed<E>(
        &self,
        containers: &mut [Container],
        files: &[FileRecord],
        extra_members: &[ContainerMember],
        schedules: &[(usize, fvoci_model::ValidatedCourseSchedule)],
        user_id: &str,
        batches: tokio::sync::mpsc::Receiver<StreamBatch<E>>,
    ) -> Result<Vec<ContainerMember>, StreamCreateError<E>>
    where
        E: Send,
    {
        for file in files {
            files::check_new_path(&file.path)?;
        }
        let mut abort: Option<StreamAbort<E>> = None;
        let outcome = self
            .transaction(async |scope| {
                containers::lock_containers(
                    scope,
                    containers.iter().map(|container| container.id.as_str()),
                )
                .await?;
                for container in containers.iter_mut() {
                    containers::insert_container(scope, container, user_id).await?;
                }
                containers::lock_containers(
                    scope,
                    containers.iter().map(|container| container.id.as_str()),
                )
                .await?;
                record_imported_files(scope, files).await?;
                stream_block_batches_in_tx(
                    scope,
                    user_id,
                    NewBodyFiles::Recorded,
                    batches,
                    &mut abort,
                )
                .await?;
                insert_streamed_schedules(scope, containers, schedules).await?;
                add_imported_members(scope, containers, extra_members).await
            })
            .await;
        map_stream_outcome(outcome, abort)
    }

    /// Does what [`Store::create_containers_and_blocks`] does and makes the creator of each
    /// board an administrator and editor of it. Returns those memberships, in the order
    /// of the boards.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::InvalidBlock`] for the first block that fails validation,
    /// and [`StoreError::NotFound`] for the workspace if the creator of a board does not
    /// work in its workspace; nothing is written in either case.
    pub async fn create_containers_and_blocks_with_admin(
        &self,
        containers_and_blocks: &mut ContainersAndBlocks,
        user_id: &str,
    ) -> Result<Vec<ContainerMember>, StoreError> {
        self.transaction(async |scope| {
            create_containers_and_blocks_with_admin(
                scope,
                containers_and_blocks,
                user_id,
                NewBodyFiles::Recorded,
            )
            .await
        })
        .await
    }

    /// Atomically inserts every container, records every written file (removing its
    /// `writing` mark), inserts every block (checking body file references against the
    /// recorded files), and makes `user_id` an administrator and editor of each
    /// container, all in one transaction.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::InvalidPath`] for a malformed file path,
    /// [`StoreError::NotFound`] if `user_id` does not work in a container's workspace or
    /// a file's writing mark is gone, [`StoreError::InvalidBlock`] for an invalid block,
    /// or [`StoreError::Database`] if a statement fails. Nothing is written on error.
    pub async fn create_imported_containers_blocks_and_files(
        &self,
        containers_and_blocks: &mut ContainersAndBlocks,
        files: &[FileRecord],
        user_id: &str,
    ) -> Result<Vec<ContainerMember>, StoreError> {
        self.create_imported_containers_blocks_files_and_members(
            containers_and_blocks,
            files,
            &[],
            user_id,
        )
        .await
    }

    /// Atomically inserts every container, records every written file (removing its
    /// `writing` mark), inserts every block (checking body file references against the
    /// recorded files), makes `user_id` an administrator and editor of each container,
    /// and adds any `extra_members` not already present, all in one transaction.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::InvalidPath`] for a malformed file path,
    /// [`StoreError::NotFound`] if `user_id` does not work in a container's workspace,
    /// a member's user does not exist, or a file's writing mark is gone,
    /// [`StoreError::InvalidContainer`] or [`StoreError::InvalidBlock`] on validation
    /// or limit refusal (such as `DIARIES_PER_SPACE` or duplicate diary days), or
    /// [`StoreError::Database`] if a statement fails. Nothing is written on error.
    pub async fn create_imported_containers_blocks_files_and_members(
        &self,
        containers_and_blocks: &mut ContainersAndBlocks,
        files: &[FileRecord],
        extra_members: &[ContainerMember],
        user_id: &str,
    ) -> Result<Vec<ContainerMember>, StoreError> {
        for file in files {
            files::check_new_path(&file.path)?;
        }
        self.transaction(async |scope| {
            create_imported_containers_blocks_and_files(
                scope,
                containers_and_blocks,
                files,
                extra_members,
                user_id,
            )
            .await
        })
        .await
    }

    /// Applies each board patch and each block patch of `patches` to the board or block
    /// at the same position. Returns the patched boards and blocks, in the order of the
    /// request.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NotFound`] if a board or block does not exist,
    /// [`StoreError::InvalidBlock`] if a patched block fails validation, and
    /// [`StoreError::InvalidContainersAndBlocks`] if there are more identifiers than
    /// patches. Nothing is changed in any of these cases.
    pub async fn patch_containers_and_blocks(
        &self,
        patches: &PatchContainersAndBlocks,
        user_id: &str,
    ) -> Result<ContainersAndBlocks, StoreError> {
        self.transaction(async |scope| patch_containers_and_blocks(scope, patches, user_id).await)
            .await
    }

    /// Deletes the listed boards, each with all its blocks, into the trash. The listed
    /// blocks must be blocks of those boards, and go with them.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NotFound`] if a board or block does not exist, and
    /// [`StoreError::BlockNotInDeletedContainers`] if a block belongs to none of the listed
    /// boards. Nothing is deleted in either case.
    pub async fn delete_containers_and_blocks(
        &self,
        request: &DeleteContainersAndBlocks,
        user_id: &str,
    ) -> Result<(), StoreError> {
        self.transaction(async |scope| delete_containers_and_blocks(scope, request, user_id).await)
            .await
    }

    /// Copies a board and its blocks, except comments, under new identifiers. Returns the
    /// copies and the membership that makes `user_id` an administrator of the new board.
    ///
    /// The copy is a private board created by `user_id`, in `to_workspace`
    /// or else in the workspace of the source board. A copy of the same kind as the source
    /// is titled by `titles.copy_of` from the source title, as it stands when it is
    /// copied; a template made from a board is titled `titles.new_template`, and a board
    /// made from a template keeps the title. The copy is made without the properties
    /// of the board that `changes` names, its days as many days later as it says.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NotFound`] if there is no such board, or if `user_id` does
    /// not work in the copy's workspace, and the store's error if the copy cannot be
    /// written.
    pub async fn duplicate_container(
        &self,
        container_id: &str,
        user_id: &str,
        to_workspace: Option<&str>,
        duplicate_as: DuplicateAs,
        titles: &CopyTitles,
        changes: CopyChanges<'_>,
    ) -> Result<(ContainersAndBlocks, Vec<ContainerMember>), StoreError> {
        self.transaction(async |scope| {
            duplicate_container(
                scope,
                container_id,
                user_id,
                to_workspace,
                duplicate_as,
                titles,
                changes,
            )
            .await
        })
        .await
    }
}

pub(crate) async fn create_containers_and_blocks(
    scope: &mut Scope<'_>,
    containers_and_blocks: &mut ContainersAndBlocks,
    user_id: &str,
    new_files: NewBodyFiles,
) -> Result<(), StoreError> {
    containers::lock_containers(
        scope,
        containers_and_blocks
            .containers
            .iter()
            .map(|container| container.id.as_str()),
    )
    .await?;
    for container in &mut containers_and_blocks.containers {
        containers::insert_container(scope, container, user_id).await?;
    }
    // Parents first, wherever in the list they are: a copied board hands its blocks
    // over oldest first, and a block may be older than the block it is on.
    // What is made with its board is a copy, a template's or an archive's: it names
    // whom the original named, and nobody is told.
    let nobody = Telling::Nobody;
    blocks::insert_blocks(
        scope,
        &mut containers_and_blocks.blocks,
        user_id,
        new_files,
        nobody,
    )
    .await
}

pub(crate) async fn create_containers_and_blocks_with_admin(
    scope: &mut Scope<'_>,
    containers_and_blocks: &mut ContainersAndBlocks,
    user_id: &str,
    new_files: NewBodyFiles,
) -> Result<Vec<ContainerMember>, StoreError> {
    create_containers_and_blocks(scope, containers_and_blocks, user_id, new_files).await?;
    add_creator_admins(scope, &containers_and_blocks.containers).await
}

async fn stream_block_batches_in_tx<E>(
    scope: &mut Scope<'_>,
    user_id: &str,
    new_files: NewBodyFiles,
    mut batches: tokio::sync::mpsc::Receiver<StreamBatch<E>>,
    abort: &mut Option<StreamAbort<E>>,
) -> Result<(), StoreError> {
    let now = scope.now_millis();
    let mut named = Vec::new();
    let mut finished = false;
    while let Some(next) = batches.recv().await {
        let mut batch = match next {
            StreamBatch::Batch(batch) => batch,
            StreamBatch::Done => {
                finished = true;
                break;
            }
            StreamBatch::Err(error) => {
                *abort = Some(StreamAbort::Producer(error));
                return Err(StoreError::NoRows);
            }
        };
        if batch.is_empty() {
            continue;
        }
        for block in &batch {
            block
                .validate()
                .map_err(|source| StoreError::InvalidBlock {
                    id: block.id.clone(),
                    source,
                })?;
        }
        for index in blocks::parents_first(&batch) {
            let block = &mut batch[index];
            named.extend(
                blocks::insert_block_at(
                    scope,
                    block,
                    user_id,
                    Some(now),
                    Written {
                        new_files,
                        body_write: BodyWrite::Whole,
                        telling: Telling::Nobody,
                    },
                )
                .await?,
            );
        }
    }
    if !finished {
        *abort = Some(StreamAbort::Incomplete);
        return Err(StoreError::NoRows);
    }
    card_values::check_named_cards(scope, &named).await
}

fn map_stream_outcome<T, E>(
    outcome: Result<T, StoreError>,
    abort: Option<StreamAbort<E>>,
) -> Result<T, StreamCreateError<E>> {
    match outcome {
        Ok(value) => Ok(value),
        Err(store_error) => match abort {
            Some(StreamAbort::Producer(error)) => Err(StreamCreateError::Producer(error)),
            Some(StreamAbort::Incomplete) => Err(StreamCreateError::Incomplete),
            None => Err(StreamCreateError::Store(store_error)),
        },
    }
}

async fn record_imported_files(
    scope: &mut Scope<'_>,
    files: &[FileRecord],
) -> Result<(), StoreError> {
    for file in files {
        if !loose_objects::record_written(scope, file).await? {
            return Err(StoreError::NotFound(format!(
                "mark of the imported file {}",
                file.path
            )));
        }
    }
    Ok(())
}

async fn add_imported_members(
    scope: &mut Scope<'_>,
    containers: &[Container],
    extra_members: &[ContainerMember],
) -> Result<Vec<ContainerMember>, StoreError> {
    let mut members = add_creator_admins(scope, containers).await?;
    for member in extra_members {
        if container_membership::add_member(scope, member)
            .await?
            .is_none()
        {
            members.push(member.clone());
        }
    }
    Ok(members)
}

async fn create_imported_containers_blocks_and_files(
    scope: &mut Scope<'_>,
    containers_and_blocks: &mut ContainersAndBlocks,
    files: &[FileRecord],
    extra_members: &[ContainerMember],
    user_id: &str,
) -> Result<Vec<ContainerMember>, StoreError> {
    containers::lock_containers(
        scope,
        containers_and_blocks
            .containers
            .iter()
            .map(|container| container.id.as_str()),
    )
    .await?;
    for container in &mut containers_and_blocks.containers {
        containers::insert_container(scope, container, user_id).await?;
    }
    record_imported_files(scope, files).await?;
    blocks::insert_blocks(
        scope,
        &mut containers_and_blocks.blocks,
        user_id,
        NewBodyFiles::Recorded,
        Telling::Nobody,
    )
    .await?;
    add_imported_members(scope, &containers_and_blocks.containers, extra_members).await
}

async fn add_creator_admins(
    scope: &mut Scope<'_>,
    containers: &[Container],
) -> Result<Vec<ContainerMember>, StoreError> {
    let mut members = Vec::with_capacity(containers.len());
    for container in containers {
        // As in `insert_container_with_admin`, the administrator is the board's creator:
        // `user_id` for a new board, the stored creator for one that existed.
        members::hold_creator(scope, &container.workspace_id, &container.created_by).await?;
        let member = ContainerMember::new(
            &container.id,
            &container.created_by,
            fvoci_model::ContainerRole::Admin,
        );
        containers::save_member(scope, &member).await?;
        members.push(member);
    }
    Ok(members)
}

pub(crate) async fn patch_containers_and_blocks(
    scope: &mut Scope<'_>,
    patches: &PatchContainersAndBlocks,
    user_id: &str,
) -> Result<ContainersAndBlocks, StoreError> {
    // Return a mismatch error below if there are fewer patches than identifiers instead of panicking.
    // The boards that are patched and the boards of the blocks that are patched are
    // locked in one order, before any of them is written. A block that does not exist
    // is reported where it is patched, below.
    let block_containers = blocks::find_container_ids(scope, &patches.block_ids).await?;
    let to_lock = patches.container_ids.iter().chain(&block_containers);
    containers::lock_containers(scope, to_lock.map(String::as_str)).await?;

    let mut result = ContainersAndBlocks::default();
    for (index, container_id) in patches.container_ids.iter().enumerate() {
        let patch = patches
            .container_patches
            .get(index)
            .ok_or(ContainersAndBlocksError::ContainerIdsAndPatchesMismatch)?;
        let container = containers::patch_container(scope, container_id, patch, user_id).await?;
        result.containers.push(container);
    }
    for (index, block_id) in patches.block_ids.iter().enumerate() {
        let patch = patches
            .block_patches
            .get(index)
            .ok_or(ContainersAndBlocksError::BlockIdsAndPatchesMismatch)?;
        let telling = Telling::of(scope);
        let block =
            blocks::patch_block(scope, block_id, patch, user_id, BodyWrite::Whole, telling).await?;
        result.blocks.push(block);
    }
    Ok(result)
}

pub(crate) async fn delete_containers_and_blocks(
    scope: &mut Scope<'_>,
    request: &DeleteContainersAndBlocks,
    user_id: &str,
) -> Result<(), StoreError> {
    let container_ids: HashSet<&str> = request.containers.iter().map(String::as_str).collect();
    // The organization's before the boards': with a joining that ends, whose roles on
    // them are given back in the trash too (`joinings::give_back_roles`).
    org_admin::lock_organization(scope).await?;
    containers::lock_containers(scope, container_ids.iter().copied()).await?;

    // Every listed block must be a block of one of the listed boards. That is all
    // there is to do about the blocks: a board takes all its blocks into the trash, as
    // one deletion, and they come back with it.
    for block_id in &request.blocks {
        let block = blocks::get_block(scope, block_id).await?;
        if !container_ids.contains(block.container_id.as_str()) {
            return Err(StoreError::BlockNotInDeletedContainers {
                block_id: block_id.clone(),
            });
        }
    }
    for container_id in &request.containers {
        trash::delete_container(scope, container_id, user_id).await?;
    }
    Ok(())
}

pub(crate) async fn duplicate_container(
    scope: &mut Scope<'_>,
    container_id: &str,
    user_id: &str,
    to_workspace: Option<&str>,
    duplicate_as: DuplicateAs,
    titles: &CopyTitles,
    changes: CopyChanges<'_>,
) -> Result<(ContainersAndBlocks, Vec<ContainerMember>), StoreError> {
    let mut container = containers::get_container(scope, container_id).await?;
    for name in changes.without_properties {
        container.properties.remove(*name);
    }

    let as_template = duplicate_as == DuplicateAs::Template;
    if as_template == container.is_template {
        // board -> board or template -> template
        container.title = (titles.copy_of)(&container.title);
    } else if as_template {
        // template from board
        titles.new_template.clone_into(&mut container.title);
    }

    // The copy is private to the user who made it.
    container.visibility = ContainerVisibility::Private;
    container.is_template = as_template;
    user_id.clone_into(&mut container.created_by);
    if let Some(workspace_id) = to_workspace {
        // A workspace of another organization is none, before anything is written.
        if !workspace_membership::is_workspace_here(scope, workspace_id).await? {
            return Err(StoreError::not_found("workspace"));
        }
        workspace_id.clone_into(&mut container.workspace_id);
    }

    let mut blocks: Vec<_> = blocks::get_blocks(scope, &BlockFilter::container(container_id))
        .await?
        .into_iter()
        .filter(|block| block.block_type != BlockType::Comment)
        .collect();
    card_values::clear_copied_values(&mut blocks, &container.card_properties);
    let dates = fvoci_model::date_property_ids(&container.card_properties);
    for block in &mut blocks {
        fvoci_model::move_dates(&mut block.fields, &dates, changes.days_later);
    }

    // A board without blocks, or with comments only, is copied as a board without blocks.
    let mut copies = renew_containers_and_blocks_ids(ContainersAndBlocks {
        containers: vec![container],
        blocks,
    });
    // The copies show the original board's files until theirs are copied.
    let members = create_containers_and_blocks_with_admin(
        scope,
        &mut copies,
        user_id,
        NewBodyFiles::Unchecked,
    )
    .await?;
    Ok((copies, members))
}

async fn insert_streamed_schedules(
    scope: &mut Scope<'_>,
    containers: &[Container],
    schedules: &[(usize, fvoci_model::ValidatedCourseSchedule)],
) -> Result<(), StoreError> {
    if schedules.is_empty() {
        return Ok(());
    }
    let org = scope.org()?;
    for (index, schedule) in schedules {
        let Some(course) = containers
            .get(*index)
            .filter(|c| c.kind == fvoci_model::ContainerKind::Course)
        else {
            return Err(StoreError::not_found(format_args!(
                "course at container index {index}"
            )));
        };
        crate::course_schedules::upsert_schedule_row(scope, &course.id, schedule, org.clone())
            .await?;
    }
    Ok(())
}
