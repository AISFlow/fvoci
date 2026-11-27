//! Atomic board operations: pre-copied card duplication, multi-card view moves, batch
//! card delete/restore, fractional-indexed board views, and verified card attachments.

use std::collections::{BTreeSet, HashSet};

use fvoci_model::{
    Block, BlockError, BlockPatch, BlockType, BoardViewDuplicate, BoardViewMove, BoardViewWrite,
    CardMovesRequest, ContainerKind, FileRecord, JsonMap, MAX_CARD_BATCH, NewBoardView,
    NewCardAttachment, PAGE_ORDER_FIELD, PropertyChanges, RefusalReason, Refusals,
    RelationProperties, UnmatchedRelation, drop_malformed_view_fields, is_id, new_id,
    place_view_at, renew_block_ids, sort_board_views,
};
use serde_json::Value;

use crate::blocks::{self, BLOCK_COLUMNS, OLDEST_FIRST, fetch_blocks, insert_blocks, patch_block};
use crate::body_versions::{self, BodyWrite};
use crate::comment_versions::EDITED_AT_COLUMN;
use crate::db::{Dialect, Id, Scope};
use crate::error::StoreError;
use crate::file_refs::NewBodyFiles;
use crate::files::{self, FILE_COLUMNS, file_from_row};
use crate::loose_objects;
use crate::notification_noting::Telling;
use crate::trash::{self, with_tree_of};
use crate::{Store, card_values, containers};
use fvoci_model::DuplicateAs;

/// Consistent snapshot read before duplicating a card so object-storage files can be
/// copied before opening the write transaction.
#[derive(Debug, Clone)]
pub struct PreparedCardDuplicate {
    /// Owning organization of the board (used for `FileKey` generation).
    pub org_id: String,
    /// Cloned blocks with renewed IDs, ready to have file IDs remapped and be committed.
    pub blocks: Vec<Block>,
    /// Recorded `files` rows on `board_id` referenced by `blocks`.
    pub source_files: Vec<FileRecord>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Subcards {
    Include,
    Omit,
}

impl Store {
    /// Reads the subtree and referenced `files` rows of `card_id` on `board_id` in one
    /// read snapshot so the server can pre-copy storage objects before committing.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NotFound`] if `board_id` or `card_id` does not name a live
    /// card on a live board, and [`StoreError::Database`] if a query fails.
    pub async fn prepare_card_duplicate(
        &self,
        board_id: &str,
        card_id: &str,
        duplicate_as: DuplicateAs,
    ) -> Result<PreparedCardDuplicate, StoreError> {
        self.read_snapshot(async |snapshot| {
            let scope = snapshot.scope();
            let not_found = || {
                StoreError::not_found(format_args!(
                    "card subtree BoardID={board_id} CardID={card_id}"
                ))
            };
            let board = containers::find_container(scope, board_id)
                .await?
                .filter(|c| c.kind == ContainerKind::Board)
                .ok_or_else(not_found)?;
            let org_id = board_org_id(scope, board_id).await?;
            let mut sql = with_tree_of(scope, card_id);
            sql.push([
                "SELECT ",
                BLOCK_COLUMNS,
                ", ",
                EDITED_AT_COLUMN,
                " FROM blocks WHERE id IN (SELECT id FROM tree) AND container_id = ",
            ])
            .bind(Id(board_id))
            .push(OLDEST_FIRST);
            let family = fetch_blocks(scope, sql).await?;
            let (roots, others): (Vec<Block>, Vec<Block>) = family
                .into_iter()
                .filter(|b| b.block_type != BlockType::Comment)
                .partition(|b| b.id == card_id);
            let mut root = roots
                .into_iter()
                .next()
                .filter(|b| b.block_type == BlockType::Card)
                .ok_or_else(not_found)?;
            let as_template = duplicate_as == DuplicateAs::Template;
            root.fields
                .insert("isTemplate".to_owned(), Value::Bool(as_template));
            let subcards = if as_template {
                root.parent_id.clear();
                Subcards::Omit
            } else {
                Subcards::Include
            };
            let children = reachable_card_descendants(card_id, others, subcards);
            let mut blocks = Vec::with_capacity(children.len() + 1);
            blocks.push(root);
            blocks.extend(children);
            card_values::clear_copied_values(&mut blocks, &board.card_properties);
            card_values::keep_cards_of_board(scope, &mut blocks, board_id, &board.card_properties)
                .await?;
            renew_block_ids(
                &mut blocks,
                &RelationProperties::of_board(board_id, &board.card_properties),
                UnmatchedRelation::Kept,
            );
            let source_files = referenced_board_files(scope, board_id, &blocks).await?;
            Ok(PreparedCardDuplicate {
                org_id,
                blocks,
                source_files,
            })
        })
        .await
    }

    /// Commits pre-copied `staged_files`, duplicated card `blocks` (with remapped file
    /// IDs), and initial `body_versions` for copied cards in a single database
    /// transaction.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::InvalidPath`] for an invalid storage path,
    /// [`StoreError::NotFound`] if the board or a staged file's writing mark is gone, or
    /// [`StoreError::InvalidBlock`] if a copied block fails validation.
    pub async fn commit_card_duplicate(
        &self,
        board_id: &str,
        mut blocks: Vec<Block>,
        staged_files: &[FileRecord],
        user_id: &str,
    ) -> Result<Vec<Block>, StoreError> {
        for file in staged_files {
            files::check_new_path(&file.path)?;
        }
        self.transaction(async |scope| {
            let Some(kind) = containers::lock_container_of_kind(scope, board_id).await? else {
                return Err(StoreError::not_found(format_args!("board ID={board_id}")));
            };
            if kind != ContainerKind::Board {
                return Err(StoreError::not_found(format_args!("board ID={board_id}")));
            }
            let definitions = containers::find_container(scope, board_id)
                .await?
                .map(|b| b.card_properties)
                .unwrap_or_default();
            card_values::keep_cards_of_board(scope, &mut blocks, board_id, &definitions).await?;
            for file in staged_files {
                if !loose_objects::record_written(scope, file).await? {
                    return Err(StoreError::NotFound(format!(
                        "mark of the duplicated card file {}",
                        file.path
                    )));
                }
            }
            insert_blocks(
                scope,
                &mut blocks,
                user_id,
                NewBodyFiles::Recorded,
                Telling::Nobody,
            )
            .await?;
            for block in &blocks {
                body_versions::keep_copied_body(scope, block, user_id).await?;
            }
            Ok(blocks)
        })
        .await
    }

    /// Atomically moves `1..=200` cards in `view_id` of `board_id`, optionally updating a
    /// grouping property on every moved card (with full property validation and assignee
    /// notifications) and updating `view.fields.cardOrder`.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Refused`] for duplicate/out-of-range `cardIds`, an invalid
    /// `groupPropertyId`, or a deleted/foreign `afterCardId`, [`StoreError::NotFound`] if
    /// the board or view does not exist, and [`StoreError::InvalidBlock`] if `groupValue`
    /// fails card property validation.
    pub async fn move_cards_in_view(
        &self,
        board_id: &str,
        view_id: &str,
        request: &CardMovesRequest,
        user_id: &str,
    ) -> Result<(Block, Vec<Block>), StoreError> {
        validate_card_id_batch(&request.card_ids)?;
        self.transaction(async |scope| {
            lock_live_board(scope, board_id).await?;
            let board = containers::get_container(scope, board_id).await?;
            let view = find_board_view(scope, board_id, view_id).await?;
            verify_after_card(
                scope,
                board_id,
                &request.card_ids,
                request.after_card_id.as_deref(),
            )
            .await?;
            verify_live_board_cards(scope, board_id, &request.card_ids).await?;

            let updated_cards = update_moved_cards_group(
                scope,
                &board.card_properties,
                &request.card_ids,
                request.group_property_id.as_deref(),
                request.group_value.as_ref(),
                user_id,
            )
            .await?;

            let next_order = compute_next_card_order(
                scope,
                board_id,
                &view.fields,
                &request.card_ids,
                request.after_card_id.as_deref(),
            )
            .await?;
            let mut view_patch = BlockPatch::default();
            view_patch.updated_fields.insert(
                "cardOrder".to_owned(),
                Value::Array(next_order.into_iter().map(Value::String).collect()),
            );
            let updated_view = patch_block(
                scope,
                view_id,
                &view_patch,
                user_id,
                BodyWrite::Whole,
                Telling::Nobody,
            )
            .await?;
            Ok((updated_view, updated_cards))
        })
        .await
    }

    /// Deletes `1..=200` cards on `board_id` in a single transaction, recording each card
    /// (with its descendants) as an individually restorable deletion family. Returns all
    /// blocks moved to the trash.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Refused`] for invalid/duplicate `card_ids` or a block that
    /// is not a card on `board_id`, and [`StoreError::NotFound`] if `board_id` does not
    /// exist.
    pub async fn batch_delete_cards(
        &self,
        board_id: &str,
        card_ids: &[String],
        user_id: &str,
    ) -> Result<Vec<Block>, StoreError> {
        validate_card_id_batch(card_ids)?;
        self.transaction(async |scope| {
            lock_live_board(scope, board_id).await?;
            verify_live_board_cards_or_templates(scope, board_id, card_ids).await?;
            let mut deleted = Vec::new();
            for card_id in card_ids {
                deleted.extend(trash::delete_block(scope, card_id, user_id).await?);
            }
            Ok(deleted)
        })
        .await
    }

    /// Restores `1..=200` deleted cards on `board_id` in a single transaction, returning
    /// all restored blocks (cards and their restored attachments/comments/sub-cards).
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Refused`] for invalid/duplicate `card_ids` or a trashed
    /// block belonging to another container, [`StoreError::NotFound`] if the board or any
    /// requested card is not in the trash, and [`StoreError::NotRestorable`] if a card
    /// cannot be restored.
    pub async fn batch_restore_cards(
        &self,
        board_id: &str,
        card_ids: &[String],
        user_id: &str,
    ) -> Result<Vec<Block>, StoreError> {
        validate_card_id_batch(card_ids)?;
        self.transaction(async |scope| {
            lock_live_board(scope, board_id).await?;
            verify_trashed_board_cards(scope, board_id, card_ids).await?;
            let mut restored = Vec::new();
            let mut restored_ids = HashSet::new();
            for card_id in card_ids {
                if restored_ids.contains(card_id.as_str()) {
                    continue;
                }
                let family = trash::undelete_block(scope, card_id, user_id).await?;
                for block in family {
                    if restored_ids.insert(block.id.clone()) {
                        restored.push(block);
                    }
                }
            }
            Ok(restored)
        })
        .await
    }

    /// Creates a board view on `board_id` with fractional `order` computed on the server,
    /// returning the new view and any companion sibling views renumbered in the same
    /// transaction.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NotFound`] if `board_id` does not exist,
    /// [`StoreError::Refused`] if `new.after` is not a live view on `board_id`, and
    /// [`StoreError::InvalidBlock`] if the view fields fail validation.
    pub async fn create_board_view(
        &self,
        board_id: &str,
        new: &NewBoardView,
        user_id: &str,
    ) -> Result<BoardViewWrite, StoreError> {
        self.transaction(async |scope| {
            lock_live_board(scope, board_id).await?;
            let mut views = load_sorted_board_views(scope, board_id).await?;
            let target_idx =
                resolve_view_insert_index(&views, new.after.as_deref(), MissingAfter::End)?;
            let (order, companions) = place_view_at(&views, target_idx);
            let companion_views = write_companion_view_orders(scope, companions).await?;
            let mut fields = new.fields.clone().unwrap_or_default();
            fields.insert(
                "viewType".to_owned(),
                Value::String(new.view_type.as_str().to_owned()),
            );
            fields.insert(PAGE_ORDER_FIELD.to_owned(), Value::String(order));
            let mut view = Block {
                id: new_id(),
                container_id: board_id.to_owned(),
                block_type: BlockType::View,
                title: new.title.clone(),
                fields,
                ..Block::default()
            };
            blocks::insert_block(scope, &mut view, user_id).await?;
            views.clear();
            Ok(BoardViewWrite {
                view,
                companion_views,
            })
        })
        .await
    }

    /// Moves `view_id` on `board_id` immediately after `mv.after` (`None` or `Some("")`
    /// places it first), renumbering siblings only if needed.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NotFound`] if `board_id` or `view_id` does not exist, and
    /// [`StoreError::Refused`] if `mv.after` is `view_id` itself or not a live view on
    /// `board_id`.
    pub async fn move_board_view(
        &self,
        board_id: &str,
        view_id: &str,
        mv: &BoardViewMove,
        user_id: &str,
    ) -> Result<BoardViewWrite, StoreError> {
        self.transaction(async |scope| {
            lock_live_board(scope, board_id).await?;
            let mut view = find_board_view(scope, board_id, view_id).await?;
            if mv.after.as_deref() == Some(view_id) {
                let mut refused = Refusals::new();
                refused.refuse(
                    "after",
                    RefusalReason::Invalid,
                    "cannot place a view after itself",
                );
                return Err(refused.into());
            }
            let mut others = load_sorted_board_views(scope, board_id).await?;
            others.retain(|v| v.id != view_id);
            let target_idx =
                resolve_view_insert_index(&others, mv.after.as_deref(), MissingAfter::Start)?;
            let (order, companions) = place_view_at(&others, target_idx);
            let companion_views = write_companion_view_orders(scope, companions).await?;
            let mut patch = BlockPatch::default();
            patch
                .updated_fields
                .insert(PAGE_ORDER_FIELD.to_owned(), Value::String(order));
            view = patch_block(
                scope,
                &view.id,
                &patch,
                user_id,
                BodyWrite::Whole,
                Telling::Nobody,
            )
            .await?;
            Ok(BoardViewWrite {
                view,
                companion_views,
            })
        })
        .await
    }

    /// Duplicates `view_id` on `board_id`, placing the duplicate right after `dup.after`
    /// (defaulting to immediately after `view_id` itself).
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NotFound`] if `board_id` or `view_id` does not exist,
    /// [`StoreError::Refused`] if `dup.after` is not a live view on `board_id`, and
    /// [`StoreError::InvalidBlock`] if the duplicated view fails validation.
    pub async fn duplicate_board_view(
        &self,
        board_id: &str,
        view_id: &str,
        dup: &BoardViewDuplicate,
        default_copy_title: impl FnOnce(&str) -> String,
        user_id: &str,
    ) -> Result<BoardViewWrite, StoreError> {
        self.transaction(async |scope| {
            lock_live_board(scope, board_id).await?;
            let source = find_board_view(scope, board_id, view_id).await?;
            let views = load_sorted_board_views(scope, board_id).await?;
            let effective_after = dup.after.as_deref().or(Some(view_id));
            let target_idx = resolve_view_insert_index(&views, effective_after, MissingAfter::End)?;
            let (order, companions) = place_view_at(&views, target_idx);
            let companion_views = write_companion_view_orders(scope, companions).await?;
            let mut fields = source.fields.clone();
            drop_malformed_view_fields(&mut fields);
            fields.insert(PAGE_ORDER_FIELD.to_owned(), Value::String(order));
            let title = dup
                .title
                .clone()
                .filter(|t| !t.trim().is_empty())
                .unwrap_or_else(|| default_copy_title(&source.title));
            let mut copy = Block {
                id: new_id(),
                container_id: board_id.to_owned(),
                block_type: BlockType::View,
                title,
                fields,
                ..Block::default()
            };
            blocks::insert_block(scope, &mut copy, user_id).await?;
            Ok(BoardViewWrite {
                view: copy,
                companion_views,
            })
        })
        .await
    }

    /// Creates a verified `BlockType::Attachment` on live card `card_id` of `board_id`
    /// by locking and verifying that `request.file_id` (matched by `files.id` or
    /// `files.name`) belongs to `board_id`, setting the byte size from `files`, and the
    /// title from the request or, failing a name there, from `files`.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NotFound`] if `board_id` or `card_id` is not a live card on
    /// `board_id`, and [`StoreError::Refused`] if the file does not belong to `board_id`.
    pub async fn create_card_attachment(
        &self,
        board_id: &str,
        card_id: &str,
        request: &NewCardAttachment,
        user_id: &str,
    ) -> Result<Block, StoreError> {
        let file_id = request.file_id.as_str();
        if file_id.trim().is_empty() {
            let mut refused = Refusals::new();
            refused.refuse("fileId", RefusalReason::Required, "fileId is required");
            return Err(refused.into());
        }
        self.transaction(async |scope| {
            lock_live_board(scope, board_id).await?;
            let card = blocks::find_block(scope, card_id)
                .await?
                .filter(|b| b.container_id == board_id && b.block_type == BlockType::Card)
                .ok_or_else(|| StoreError::not_found(format_args!("card ID={card_id}")))?;
            let file = lock_board_file(scope, board_id, file_id).await?;
            let mut fields = JsonMap::new();
            fields.insert("fileId".to_owned(), Value::String(file.stored_name));
            fields.insert("size".to_owned(), Value::Number(file.size.into()));
            let mut attachment = Block {
                id: new_id(),
                parent_id: card.id,
                container_id: board_id.to_owned(),
                block_type: BlockType::Attachment,
                title: request
                    .title
                    .clone()
                    .filter(|title| !title.trim().is_empty())
                    .unwrap_or(file.name),
                fields,
                ..Block::default()
            };
            blocks::insert_block(scope, &mut attachment, user_id).await?;
            Ok(attachment)
        })
        .await
    }
}

fn reachable_card_descendants(
    root_id: &str,
    candidates: Vec<Block>,
    subcards: Subcards,
) -> Vec<Block> {
    let include = subcards == Subcards::Include;
    let mut allowed: HashSet<String> = HashSet::from([root_id.to_owned()]);
    let mut remaining: Vec<Block> = candidates
        .into_iter()
        .filter(|b| include || b.block_type != BlockType::Card)
        .collect();
    let mut kept = Vec::with_capacity(remaining.len());
    loop {
        let mut next = Vec::new();
        let before = kept.len();
        for block in remaining {
            if allowed.contains(&block.parent_id) {
                if include && block.block_type == BlockType::Card {
                    allowed.insert(block.id.clone());
                }
                kept.push(block);
            } else {
                next.push(block);
            }
        }
        if kept.len() == before {
            return kept;
        }
        remaining = next;
    }
}

async fn board_org_id(scope: &mut Scope<'_>, board_id: &str) -> Result<String, StoreError> {
    if let Some(org_id) = scope.org_id() {
        return Ok(org_id);
    }
    let mut sql = scope.sql(
        "SELECT w.org_id FROM containers AS c JOIN workspaces AS w ON w.id = c.workspace_id \
         WHERE c.id = ",
    );
    sql.bind(Id(board_id));
    scope
        .fetch_optional(sql)
        .await?
        .ok_or_else(|| StoreError::not_found("board"))?
        .id(0)
}

async fn referenced_board_files(
    scope: &mut Scope<'_>,
    board_id: &str,
    blocks: &[Block],
) -> Result<Vec<FileRecord>, StoreError> {
    let names: BTreeSet<String> = blocks.iter().flat_map(Block::shown_files).collect();
    if names.is_empty() {
        return Ok(Vec::new());
    }
    let mut sql = scope.sql(["SELECT ", FILE_COLUMNS, " FROM files WHERE container_id = "]);
    sql.bind(Id(board_id))
        .push(" AND name IN (")
        .bind_list(names.iter().map(String::as_str))
        .push(") ORDER BY created_at, id");
    scope
        .fetch_all(sql)
        .await?
        .iter()
        .map(file_from_row)
        .collect()
}

fn validate_card_id_batch(card_ids: &[String]) -> Result<(), Refusals> {
    let mut refused = Refusals::new();
    if card_ids.is_empty() {
        refused.refuse(
            "cardIds",
            RefusalReason::Required,
            "at least one card ID is required",
        );
        return Err(refused);
    }
    if card_ids.len() > MAX_CARD_BATCH {
        refused.refuse(
            "cardIds",
            RefusalReason::TooLong,
            format!("at most {MAX_CARD_BATCH} card IDs are allowed"),
        );
        return Err(refused);
    }
    let mut seen = HashSet::with_capacity(card_ids.len());
    for (idx, id) in card_ids.iter().enumerate() {
        if !is_id(id) {
            refused.refuse_at(
                format!("/cardIds/{idx}"),
                RefusalReason::InvalidFormat,
                "must be a valid identifier",
            );
        } else if !seen.insert(id.as_str()) {
            refused.refuse_at(
                format!("/cardIds/{idx}"),
                RefusalReason::Invalid,
                "duplicate card identifier",
            );
        }
    }
    refused.into_result()
}

async fn lock_live_board(scope: &mut Scope<'_>, board_id: &str) -> Result<(), StoreError> {
    match containers::lock_container_of_kind(scope, board_id).await? {
        Some(ContainerKind::Board) => Ok(()),
        _ => Err(StoreError::not_found(format_args!("board ID={board_id}"))),
    }
}

async fn find_board_view(
    scope: &mut Scope<'_>,
    board_id: &str,
    view_id: &str,
) -> Result<Block, StoreError> {
    blocks::find_block(scope, view_id)
        .await?
        .filter(|b| b.container_id == board_id && b.block_type == BlockType::View)
        .ok_or_else(|| StoreError::not_found(format_args!("view ID={view_id}")))
}

async fn verify_after_card(
    scope: &mut Scope<'_>,
    board_id: &str,
    moved_ids: &[String],
    after_card_id: Option<&str>,
) -> Result<(), StoreError> {
    let Some(after_id) = after_card_id.filter(|s| !s.is_empty()) else {
        return Ok(());
    };
    if !is_id(after_id) || moved_ids.iter().any(|id| id == after_id) {
        let mut refused = Refusals::new();
        refused.refuse(
            "afterCardId",
            RefusalReason::Invalid,
            "afterCardId must name a live card on the board outside cardIds",
        );
        return Err(refused.into());
    }
    let valid = blocks::find_block(scope, after_id).await?.is_some_and(|b| {
        b.container_id == board_id && b.block_type == BlockType::Card && !b.is_template()
    });
    if !valid {
        let mut refused = Refusals::new();
        refused.refuse(
            "afterCardId",
            RefusalReason::Invalid,
            "afterCardId must name a live non-template card on this board",
        );
        return Err(refused.into());
    }
    Ok(())
}

async fn verify_live_board_cards(
    scope: &mut Scope<'_>,
    board_id: &str,
    card_ids: &[String],
) -> Result<(), StoreError> {
    let mut sql = scope.sql("SELECT id, type, fields FROM blocks WHERE container_id = ");
    sql.bind(Id(board_id))
        .push(" AND id IN (")
        .bind_list(card_ids.iter().map(|id| Id(id)))
        .push(")");
    let rows = scope.fetch_all(sql).await?;
    let mut valid_ids = HashSet::with_capacity(rows.len());
    for row in &rows {
        let id = row.id(0)?;
        let kind = blocks::block_type_column(row, 1, "type")?;
        let fields = row.json_map(2, "fields")?;
        if kind == BlockType::Card && fields.get("isTemplate") != Some(&Value::Bool(true)) {
            valid_ids.insert(id);
        }
    }
    if valid_ids.len() != card_ids.len() {
        let mut refused = Refusals::new();
        refused.refuse(
            "cardIds",
            RefusalReason::Invalid,
            "every cardId must name a live non-template card on this board",
        );
        return Err(refused.into());
    }
    Ok(())
}

async fn verify_live_board_cards_or_templates(
    scope: &mut Scope<'_>,
    board_id: &str,
    card_ids: &[String],
) -> Result<(), StoreError> {
    let mut sql = scope.sql("SELECT id, type FROM blocks WHERE container_id = ");
    sql.bind(Id(board_id))
        .push(" AND id IN (")
        .bind_list(card_ids.iter().map(|id| Id(id)))
        .push(")");
    let rows = scope.fetch_all(sql).await?;
    let mut found_cards = HashSet::with_capacity(rows.len());
    for row in &rows {
        let id = row.id(0)?;
        let kind = blocks::block_type_column(row, 1, "type")?;
        if kind != BlockType::Card {
            let mut refused = Refusals::new();
            refused.refuse(
                "cardIds",
                RefusalReason::Invalid,
                "every cardId must name a card on this board",
            );
            return Err(refused.into());
        }
        found_cards.insert(id);
    }
    if found_cards.len() != card_ids.len() {
        return Err(StoreError::NotAllFound {
            entity: "card",
            ids: card_ids.to_vec(),
        });
    }
    Ok(())
}

async fn verify_trashed_board_cards(
    scope: &mut Scope<'_>,
    board_id: &str,
    card_ids: &[String],
) -> Result<(), StoreError> {
    let mut sql =
        scope.sql("SELECT id, container_id, block_type FROM deleted_blocks WHERE id IN (");
    sql.bind_list(card_ids.iter().map(|id| Id(id))).push(")");
    let rows = scope.fetch_all(sql).await?;
    if rows.len() != card_ids.len() {
        return Err(StoreError::NotAllFound {
            entity: "deleted card",
            ids: card_ids.to_vec(),
        });
    }
    for row in &rows {
        let container_id = row.id(1)?;
        let block_type = row.text(2)?;
        if container_id != board_id || (!block_type.is_empty() && block_type != "card") {
            let mut refused = Refusals::new();
            refused.refuse(
                "cardIds",
                RefusalReason::Invalid,
                "every cardId must name a deleted card of this board",
            );
            return Err(refused.into());
        }
    }
    Ok(())
}

async fn update_moved_cards_group(
    scope: &mut Scope<'_>,
    definitions: &[fvoci_model::CardPropertyDefinition],
    card_ids: &[String],
    group_property_id: Option<&str>,
    group_value: Option<&Value>,
    user_id: &str,
) -> Result<Vec<Block>, StoreError> {
    let Some(prop_id) = group_property_id.filter(|s| !s.is_empty()) else {
        return Ok(Vec::new());
    };
    let Some(def) = definitions.iter().find(|d| d.id == prop_id) else {
        let mut refused = Refusals::new();
        refused.refuse(
            "groupPropertyId",
            RefusalReason::Invalid,
            "unknown card property on this board",
        );
        return Err(refused.into());
    };
    if matches!(
        def.property_type,
        fvoci_model::PropertyType::CreatedTime
            | fvoci_model::PropertyType::CreatedBy
            | fvoci_model::PropertyType::UpdatedTime
            | fvoci_model::PropertyType::UpdatedBy
    ) {
        let mut refused = Refusals::new();
        refused.refuse(
            "groupPropertyId",
            RefusalReason::Invalid,
            "automatic metadata properties cannot be modified",
        );
        return Err(refused.into());
    }
    let mut prop_changes = PropertyChanges::default();
    match group_value {
        None | Some(Value::Null) => {
            prop_changes.deleted.push(prop_id.to_owned());
        }
        Some(Value::String(s)) if s.is_empty() => {
            prop_changes.deleted.push(prop_id.to_owned());
        }
        Some(Value::Array(arr)) if arr.is_empty() => {
            prop_changes.deleted.push(prop_id.to_owned());
        }
        Some(val) => {
            let mut check_map = JsonMap::new();
            check_map.insert(prop_id.to_owned(), val.clone());
            fvoci_model::validate_card_property_values(&check_map).map_err(|err| {
                StoreError::InvalidBlock {
                    id: card_ids[0].clone(),
                    source: BlockError::from(err),
                }
            })?;
            prop_changes.updated.insert(prop_id.to_owned(), val.clone());
        }
    }
    let patch = BlockPatch {
        property_changes: prop_changes,
        ..BlockPatch::default()
    };
    let telling = Telling::of(scope);
    let mut updated = Vec::with_capacity(card_ids.len());
    for card_id in card_ids {
        updated
            .push(patch_block(scope, card_id, &patch, user_id, BodyWrite::Whole, telling).await?);
    }
    Ok(updated)
}

async fn compute_next_card_order(
    scope: &mut Scope<'_>,
    board_id: &str,
    view_fields: &JsonMap,
    moved_ids: &[String],
    after_card_id: Option<&str>,
) -> Result<Vec<String>, StoreError> {
    let mut sql = scope.sql("SELECT id, fields FROM blocks WHERE container_id = ");
    sql.bind(Id(board_id))
        .push_bind(" AND type = ", BlockType::Card.as_str())
        .push(OLDEST_FIRST);
    let mut live_ids = Vec::new();
    let mut live_set = HashSet::new();
    for row in scope.fetch_all(sql).await? {
        let id = row.id(0)?;
        let fields = row.json_map(1, "fields")?;
        if fields.get("isTemplate") != Some(&Value::Bool(true)) {
            live_set.insert(id.clone());
            live_ids.push(id);
        }
    }
    let moved_set: HashSet<&str> = moved_ids.iter().map(String::as_str).collect();
    let mut order = Vec::with_capacity(live_ids.len());
    let mut seen = HashSet::with_capacity(live_ids.len());
    if let Some(existing) = view_fields.get("cardOrder").and_then(Value::as_array) {
        for item in existing {
            if let Some(id) = item.as_str()
                && live_set.contains(id)
                && seen.insert(id.to_owned())
                && !moved_set.contains(id)
            {
                order.push(id.to_owned());
            }
        }
    }
    for id in live_ids {
        if seen.insert(id.clone()) && !moved_set.contains(id.as_str()) {
            order.push(id);
        }
    }
    let insert_idx = match after_card_id {
        Some("") => 0,
        Some(after_id) => order
            .iter()
            .position(|id| id == after_id)
            .map_or(order.len(), |idx| idx + 1),
        None => order.len(),
    };
    order.splice(insert_idx..insert_idx, moved_ids.iter().cloned());
    Ok(order)
}

async fn load_sorted_board_views(
    scope: &mut Scope<'_>,
    board_id: &str,
) -> Result<Vec<Block>, StoreError> {
    let mut views = blocks::get_blocks(
        scope,
        &blocks::BlockFilter::container(board_id).with_type(BlockType::View),
    )
    .await?;
    sort_board_views(&mut views);
    Ok(views)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MissingAfter {
    Start,
    End,
}

fn resolve_view_insert_index(
    views: &[Block],
    after: Option<&str>,
    missing: MissingAfter,
) -> Result<usize, Refusals> {
    match after {
        None => Ok(if missing == MissingAfter::End {
            views.len()
        } else {
            0
        }),
        Some("") => Ok(0),
        Some(after_id) => {
            if let Some(pos) = views.iter().position(|v| v.id == after_id) {
                Ok(pos + 1)
            } else {
                let mut refused = Refusals::new();
                refused.refuse(
                    "after",
                    RefusalReason::Invalid,
                    "after must name a live view on this board",
                );
                Err(refused)
            }
        }
    }
}

async fn write_companion_view_orders(
    scope: &mut Scope<'_>,
    companions: std::collections::BTreeMap<String, String>,
) -> Result<Vec<Block>, StoreError> {
    let mut updated = Vec::with_capacity(companions.len());
    for (sibling_id, order) in companions {
        let now = scope.now_millis();
        let (prefix, suffix) = match scope.dialect() {
            Dialect::Sqlite => (
                "UPDATE blocks SET fields = json_set(COALESCE(fields, '{}'), '$.order', ",
                "), updated_at = MAX(",
            ),
            Dialect::Postgres => (
                "UPDATE blocks SET fields = jsonb_set(COALESCE(fields::jsonb, '{}'::jsonb), '{order}', to_jsonb(",
                "::text), true)::json, updated_at = GREATEST(",
            ),
        };
        let mut sql = scope.sql(prefix);
        sql.bind(order)
            .push(suffix)
            .bind(now)
            .push(", updated_at + 1)")
            .push_bind(" WHERE id = ", Id(&sibling_id))
            .push([" RETURNING ", BLOCK_COLUMNS, ", ", EDITED_AT_COLUMN]);
        let view = fetch_blocks(scope, sql)
            .await?
            .pop()
            .ok_or_else(|| StoreError::not_found(format_args!("view ID={sibling_id}")))?;
        updated.push(view);
    }
    Ok(updated)
}

async fn lock_board_file(
    scope: &mut Scope<'_>,
    board_id: &str,
    file_id: &str,
) -> Result<FileRecord, StoreError> {
    let mut sql = scope.sql(["SELECT ", FILE_COLUMNS, " FROM files WHERE container_id = "]);
    sql.bind(Id(board_id));
    if is_id(file_id) {
        sql.push_bind(" AND (id = ", Id(file_id))
            .push_bind(" OR name = ", file_id)
            .push(")");
    } else {
        sql.push_bind(" AND name = ", file_id);
    }
    if scope.dialect() == Dialect::Postgres {
        sql.push(" FOR KEY SHARE");
    }
    let Some(row) = scope.fetch_optional(sql).await? else {
        let mut refused = Refusals::new();
        refused.refuse(
            "fileId",
            RefusalReason::Invalid,
            "fileId does not belong to this board",
        );
        return Err(refused.into());
    };
    file_from_row(&row)
}
