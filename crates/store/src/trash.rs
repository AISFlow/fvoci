//! The trash: deleting boards and blocks, and bringing them back.
//!
//! The tables of boards and blocks hold what exists. Deleting moves a row out of them
//! into `deleted_containers` or `deleted_blocks`, whole, as JSON; restoring moves it back.
//! So no query of what exists has to leave out what is deleted, and the foreign keys of
//! what exists hold without exceptions.
//!
//! Blocks that go with one deletion share a `deletion_id` and come back together: a
//! card with what is on it, a board with all its blocks. A block takes the subscriptions
//! to it (`watchers.rs`) and the reactions to it (`reactions.rs`) into the trash and
//! brings them back. A board takes its
//! memberships, its file records and the places users filed it under into the trash and
//! brings them back; its sharing ends with the deletion.
//!
//! What has been in the trash for longer than the retention period is removed for good
//! by [`Store::purge_trash`], and what people were told of it with it (`told.rs`).

mod holds;
mod reactions;
mod stored;
mod told;
mod watchers;

pub(crate) use holds::{hold_blocks_of_containers, hold_trees_of};
pub(crate) use reactions::{KeptReactions, forget_reactions_of_organization};
pub(crate) use watchers::{
    forget_watcher_of_container, forget_watcher_of_organization, forget_watcher_of_workspace,
};

use fvoci_model::{
    Block, Body, Container, ContainerKinds, ContainerMember, ContainerVisibility, FileRecord,
    new_id,
};
use serde::de::DeserializeOwned;
pub(crate) use stored::undelete_block;
use stored::{KnownUsers, StoredFiles, StoredPlace};
use told::Gone;

use crate::Store;
use crate::block_derived;
use crate::blocks::{self, Paging, parents_first};
use crate::card_hierarchy;
use crate::category_containers::{self, ContainerPlace};
use crate::containers;
use crate::db::{
    DeletedContainersHeld, Dialect, Id, LockRank, OptId, Param, Row, RowLock, Scope, Sql,
    SqlFragment,
};
use crate::error::StoreError;
use crate::file_refs::NewBodyFiles;
use crate::files;
use crate::loose_objects;
use crate::members;
use crate::org_admin;

/// A block in the trash.
#[derive(Debug, Clone, PartialEq)]
pub struct DeletedBlock {
    /// The block as it was when it was deleted.
    pub block: Block,
    /// When it was deleted, in milliseconds since the Unix epoch.
    pub deleted_at: i64,
    /// The identifier of the user who deleted it; empty if nobody did.
    pub deleted_by: String,
}

/// A board in the trash.
#[derive(Debug, Clone, PartialEq)]
pub struct DeletedContainer {
    /// The board as it was when it was deleted.
    pub container: Container,
    /// The memberships the board had.
    pub members: Vec<ContainerMember>,
    /// When it was deleted, in milliseconds since the Unix epoch.
    pub deleted_at: i64,
    /// The identifier of the user who deleted it; empty if nobody did.
    pub deleted_by: String,
}

impl Store {
    /// Deletes a block and everything below it: moves them to the trash, as one
    /// deletion, with the subscriptions to them. Deleting a block that does not exist
    /// succeeds.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if a statement fails; nothing is deleted then.
    pub async fn delete_block(&self, block_id: &str, user_id: &str) -> Result<(), StoreError> {
        self.delete_block_family(block_id, user_id)
            .await
            .map(|_| ())
    }

    /// Deletes a block and everything below it as [`Store::delete_block`] does, returning
    /// every block that moved to the trash (empty if the block did not exist).
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if a statement fails; nothing is deleted then.
    pub async fn delete_block_family(
        &self,
        block_id: &str,
        user_id: &str,
    ) -> Result<Vec<Block>, StoreError> {
        self.transaction(async |scope| delete_block(scope, block_id, user_id).await)
            .await
    }

    /// Returns a block that is in the trash.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NotFound`] if no block of that identifier is in the trash.
    pub async fn get_deleted_block(&self, block_id: &str) -> Result<DeletedBlock, StoreError> {
        self.with_conn(async |scope| {
            let mut sql =
                scope.sql("SELECT block, deleted_by, deleted_at FROM deleted_blocks WHERE id = ");
            sql.bind(Id(block_id));
            let row = scope
                .fetch_optional(sql)
                .await?
                .ok_or_else(block_not_in_trash)?;
            Ok(DeletedBlock {
                block: block_from_json(&row, 0)?,
                deleted_by: row.id(1)?,
                deleted_at: row.int(2)?,
            })
        })
        .await
    }

    /// Returns the blocks of the deletion that a block of the trash went with: the one
    /// that was deleted, and took the others along, first, then the others by
    /// identifier. Restoring any of them restores them all ([`Store::undelete_block`]).
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NotFound`] if no block of that identifier is in the trash.
    pub async fn get_deletion(&self, block_id: &str) -> Result<Vec<Block>, StoreError> {
        self.with_conn(async |scope| {
            let mut sql = scope.sql(
                "SELECT went.block FROM deleted_blocks named \
                 JOIN deleted_blocks went ON went.deletion_id = named.deletion_id \
                 WHERE named.id = ",
            );
            sql.bind(Id(block_id)).push(
                " ORDER BY CASE WHEN EXISTS (SELECT 1 FROM deleted_blocks above \
                 WHERE above.deletion_id = went.deletion_id AND above.id = went.parent_id) \
                 THEN 1 ELSE 0 END, went.id",
            );
            let rows = scope.fetch_all(sql).await?;
            if rows.is_empty() {
                return Err(block_not_in_trash());
            }
            rows.iter().map(|row| block_from_json(row, 0)).collect()
        })
        .await
    }

    /// Returns the blocks below a block, at any depth, by identifier: what deleting it
    /// takes along ([`Store::delete_block`]). Empty for a block that has nothing below it
    /// or does not exist.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the query fails.
    pub async fn get_blocks_below(&self, block_id: &str) -> Result<Vec<Block>, StoreError> {
        self.with_conn(async |scope| {
            let mut sql = with_tree_of(scope, block_id);
            sql.push([
                "SELECT ",
                blocks::BLOCK_COLUMNS,
                ", ",
                crate::comment_versions::EDITED_AT_COLUMN,
                " FROM blocks WHERE id IN (SELECT id FROM tree) AND id <> ",
            ]);
            sql.bind(Id(block_id)).push(" ORDER BY id");
            blocks::fetch_blocks(scope, sql).await
        })
        .await
    }

    /// Returns the cards of a board that are in the trash at the top of what was deleted
    /// with them, the last deleted first and then by identifier: the page `paging` names,
    /// or every one without it. A card that went with the block it was on (a card, a
    /// view) comes back with that block and is not listed.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the query fails.
    pub async fn get_deleted_cards(
        &self,
        container_id: &str,
        paging: Option<Paging>,
    ) -> Result<Vec<DeletedBlock>, StoreError> {
        self.with_conn(async |scope| {
            let mut sql = scope.sql(
                "SELECT block, deleted_by, deleted_at FROM deleted_blocks WHERE container_id = ",
            );
            sql.bind(Id(container_id)).push(
                " AND block_type = 'card' AND NOT EXISTS (SELECT 1 FROM deleted_blocks below \
                 WHERE below.deletion_id = deleted_blocks.deletion_id \
                 AND below.id = deleted_blocks.parent_id) ORDER BY deleted_at DESC, id",
            );
            sql.push_page(paging);
            let mut cards = Vec::new();
            for row in &scope.fetch_all(sql).await? {
                let block = match block_from_json(row, 0) {
                    Ok(block) => block,
                    Err(StoreError::Corrupt { column, reason }) => {
                        tracing::error!(
                            container_id,
                            column,
                            %reason,
                            "skipping corrupt deleted card in trash listing"
                        );
                        continue;
                    }
                    Err(error) => return Err(error),
                };
                cards.push(DeletedBlock {
                    block,
                    deleted_by: row.id(1)?,
                    deleted_at: row.int(2)?,
                });
            }
            Ok(cards)
        })
        .await
    }

    /// Restores a block from the trash, with every block that was deleted along with
    /// it and the subscriptions to them, and returns the block. The restored blocks are
    /// marked as modified by `user_id` now. A top-level card in the deletion whose parent
    /// card is no longer live is re-rooted to the top level (`parent_id == ""`).
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NotFound`] if no block of that identifier is in the trash,
    /// or there is nothing to restore it to: its board does not exist (any more, or is
    /// in the trash itself), or (for non-card blocks) the block it was on does not.
    /// Returns [`StoreError::NotRestorable`] if a block at the top would come back where
    /// it may not be: a comment off a card, any block below a comment, or a card whose
    /// live parent is on another board, is not a non-template card, or would push the
    /// restored card subtree past [`fvoci_model::MAX_CARD_DEPTH`].
    pub async fn undelete_block(&self, block_id: &str, user_id: &str) -> Result<Block, StoreError> {
        let restored = self.undelete_block_family(block_id, user_id).await?;
        restored
            .into_iter()
            .find(|block| block.id == block_id)
            .ok_or_else(block_not_in_trash)
    }

    /// Restores a block from the trash with every block deleted along with it, returning
    /// all restored blocks in parents-first order.
    ///
    /// # Errors
    ///
    /// As [`Store::undelete_block`].
    pub async fn undelete_block_family(
        &self,
        block_id: &str,
        user_id: &str,
    ) -> Result<Vec<Block>, StoreError> {
        self.transaction(async |scope| {
            let mut reactions = KeptReactions::default();
            let restored = undelete_block(scope, block_id, user_id, &mut reactions).await?;
            reactions.make_again(scope).await?;
            Ok(restored)
        })
        .await
    }

    /// Deletes a board with all its blocks: moves them to the trash, together with the
    /// subscriptions to them and the board's memberships, file records and the places
    /// users filed it under. The board's sharing ends.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NotFound`] if there is no such board.
    pub async fn delete_container(
        &self,
        container_id: &str,
        user_id: &str,
    ) -> Result<(), StoreError> {
        self.transaction(async |scope| {
            // Before the board's: with a joining that ends (`joinings::give_back_roles`).
            org_admin::lock_organization(scope).await?;
            delete_container(scope, container_id, user_id).await
        })
        .await
    }

    /// Returns a board that is in the trash, with the memberships it had.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NotFound`] if no board of that identifier is in the trash.
    pub async fn get_deleted_container(
        &self,
        container_id: &str,
    ) -> Result<DeletedContainer, StoreError> {
        self.with_conn(async |scope| {
            let mut sql = scope.sql(
                "SELECT container, members, deleted_by, deleted_at FROM deleted_containers WHERE id = ",
            );
            sql.bind(Id(container_id));
            let row = scope
                .fetch_optional(sql)
                .await?
                .ok_or_else(container_not_in_trash)?;
            Ok(DeletedContainer {
                container: from_json(&row, 0, "container")?,
                members: from_json(&row, 1, "members")?,
                deleted_by: row.id(2)?,
                deleted_at: row.int(3)?,
            })
        })
        .await
    }

    /// Returns the boards of `kinds` of a workspace that are in the trash and, with an
    /// `admin_id`, that the user was an admin of when they were deleted (`role` `admin` of
    /// the membership they had); without one, every such board of the workspace in the
    /// trash. With their memberships, the last deleted first and then by identifier: the
    /// page `paging` names, or every one without it.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the query fails.
    pub async fn get_deleted_containers(
        &self,
        workspace_id: &str,
        admin_id: Option<&str>,
        paging: Option<Paging>,
        kinds: ContainerKinds,
    ) -> Result<Vec<DeletedContainer>, StoreError> {
        self.with_conn(async |scope| {
            let kind = scope.json_text("container", "kind");
            // The memberships are the JSON array of the board's members when it went.
            let was_admin = match scope.dialect() {
                Dialect::Sqlite => {
                    " AND EXISTS (SELECT 1 FROM json_each(deleted_containers.members) member \
                     WHERE json_extract(member.value, '$.role') = 'admin' \
                     AND json_extract(member.value, '$.userId') = "
                }
                Dialect::Postgres => {
                    " AND EXISTS (SELECT 1 FROM json_array_elements(deleted_containers.members) \
                     member WHERE member->>'role' = 'admin' AND member->>'userId' = "
                }
            };
            let mut sql = scope.sql(
                "SELECT container, members, deleted_by, deleted_at FROM deleted_containers WHERE workspace_id = ",
            );
            sql.bind(Id(workspace_id));
            if let Some(admin_id) = admin_id {
                sql.push_bind(was_admin, admin_id).push(")");
            }
            crate::container_kinds::push_kinds(&mut sql, &kind, kinds);
            sql.push(" ORDER BY deleted_at DESC, id");
            sql.push_page(paging);
            let mut deleted = Vec::new();
            for row in &scope.fetch_all(sql).await? {
                let container = match from_json(row, 0, "container") {
                    Ok(container) => container,
                    Err(StoreError::Corrupt { column, reason }) => {
                        tracing::error!(
                            workspace_id,
                            column,
                            %reason,
                            "skipping corrupt deleted container in trash listing"
                        );
                        continue;
                    }
                    Err(error) => return Err(error),
                };
                let members = match from_json(row, 1, "members") {
                    Ok(members) => members,
                    Err(StoreError::Corrupt { column, reason }) => {
                        tracing::error!(
                            workspace_id,
                            column,
                            %reason,
                            "skipping deleted container with corrupt members in trash listing"
                        );
                        continue;
                    }
                    Err(error) => return Err(error),
                };
                deleted.push(DeletedContainer {
                    container,
                    members,
                    deleted_by: row.id(2)?,
                    deleted_at: row.int(3)?,
                });
            }
            Ok(deleted)
        })
        .await
    }

    /// Restores a board from the trash and returns it: the board, the blocks that were
    /// deleted with it and the subscriptions to them, its file records, and the
    /// memberships and places in the sidebar of the users who still exist: under the
    /// category the user had filed it under, or the user's default category if that
    /// category is gone since. The board is marked as modified by `user_id` now. Its
    /// sharing does not come back: a restored board is not shared until someone shares
    /// it again.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NotFound`] if no board of that identifier is in the trash.
    pub async fn undelete_container(
        &self,
        container_id: &str,
        user_id: &str,
    ) -> Result<Container, StoreError> {
        self.transaction(async |scope| {
            // Before the board's: with a joining that ends (`joinings::give_back_roles`).
            org_admin::lock_organization(scope).await?;
            crate::diaries::hold_place_of_deleted(scope, container_id).await?;
            undelete_container(scope, container_id, user_id).await
        })
        .await
    }

    /// Removes for good what was deleted before `deleted_before` (milliseconds since
    /// the Unix epoch), and the deleted blocks of boards that are gone for good, with
    /// what people were told of them. Returns how many boards and blocks went, and the
    /// file records of the boards that went, whose objects are doomed in the same
    /// transaction ([`Store::due_doomed_objects`]).
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if a statement fails; nothing is removed then.
    pub async fn purge_trash(&self, deleted_before: i64) -> Result<Purged, StoreError> {
        self.transaction(async |scope| {
            let mut purged = remove_containers(scope, Trashed::Before(deleted_before)).await?;
            let mut sql = scope.sql("DELETE FROM deleted_blocks WHERE deleted_at < ");
            sql.bind(deleted_before).push(" RETURNING id");
            purged.records += remove_blocks(scope, sql).await?;
            purged.records += remove_what_they_left(scope).await?;
            Ok(purged)
        })
        .await
    }
}

/// Which boards of the trash [`remove_containers`] removes for good.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Trashed<'a> {
    /// Those deleted before the time, in milliseconds since the Unix epoch.
    Before(i64),
    /// Those of the workspace, which goes for good.
    OfWorkspace(&'a str),
}

/// Removes boards from the trash for good and dooms the objects of their files: the
/// one way a board and its files go. What people were told of them goes too. Returns
/// how many went, and their file records.
pub(crate) async fn remove_containers(
    scope: &mut Scope<'_>,
    which: Trashed<'_>,
) -> Result<Purged, StoreError> {
    // One statement decides and deletes: a board restored meanwhile is not deleted, and
    // so its files are not doomed.
    let org = loose_objects::org_column(scope);
    let mut sql = scope.sql("DELETE FROM deleted_containers WHERE ");
    match which {
        Trashed::Before(deleted_before) => sql.push_bind("deleted_at < ", deleted_before),
        Trashed::OfWorkspace(workspace_id) => sql.push_bind("workspace_id = ", Id(workspace_id)),
    };
    sql.push(" RETURNING id, files");
    if let Some(column) = org {
        sql.push([", ", column]);
    }
    let containers = scope.fetch_all(sql).await?;
    let records = u64::try_from(containers.len()).unwrap_or(u64::MAX);
    let mut gone = Vec::with_capacity(containers.len());
    let mut files = Vec::new();
    let mut doomed = Vec::new();
    for row in &containers {
        gone.push(row.id(0)?);
        let of_container: StoredFiles = from_json(row, 1, "files")?;
        let org_id = org.map(|_| row.id(2)).transpose()?;
        for file in Vec::<FileRecord>::from(of_container) {
            // Nothing else says where the file is once the board is gone.
            doomed.push((file.path.clone(), org_id.clone()));
            files.push(file);
        }
    }
    loose_objects::doom(scope, &doomed).await?;
    let gone_files: Vec<&str> = files.iter().map(|file| file.id.as_str()).collect();
    crate::file_versions::forget(scope, &gone_files).await?;
    told::forget_told_of(scope, Gone::Containers(&gone)).await?;
    Ok(Purged { records, files })
}

/// Runs `sql`, a statement that removes blocks from the trash for good and returns
/// their identifiers, and forgets what people were told of them. Returns how many went.
async fn remove_blocks(scope: &mut Scope<'_>, sql: Sql) -> Result<u64, StoreError> {
    let gone = scope
        .fetch_all(sql)
        .await?
        .iter()
        .map(|row| row.id(0))
        .collect::<Result<Vec<String>, _>>()?;
    told::forget_told_of(scope, Gone::Blocks(&gone)).await?;
    Ok(u64::try_from(gone.len()).unwrap_or(u64::MAX))
}

/// Removes what boards that went for good leave behind: the blocks deleted on their
/// own from them, what those showed and the texts of their comments. Returns how many
/// blocks went.
pub(crate) async fn remove_what_they_left(scope: &mut Scope<'_>) -> Result<u64, StoreError> {
    // A block that was deleted on its own while its board existed has nothing to come
    // back to once the board is neither there nor in the trash of the block's
    // organization.
    let mut orphans = SqlFragment::of(&[
        "DELETE FROM deleted_blocks WHERE NOT EXISTS (SELECT 1 FROM containers b \
         WHERE b.id = deleted_blocks.container_id",
    ]);
    orphans
        .push_fragment(&scope.same_organization(&[("b", "deleted_blocks")]))
        .push(") AND NOT EXISTS (SELECT 1 FROM deleted_containers d WHERE d.id = deleted_blocks.container_id")
        .push_fragment(&scope.same_organization(&[("d", "deleted_blocks")]))
        .push(") RETURNING id");
    let sql = scope.sql(&orphans);
    let records = remove_blocks(scope, sql).await?;
    block_derived::forget_gone_blocks(scope).await?;
    crate::course_meetings::forget_gone_containers(scope).await?;
    crate::course_exams::forget_gone_containers(scope).await?;
    crate::course_schedules::forget_gone_containers(scope).await?;
    Ok(records)
}

/// What a purge of the trash removed for good.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Purged {
    /// How many boards and blocks went.
    pub records: u64,
    /// The file records of the boards that went. Their objects are doomed.
    pub files: Vec<FileRecord>,
}

pub(crate) fn block_not_in_trash() -> StoreError {
    StoreError::not_found("deleted block")
}

fn container_not_in_trash() -> StoreError {
    StoreError::not_found("deleted board")
}

/// Reads a block of the trash. Its body is taken as it was stored: it was checked when
/// it was written, against the allowlist of that time, which a later one may not
/// accept.
pub(crate) fn block_from_json(row: &Row, index: usize) -> Result<Block, StoreError> {
    block_from_value(row.json(index, "block")?)
}

/// What a block of the trash keeps beside itself, to make again when it comes back.
struct KeptWith {
    watchers: Vec<watchers::StoredWatcher>,
    reactions: Vec<reactions::StoredReaction>,
}

/// Reads a block and what it kept in the trash beside it from one JSON parse.
fn block_and_kept_from_json(row: &Row, index: usize) -> Result<(Block, KeptWith), StoreError> {
    let mut value = row.json(index, "block")?;
    let kept = KeptWith {
        watchers: watchers::take_kept(&mut value)?,
        reactions: reactions::take_kept(&mut value)?,
    };
    Ok((block_from_value(value)?, kept))
}

/// Decodes a stored trash block JSON value, keeping its body as stored.
fn block_from_value(mut value: serde_json::Value) -> Result<Block, StoreError> {
    let corrupt = |error: serde_json::Error| StoreError::Corrupt {
        column: "block",
        reason: error.to_string(),
    };
    let body = value
        .as_object_mut()
        .and_then(|members| members.remove("body"))
        .filter(|body| !body.is_null());
    let mut block: Block = serde_json::from_value(value).map_err(corrupt)?;
    block.body = body.map(Body::from_stored);
    Ok(block)
}

/// What becomes of a user's memberships of the boards of a workspace that are in the
/// trash, when the user stops being what they were in the workspace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TrashedRoles {
    /// They go: for who leaves the workspace, and who comes into it anew, and is not to
    /// find the roles of an earlier time on a board that is restored.
    Forget,
    /// An admin is an editor: for who becomes a guest of the workspace.
    NoMoreThanEditor,
}

/// Holds the boards of a workspace that are in the trash, in the order of their
/// identifiers, against whoever restores one or changes who it comes back with. Who
/// also holds a membership of the workspace takes these first, as a restore does.
pub(crate) async fn hold_deleted_containers<'c>(
    scope: &mut Scope<'c>,
    workspace_id: &str,
) -> Result<DeletedContainersHeld<'c>, StoreError> {
    let mut sql = scope.sql("SELECT id FROM deleted_containers WHERE workspace_id = ");
    sql.bind(Id(workspace_id)).push(" ORDER BY id");
    scope
        .hold(
            LockRank::DeletedContainers,
            workspace_id,
            sql,
            RowLock::Update,
        )
        .await?;
    Ok(DeletedContainersHeld::new())
}

/// Changes the memberships of a user that the boards of a workspace in the trash would
/// come back with, as `roles` says. The boards are held ([`hold_deleted_containers`]) from
/// before they are read: what is written back is the whole list of a board's members.
pub(crate) async fn change_member_of_deleted_containers(
    scope: &mut Scope<'_>,
    workspace_id: &str,
    user_id: &str,
    roles: TrashedRoles,
) -> Result<(), StoreError> {
    let _held = hold_deleted_containers(scope, workspace_id).await?;
    let mut sql = scope.sql("SELECT id, members FROM deleted_containers WHERE workspace_id = ");
    sql.bind(Id(workspace_id));
    for row in scope.fetch_all(sql).await? {
        let members: Vec<ContainerMember> = from_json(&row, 1, "members")?;
        let changes = |member: &ContainerMember| {
            member.user_id == user_id
                && (roles == TrashedRoles::Forget
                    || member.role == fvoci_model::ContainerRole::Admin)
        };
        if !members.iter().any(changes) {
            continue;
        }
        let kept: Vec<ContainerMember> = members
            .into_iter()
            .filter_map(|member| match roles {
                _ if !changes(&member) => Some(member),
                TrashedRoles::Forget => None,
                TrashedRoles::NoMoreThanEditor => {
                    Some(member.with_role(fvoci_model::ContainerRole::Editor))
                }
            })
            .collect();
        let mut sql = scope.sql("UPDATE deleted_containers SET members = ");
        sql.bind(Param::json(&kept)?)
            .push_bind(" WHERE id = ", Id(&row.id(0)?));
        scope.execute(sql).await?;
    }
    Ok(())
}

/// Reads a JSON column of the trash into the type it was written from.
pub(crate) fn from_json<T: DeserializeOwned>(
    row: &Row,
    index: usize,
    column: &'static str,
) -> Result<T, StoreError> {
    serde_json::from_value(row.json(index, column)?).map_err(|error| StoreError::Corrupt {
        column,
        reason: error.to_string(),
    })
}

/// Starts a statement over a block and everything below it: a `WITH` clause that
/// defines `tree`, the identifiers of those blocks.
///
/// `UNION` and not `UNION ALL`: it also ends the recursion if parents ever form a cycle.
pub(crate) fn with_tree_of(scope: &Scope<'_>, block_id: &str) -> crate::db::Sql {
    let mut sql = scope.sql("WITH RECURSIVE tree(id) AS (SELECT id FROM blocks WHERE id = ");
    sql.bind(Id(block_id))
        .push(" UNION SELECT b.id FROM blocks AS b JOIN tree ON b.parent_id = tree.id) ");
    sql
}

pub(crate) async fn delete_block(
    scope: &mut Scope<'_>,
    block_id: &str,
    user_id: &str,
) -> Result<Vec<Block>, StoreError> {
    // The block says which board it is on, and the board is then held: what is below
    // the block is read, written to the trash and deleted without anything being added
    // or taken away in between. Read again under the lock, the block may be gone.
    let Some(container_id) = blocks::find_container_id(scope, block_id).await? else {
        tracing::debug!(block_id, "delete_block: block not found");
        return Ok(Vec::new());
    };
    containers::lock_container(scope, &container_id).await?;
    let mut sql = with_tree_of(scope, block_id);
    sql.push([
        "SELECT ",
        blocks::BLOCK_COLUMNS,
        ", ",
        crate::comment_versions::EDITED_AT_COLUMN,
        " FROM blocks WHERE id IN (SELECT id FROM tree)",
    ]);
    let doomed = blocks::fetch_blocks(scope, sql).await?;
    if doomed.is_empty() {
        tracing::debug!(block_id, "delete_block: block deleted meanwhile");
        return Ok(Vec::new());
    }

    let now = scope.now_millis();
    put_blocks_in_trash(scope, &doomed, &new_id(), now, user_id).await?;
    block_derived::trashed(scope, &doomed).await?;

    // One statement for parents and children: the reference from a block to its parent
    // is checked when the statement is done.
    let mut sql = with_tree_of(scope, block_id);
    sql.push("DELETE FROM blocks WHERE id IN (SELECT id FROM tree)");
    scope.execute(sql).await?;
    Ok(doomed)
}

/// Writes the blocks into the trash as one deletion, batching multi-row inserts so
/// deleting a container or large card tree avoids per-block statement roundtrips.
async fn put_blocks_in_trash(
    scope: &mut Scope<'_>,
    blocks: &[Block],
    deletion_id: &str,
    deleted_at: i64,
    deleted_by: &str,
) -> Result<(), StoreError> {
    const TRASH_INSERT_BATCH: usize = 100;
    if blocks.is_empty() {
        return Ok(());
    }
    let ids: Vec<&str> = blocks.iter().map(|block| block.id.as_str()).collect();
    holds::hold_blocks(scope, &ids).await?;
    let mut watchers = watchers::watchers_of(scope, &ids).await?;
    let mut reactions = reactions::reactions_of(scope, &ids).await?;
    for chunk in blocks.chunks(TRASH_INSERT_BATCH) {
        let mut sql = scope.sql(
            "INSERT INTO deleted_blocks \
             (id, container_id, parent_id, block_type, deleted_at, deleted_by, deletion_id, block) \
             VALUES ",
        );
        for (index, block) in chunk.iter().enumerate() {
            let in_trash = Block {
                deleted_at: Some(deleted_at),
                ..block.clone()
            };
            let mut stored = watchers::with_watchers(&in_trash, watchers.remove(&block.id))?;
            reactions::keep(&mut stored, reactions.remove(&block.id))?;
            sql.push(if index == 0 { "(" } else { ", (" })
                .bind(Id(&block.id))
                .push_bind(", ", Id(&block.container_id))
                .push_bind(", ", OptId(&block.parent_id))
                .push_bind(", ", block.block_type.as_str())
                .push_bind(", ", deleted_at)
                .push_bind(", ", OptId(deleted_by))
                .push_bind(", ", Id(deletion_id))
                .push_bind(", ", Param::json(&stored)?)
                .push(")");
        }
        scope.execute(sql).await?;
    }
    Ok(())
}

/// Takes the blocks of one deletion out of the trash and puts them back among the
/// blocks, marked as modified by `user_id` at `now`, each as `adjust` leaves it, with
/// the subscriptions and the reactions they kept. Returns them.
///
/// The reactions are made again last ([`KeptReactions::make_again`]): the transaction
/// is to take no lock after it but of accounts further on.
pub(crate) async fn take_blocks_out_of_trash(
    scope: &mut Scope<'_>,
    deletion_id: &str,
    user_id: &str,
    now: i64,
    adjust: impl FnMut(&mut Block),
) -> Result<Vec<Block>, StoreError> {
    let mut reactions = KeptReactions::default();
    let restored =
        take_out_keeping_reactions(scope, deletion_id, user_id, now, adjust, &mut reactions)
            .await?;
    reactions.make_again(scope).await?;
    Ok(restored)
}

/// Takes the blocks of one deletion out of the trash as [`take_blocks_out_of_trash`]
/// does, but for the reactions they kept, which go to `reactions`: for a transaction
/// that takes several deletions out, and makes the reactions of all again at its end.
pub(crate) async fn take_out_keeping_reactions(
    scope: &mut Scope<'_>,
    deletion_id: &str,
    user_id: &str,
    now: i64,
    mut adjust: impl FnMut(&mut Block),
    reactions: &mut KeptReactions,
) -> Result<Vec<Block>, StoreError> {
    let mut sql = scope.sql("SELECT block FROM deleted_blocks WHERE deletion_id = ");
    sql.bind(Id(deletion_id));
    let rows = scope.fetch_all(sql).await?;
    let mut restored: Vec<Block> = Vec::with_capacity(rows.len());
    let mut kept = Vec::with_capacity(rows.len());
    for row in &rows {
        let (block, kept_with) = block_and_kept_from_json(row, 0)?;
        kept.push((block.id.clone(), kept_with));
        restored.push(block);
    }
    let mut known_users = KnownUsers::default();
    for block in &mut restored {
        adjust(block);
        block.deleted_at = None;
        block.updated_at = now;
        user_id.clone_into(&mut block.modified_by);
        known_users
            .forget_if_gone(scope, &mut block.created_by)
            .await?;
    }
    crate::card_numbers::number_restored(scope, &mut restored).await?;
    // A statement per block: each is checked on its own, so a parent goes first. What
    // they show, the trash kept for them.
    for index in parents_first(&restored) {
        blocks::insert_block_row(scope, &restored[index], NewBodyFiles::Unchecked).await?;
    }
    for (block_id, kept_with) in kept {
        watchers::watch_again(scope, &block_id, &kept_with.watchers).await?;
        reactions.keep(block_id, kept_with.reactions);
    }
    let mut sql = scope.sql("DELETE FROM deleted_blocks WHERE deletion_id = ");
    sql.bind(Id(deletion_id));
    scope.execute(sql).await?;
    Ok(restored)
}

pub(crate) async fn delete_container(
    scope: &mut Scope<'_>,
    container_id: &str,
    user_id: &str,
) -> Result<(), StoreError> {
    // Held from before anything of the board is read: nothing is added to the board, or
    // stored for it, between what goes to the trash and what is deleted.
    containers::lock_container_for_deletion(scope, container_id).await?;
    let container = containers::get_container(scope, container_id).await?;
    let members = containers::get_members_for_container(scope, container_id).await?;
    let files = files::files_of_container(scope, container_id).await?;
    let container_blocks =
        blocks::get_blocks(scope, &blocks::BlockFilter::container(container_id)).await?;
    // Where users filed the board, to file it there again when it comes back.
    let places: Vec<StoredPlace> = category_containers::places_of_container(scope, container_id)
        .await?
        .into_iter()
        .map(StoredPlace::from)
        .collect();

    let now = scope.now_millis();
    let deletion_id = new_id();
    put_blocks_in_trash(scope, &container_blocks, &deletion_id, now, user_id).await?;
    let in_trash = Container {
        deleted_at: Some(now),
        ..container
    };
    let mut sql = scope.sql(
        "INSERT INTO deleted_containers \
         (id, workspace_id, deleted_at, deleted_by, deletion_id, container, members, files, places) \
         VALUES (",
    );
    sql.bind(Id(container_id))
        .push_bind(", ", Id(&in_trash.workspace_id))
        .push_bind(", ", now)
        .push_bind(", ", OptId(user_id))
        .push_bind(", ", Id(&deletion_id))
        .push_bind(", ", Param::json(&in_trash)?)
        .push_bind(", ", Param::json(&members)?)
        .push_bind(", ", Param::json(&StoredFiles::from(files))?)
        .push_bind(", ", Param::json(&places)?)
        .push(")");
    scope.execute(sql).await?;
    crate::card_numbers::keep_with_trashed(scope, container_id).await?;

    crate::container_team_trash::keep(scope, container_id).await?;
    block_derived::container_trashed(scope, container_id).await?;

    // The blocks first: a board does not go while it has any. With the board go its
    // memberships, its sharing, its file records and where users filed it (kept above).
    let mut sql = scope.sql("DELETE FROM blocks WHERE container_id = ");
    sql.bind(Id(container_id));
    scope.execute(sql).await?;
    let mut sql = scope.sql("DELETE FROM containers WHERE id = ");
    sql.bind(Id(container_id));
    scope.execute(sql).await?;
    Ok(())
}

pub(crate) async fn undelete_container(
    scope: &mut Scope<'_>,
    container_id: &str,
    user_id: &str,
) -> Result<Container, StoreError> {
    let mut sql = scope.sql(
        "SELECT container, members, files, deletion_id, places FROM deleted_containers WHERE id = ",
    );
    sql.bind(Id(container_id));
    // Of two requests that restore the board at once, the second waits here and then
    // finds the trash without it.
    sql.push_lock(scope.row_lock(LockRank::DeletedContainers, container_id, RowLock::Update));
    let row = scope
        .fetch_optional(sql)
        .await?
        .ok_or_else(container_not_in_trash)?;
    let mut container: Container = from_json(&row, 0, "container")?;
    let members: Vec<ContainerMember> = from_json(&row, 1, "members")?;
    let files: StoredFiles = from_json(&row, 2, "files")?;
    let deletion_id = row.id(3)?;
    let places: Vec<StoredPlace> = from_json(&row, 4, "places")?;

    let now = scope.now_millis();
    container.deleted_at = None;
    container.updated_at = now;
    user_id.clone_into(&mut container.modified_by);
    let mut known_users = KnownUsers::default();
    known_users
        .forget_if_gone(scope, &mut container.created_by)
        .await?;
    containers::insert_container_row(scope, &container).await?;
    crate::card_numbers::bring_back(scope, container_id).await?;

    for member in &members {
        // A membership of a user whose row is gone since does not come back.
        if known_users.has(scope, &member.user_id).await? {
            containers::save_member(scope, member).await?;
        }
    }
    // A board does not come back without an admin who can act as one: where those it
    // had are gone from its workspace since, or are its guests now, who brings it back
    // is its admin.
    if !user_id.is_empty()
        && members::has_no_admin(scope, container_id).await?
        && members::works_in(scope, &container.workspace_id, user_id).await?
    {
        let admin = ContainerMember::new(container_id, user_id, fvoci_model::ContainerRole::Admin);
        containers::save_member(scope, &admin).await?;
    }
    crate::container_team_trash::restore(scope, &container, user_id).await?;
    // Each user who still exists finds the board where they had filed it; a private
    // board, only those who are its members again (a member removed earlier keeps the
    // place but cannot see the board).
    for place in places.into_iter().map(ContainerPlace::from) {
        let sees = container.visibility == ContainerVisibility::Open
            || members.iter().any(|member| member.user_id == place.user_id)
            || crate::container_team_access::role_for(scope, container_id, &place.user_id)
                .await?
                .is_some();
        if sees && known_users.has(scope, &place.user_id).await? {
            category_containers::put_back(scope, container_id, &container.workspace_id, &place)
                .await?;
        }
    }
    for file in Vec::<FileRecord>::from(files) {
        files::save_file_record(scope, &file).await?;
    }
    take_blocks_out_of_trash(scope, &deletion_id, user_id, now, |_| {}).await?;
    // The trash kept no marks: a file that no block of the board shows is counted from
    // now, as at a file's upload, and one that a block shows has none.
    let mut sql = scope.sql("UPDATE files SET unreferenced_at = NULL WHERE container_id = ");
    sql.bind(Id(container_id));
    scope.execute(sql).await?;
    let mut sql = scope.sql("UPDATE files SET unreferenced_at = ");
    sql.bind(now)
        .push(" WHERE NOT EXISTS (SELECT 1 FROM file_refs r WHERE r.container_id = files.container_id AND r.name = files.name) AND container_id = ")
        .bind(Id(container_id));
    scope.execute(sql).await?;

    let mut sql = scope.sql("DELETE FROM deleted_containers WHERE id = ");
    sql.bind(Id(container_id));
    scope.execute(sql).await?;
    Ok(container)
}
