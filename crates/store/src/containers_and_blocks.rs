//! Operations on several boards and blocks at once, all or none.

use std::collections::BTreeSet;
use std::convert::Infallible;

use fvoci_model::{
    Block, BlockType, Container, ContainerMember, ContainerVisibility, ContainersAndBlocks,
    DeleteContainersAndBlocks, DuplicateAs, FileRecord, ValidatedCourseSchedule,
    renew_containers_and_blocks_ids,
};
use tokio::sync::mpsc::Receiver;

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

/// Message sent from a streaming block producer into [`Store::create_containers`]
/// ([`NewContainers::streamed`]).
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

/// Failure returned by [`Store::create_containers`].
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

pub(crate) enum StreamAbort<E> {
    Producer(E),
    Incomplete,
}

/// Boards and what is made with them in one transaction, all or none
/// ([`Store::create_containers`]): the boards, then the files their blocks show, then
/// their blocks, then the schedules of the courses among them, then their members.
///
/// Made by [`NewContainers::whole`] or [`NewContainers::streamed`], for a write whose
/// blocks are checked as a client's are ([`NewBodyFiles::Recorded`]), that records no
/// file and makes the creator of each board its administrator ([`Creator::Admin`]);
/// what else goes with the boards is said by the methods that take it.
#[derive(Debug)]
#[must_use = "nothing is made until it is given to `Store::create_containers`"]
pub struct NewContainers<'a, E = Infallible> {
    containers: &'a mut [Container],
    blocks: NewBlocks<'a, E>,
    body_files: NewBodyFiles,
    files: &'a [FileRecord],
    creator: Creator,
    members: &'a [ContainerMember],
    schedules: &'a [(usize, ValidatedCourseSchedule)],
}

/// The blocks of [`NewContainers`], which its constructors choose.
#[derive(Debug)]
pub(crate) enum NewBlocks<'a, E> {
    /// All of them at once, inserted or updated as [`Store::insert_block`] does each,
    /// parents first wherever in the list they are, and left as they were stored.
    Whole(&'a mut Vec<Block>),
    /// Batch after batch from a producer, each block new and made at the same time as
    /// the others, so that ordering by creation time and identifier stays the same; the
    /// cards their values name across batches are asked after the last.
    Streamed(Receiver<StreamBatch<E>>),
}

/// Who is made a member of the boards of [`NewContainers`] besides those it names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Creator {
    /// The creator of each board (its `created_by`) is made its administrator, and
    /// must work in its workspace.
    Admin,
    /// Nobody: for boards that are nobody's, such as the built-in templates.
    Nobody,
}

impl<'a> NewContainers<'a> {
    /// The boards and the blocks of `made`, both left as they were stored.
    pub fn whole(made: &'a mut ContainersAndBlocks) -> Self {
        let ContainersAndBlocks { containers, blocks } = made;
        Self::of(containers, NewBlocks::Whole(blocks))
    }
}

impl<'a, E> NewContainers<'a, E> {
    /// The boards `containers`, left as they were stored, with the blocks `batches`
    /// brings.
    pub fn streamed(containers: &'a mut [Container], batches: Receiver<StreamBatch<E>>) -> Self {
        Self::of(containers, NewBlocks::Streamed(batches))
    }

    fn of(containers: &'a mut [Container], blocks: NewBlocks<'a, E>) -> Self {
        Self {
            containers,
            blocks,
            body_files: NewBodyFiles::Recorded,
            files: &[],
            creator: Creator::Admin,
            members: &[],
            schedules: &[],
        }
    }

    /// Asks the files the blocks' bodies name as `body_files` says.
    pub fn body_files(mut self, body_files: NewBodyFiles) -> Self {
        self.body_files = body_files;
        self
    }

    /// Records `files`, whose objects were written at places marked with
    /// [`Store::mark_writing`], before the blocks that show them: the records take the
    /// marks away.
    pub fn recording(mut self, files: &'a [FileRecord]) -> Self {
        self.files = files;
        self
    }

    /// Makes members of the boards as `creator` says.
    pub fn creator(mut self, creator: Creator) -> Self {
        self.creator = creator;
        self
    }

    /// Adds `members` too, each one that is not a member already.
    pub fn with_members(mut self, members: &'a [ContainerMember]) -> Self {
        self.members = members;
        self
    }

    /// Stores each schedule of `schedules` for the course at its index of the boards.
    pub fn with_schedules(mut self, schedules: &'a [(usize, ValidatedCourseSchedule)]) -> Self {
        self.schedules = schedules;
        self
    }
}

impl From<StreamCreateError<Infallible>> for StoreError {
    /// The error of a write of boards whose producer cannot fail: the store's, or that
    /// its stream ended before its end, which rolled the transaction back as
    /// [`StoreError::NoRows`].
    fn from(error: StreamCreateError<Infallible>) -> Self {
        match error {
            StreamCreateError::Store(error) => error,
            StreamCreateError::Producer(never) => match never {},
            StreamCreateError::Incomplete => Self::NoRows,
        }
    }
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
            let made = NewContainers::whole(templates)
                .body_files(NewBodyFiles::Unchecked)
                .creator(Creator::Nobody);
            create_whole(scope, made, "").await?;
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

    /// Makes the boards of `new` and what goes with them in one transaction, all or
    /// none, as `user_id`, who is the creator of each board that is new; a board that
    /// was there is updated and keeps its creator. Returns the memberships made, those
    /// of [`Creator::Admin`] in the order of the boards and then those of
    /// [`NewContainers::with_members`]; the boards and blocks of `new` are left as they
    /// were stored. What is made with its board is a copy, a template's or an
    /// archive's: it names whom the original named, and nobody is told.
    ///
    /// # Errors
    ///
    /// Returns [`StreamCreateError::Store`] with [`StoreError::InvalidPath`] for a file at
    /// a place spelled otherwise than the one way, before anything is asked;
    /// [`StoreError::InvalidContainer`] or [`StoreError::InvalidBlock`] for a board or a
    /// block that is refused (a limit included); [`StoreError::NotFound`] if a file's
    /// place is no longer marked, a creator made administrator does not work in the
    /// board's workspace, a member's user does not exist or a schedule names no course
    /// of the boards. Returns [`StreamCreateError::Producer`] if the stream brings
    /// [`StreamBatch::Err`], and [`StreamCreateError::Incomplete`] if it closes before
    /// [`StreamBatch::Done`]. Nothing is written on error.
    pub async fn create_containers<E: Send>(
        &self,
        new: NewContainers<'_, E>,
        user_id: &str,
    ) -> Result<Vec<ContainerMember>, StreamCreateError<E>> {
        for file in new.files {
            files::check_new_path(&file.path)?;
        }
        let mut abort = None;
        let outcome = self
            .transaction(async |scope| create_containers(scope, new, user_id, &mut abort).await)
            .await;
        map_stream_outcome(outcome, abort)
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

/// Makes `new` in the transaction of `scope`, as [`Store::create_containers`] says; a
/// stream that fails or ends early leaves why in `abort` and fails the transaction.
pub(crate) async fn create_containers<E>(
    scope: &mut Scope<'_>,
    new: NewContainers<'_, E>,
    user_id: &str,
    abort: &mut Option<StreamAbort<E>>,
) -> Result<Vec<ContainerMember>, StoreError> {
    let NewContainers {
        containers,
        blocks,
        body_files,
        files,
        creator,
        members,
        schedules,
    } = new;
    containers::lock_containers(scope, containers.iter().map(|made| made.id.as_str())).await?;
    for container in containers.iter_mut() {
        containers::insert_container(scope, container, user_id).await?;
    }
    // Taken again now that the new ones are there to be held: what is written of them
    // below finds them held.
    containers::lock_containers(scope, containers.iter().map(|made| made.id.as_str())).await?;
    record_files(scope, files).await?;
    match blocks {
        // Parents first, wherever in the list they are: a copied board hands its blocks
        // over oldest first, and a block may be older than the block it is on.
        NewBlocks::Whole(blocks) => {
            blocks::insert_blocks(scope, blocks, user_id, body_files, Telling::Nobody).await?;
        }
        NewBlocks::Streamed(batches) => {
            stream_block_batches_in_tx(scope, user_id, body_files, batches, abort).await?;
        }
    }
    insert_schedules(scope, containers, schedules).await?;
    add_members(scope, containers, creator, members).await
}

/// Makes `new`, whose blocks come whole, in the transaction of `scope`, as
/// [`create_containers`] does.
pub(crate) async fn create_whole(
    scope: &mut Scope<'_>,
    new: NewContainers<'_>,
    user_id: &str,
) -> Result<Vec<ContainerMember>, StoreError> {
    create_containers(scope, new, user_id, &mut None).await
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

async fn record_files(scope: &mut Scope<'_>, files: &[FileRecord]) -> Result<(), StoreError> {
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

/// Makes members of `containers` as `creator` says, and then of `members` each who is
/// not one already. The memberships made, in that order.
async fn add_members(
    scope: &mut Scope<'_>,
    containers: &[Container],
    creator: Creator,
    members: &[ContainerMember],
) -> Result<Vec<ContainerMember>, StoreError> {
    let mut made = Vec::with_capacity(containers.len() + members.len());
    if creator == Creator::Admin {
        for container in containers {
            // As in `insert_container_with_admin`, the administrator is the board's
            // creator: `user_id` for a new board, the stored creator for one that existed.
            members::hold_creator(scope, &container.workspace_id, &container.created_by).await?;
            let member = ContainerMember::new(
                &container.id,
                &container.created_by,
                fvoci_model::ContainerRole::Admin,
            );
            containers::save_member(scope, &member).await?;
            made.push(member);
        }
    }
    for member in members {
        if container_membership::add_member(scope, member)
            .await?
            .is_none()
        {
            made.push(member.clone());
        }
    }
    Ok(made)
}

pub(crate) async fn delete_containers_and_blocks(
    scope: &mut Scope<'_>,
    request: &DeleteContainersAndBlocks,
    user_id: &str,
) -> Result<(), StoreError> {
    let container_ids: BTreeSet<&str> = request.containers.iter().map(String::as_str).collect();
    // The organization's before the boards': with a joining that ends, whose roles on
    // them are given back in the trash too (`joinings::give_back_roles`). The boards
    // as their deletion holds them, in their order, before anything under them.
    org_admin::lock_organization(scope).await?;
    containers::lock_containers_for_deletion(scope, &container_ids).await?;

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
    // Every block of the boards before the first goes: each then holds what it holds.
    let ids: Vec<&str> = container_ids.iter().copied().collect();
    trash::hold_blocks_of_containers(scope, &ids).await?;
    for container_id in &container_ids {
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
    let made = NewContainers::whole(&mut copies).body_files(NewBodyFiles::Unchecked);
    let members = create_whole(scope, made, user_id).await?;
    Ok((copies, members))
}

async fn insert_schedules(
    scope: &mut Scope<'_>,
    containers: &[Container],
    schedules: &[(usize, ValidatedCourseSchedule)],
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
