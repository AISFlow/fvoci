//! Atomic server board operations: pre-copied card duplication, multi-card view moves,
//! batch card delete/restore, fractional-indexed board views, verified card attachments,
//! and blank board scaffolding.

use std::collections::{HashMap, HashSet};

use fvoci_model::DuplicateAs;
use fvoci_model::{
    BlankBoardScaffolding, Block, BlockPatchBatch, BlockType, BoardLocale, BoardViewDuplicate,
    BoardViewMove, BoardViewWrite, Card, CardBatchPatch, CardMovesRequest, CardMovesResult,
    Container, ContainerKind, ContainersAndBlocks, JsonMap, NewBoardView, NewCardAttachment,
    PAGE_ORDER_FIELD, RestoredCards, ViewType, default_blank_board_scaffolding, is_id, new_id,
    place_view_at,
};
use serde_json::{Value, json};

use super::blocks::{ChangeAction, log_notification};
use super::board_parts::{
    ATTACH_DENIED, CREATE_VIEW_DENIED, DUPLICATE_VIEW_DENIED, MOVE_VIEW_DENIED,
};
use super::changes::Changes;
use super::container_copies::titles_in;
use super::containers::refuse_named_id;
use super::language::Language;
use super::{App, ChangeNotification, Reached};
use crate::collab::Deletion;
use crate::error::{AppError, OrMissing};
use crate::permissions::ContainerPermission;

impl App {
    /// Atomically duplicates `card_id` on `board_id`, pre-copying referenced storage
    /// files under `loose_objects` marks first and then committing `files` records,
    /// cloned blocks (with remapped file IDs), and initial `body_versions` in a single
    /// database transaction.
    ///
    /// If copying any file in storage fails, all newly written objects are given up and
    /// an error is returned without committing any block rows.
    ///
    /// # Errors
    ///
    /// Returns 404 if the board or card does not exist, 413 (`storage_full`) if copying
    /// files would exceed `max_org_storage_bytes`, and 500 if storage copy fails.
    pub async fn duplicate_card(
        &self,
        board_id: &str,
        card_id: &str,
        user_id: &str,
        duplicate_as: DuplicateAs,
    ) -> Result<Card, AppError> {
        let container = self.container(board_id).await?;
        let mut sources = vec![card_id.to_owned()];
        sources.extend(
            self.blocks_below(card_id)
                .await?
                .into_iter()
                .filter(|child| {
                    child.container_id == board_id && child.block_type == BlockType::Card
                })
                .map(|child| child.id),
        );
        self.collab().write_bodies(self, &sources).await?;

        let prepared = self
            .store()
            .prepare_card_duplicate(board_id, card_id, duplicate_as)
            .await?;
        let file_ids: Vec<String> = prepared
            .source_files
            .iter()
            .map(|file| file.stored_name.clone())
            .collect();
        let (file_map, staged_files, written_places) = self
            .pre_copy_files_to_container(
                self,
                board_id,
                &prepared.org_id,
                board_id,
                &file_ids,
                user_id,
            )
            .await?;

        let mut blocks = prepared.blocks;
        if let Err(error) = remap_copied_block_files(&mut blocks, &file_map) {
            for place in written_places {
                place.give_up().await;
            }
            return Err(error);
        }

        let committed = {
            let _structure = self.collab().changing_structure(self, board_id).await;
            self.store()
                .commit_card_duplicate(board_id, blocks, &staged_files, user_id)
                .await
        };
        let stored = match committed {
            Ok(stored) => {
                for place in written_places {
                    place.settled_in_transaction();
                }
                stored
            }
            Err(error) => {
                for place in written_places {
                    place.give_up().await;
                }
                return Err(error.into());
            }
        };
        let root = stored
            .first()
            .ok_or_else(|| AppError::not_found("card"))?
            .clone();
        // The copy is stored: its members and the webhooks are told before the answer is
        // read back, which may still fail.
        self.tell(Changes::in_workspace(&container.workspace_id).changed(stored));
        self.card_of_block(&root).await
    }

    /// Atomically moves `1..=200` cards in `view_id` of `board_id`, acquiring the
    /// `Collab` structure lock before the database `lock_container` lock and broadcasting
    /// the updated view before the updated cards.
    ///
    /// # Errors
    ///
    /// Returns 400 for duplicate/out-of-range `cardIds`, invalid `groupPropertyId` /
    /// `groupValue`, or invalid `afterCardId`, and 404 if the board or view is absent.
    pub async fn move_cards_in_view(
        &self,
        board_id: &str,
        view_id: &str,
        request: &CardMovesRequest,
        user_id: &str,
        notification: ChangeNotification,
    ) -> Result<CardMovesResult, AppError> {
        let container = self.container(board_id).await?;
        // Moved to the column of done, a card that repeats makes its next one.
        let completes = container.done_when.as_ref().is_some_and(|rule| {
            request.group_property_id.as_deref() == Some(rule.property_id.as_str())
        });
        let before = if completes {
            Some(self.store().get_blocks_by_ids(&request.card_ids).await?)
        } else {
            None
        };
        let (view, updated_blocks) = {
            let _structure = self.collab().changing_structure(self, board_id).await;
            self.store_telling(notification)
                .move_cards_in_view(board_id, view_id, request, user_id)
                .await?
        };
        let mut cards = Vec::with_capacity(updated_blocks.len());
        for block in &updated_blocks {
            cards.push(self.card_of_block(block).await?);
            log_notification(notification, ChangeAction::Update, &block.id);
        }
        log_notification(notification, ChangeAction::Update, &view.id);
        // The view before the cards it moved, and the next cards after the cards they follow.
        let made = before
            .map_or_default(|before| super::blocks::cards_made_after(&before, &updated_blocks));
        let told = std::iter::once(view.clone()).chain(updated_blocks);
        self.tell(
            Changes::in_workspace(&container.workspace_id)
                .changed(told)
                .reread(made),
        );
        Ok(CardMovesResult { view, cards })
    }

    /// Atomically deletes `1..=200` cards on `board_id` as individually restorable
    /// deletion families, acquiring the `Collab` container gate before the database lock.
    ///
    /// # Errors
    ///
    /// Returns 400 for invalid/duplicate `card_ids` or non-card blocks, and 404 if the
    /// board or any requested card does not exist.
    pub async fn batch_delete_cards(
        &self,
        board_id: &str,
        card_ids: &[String],
        user_id: &str,
        notification: ChangeNotification,
    ) -> Result<(), AppError> {
        let container = self.container(board_id).await?;
        let owned_board = board_id.to_owned();
        let owned_ids = card_ids.to_vec();
        let owned_user = user_id.to_owned();
        let deleted = self
            .collab()
            .remove(
                self,
                Deletion::Blocks {
                    container_id: board_id.to_owned(),
                    block_ids: card_ids.to_vec(),
                },
                async move |app| {
                    Ok(app
                        .store_telling(notification)
                        .batch_delete_cards(&owned_board, &owned_ids, &owned_user)
                        .await?)
                },
            )
            .await?;
        for card_id in card_ids {
            log_notification(notification, ChangeAction::Delete, card_id);
        }
        let deleted_cards = deleted
            .into_iter()
            .filter(|block| block.block_type == BlockType::Card);
        self.tell(Changes::in_workspace(&container.workspace_id).deleted(deleted_cards));
        Ok(())
    }

    /// Changes the property values of the cards of `batch` on `board_id` alike, all or
    /// none in one transaction, each as `App::patch_card` changes one: the values and
    /// what the board allows of them are checked alike, and whom a card is newly given
    /// to is told as `notification` says. Returns the cards as they are now, in the
    /// order of `batch`; listeners and webhooks are told in the background.
    ///
    /// # Errors
    ///
    /// Returns a not-found error for a card that is not on the board (also one of
    /// another board, or a block of another type), a bad request for a template of the
    /// board's cards, and the errors of `App::patch_blocks`.
    pub async fn patch_cards(
        &self,
        board_id: &str,
        batch: &CardBatchPatch,
        user_id: &str,
        notification: ChangeNotification,
    ) -> Result<Vec<Card>, AppError> {
        let container = self.container(board_id).await?;
        let cards = self.cards_of_board(board_id, &batch.card_ids).await?;
        if let Some(index) = cards.iter().position(Block::is_template) {
            return Err(AppError::bad_request(
                "a template of the board's cards is not patched in a batch",
            )
            .with_pointer(format!("/cardIds/{index}")));
        }
        let patch = batch.card_patch().to_block_patch()?;
        let alike = BlockPatchBatch {
            block_ids: batch.card_ids.clone(),
            block_patches: vec![patch; batch.card_ids.len()],
        };
        self.patch_blocks(&container.workspace_id, &alike, user_id, notification)
            .await?;
        let blocks = self.cards_of_board(board_id, &batch.card_ids).await?;
        let mut cards = Vec::with_capacity(blocks.len());
        for block in &blocks {
            cards.push(self.card_of_block(block).await?);
        }
        Ok(cards)
    }

    /// The cards of the board with these identifiers, in their order: one that is not
    /// there, of another board or a block of another type is not found, as the path
    /// names the board; so is text that is no identifier.
    async fn cards_of_board(
        &self,
        board_id: &str,
        card_ids: &[String],
    ) -> Result<Vec<Block>, AppError> {
        let not_found = || AppError::not_found("card");
        if !card_ids.iter().all(|card_id| is_id(card_id)) {
            return Err(not_found());
        }
        let blocks = self
            .store()
            .get_blocks_by_ids(card_ids)
            .await
            .or_missing("card")?;
        let mut by_id: HashMap<String, Block> = blocks
            .into_iter()
            .filter(|block| block.container_id == board_id && block.block_type == BlockType::Card)
            .map(|block| (block.id.clone(), block))
            .collect();
        card_ids
            .iter()
            .map(|card_id| by_id.remove(card_id).ok_or_else(not_found))
            .collect()
    }

    /// Atomically restores `1..=200` deleted cards on `board_id`, acquiring the `Collab`
    /// structure lock before the database lock and returning the restored cards.
    ///
    /// # Errors
    ///
    /// Returns 400 for invalid/duplicate `card_ids`, 404 if the board or any requested
    /// card is not in the board's trash, and 409 if a card cannot be restored.
    pub async fn batch_restore_cards(
        &self,
        board_id: &str,
        card_ids: &[String],
        user_id: &str,
    ) -> Result<RestoredCards, AppError> {
        let container = self.container(board_id).await?;
        let restored = {
            let _structure = self.collab().changing_structure(self, board_id).await;
            self.store()
                .batch_restore_cards(board_id, card_ids, user_id)
                .await?
        };
        let mut cards = Vec::new();
        for block in &restored {
            if block.block_type == BlockType::Card {
                cards.push(self.card_of_block(block).await?);
                log_notification(ChangeNotification::Send, ChangeAction::Add, &block.id);
            }
        }

        let restored_cards = restored
            .into_iter()
            .filter(|block| block.block_type == BlockType::Card);
        self.tell(Changes::in_workspace(&container.workspace_id).changed(restored_cards));
        Ok(RestoredCards { cards })
    }

    /// Creates a fractional-indexed view on the board `on` reached, for who may change
    /// its properties.
    ///
    /// # Errors
    ///
    /// Returns [`CREATE_VIEW_DENIED`] as [`Reached::require`] does, and 400 if `new` is
    /// invalid.
    pub async fn create_board_view(
        &self,
        on: &Reached,
        new: &NewBoardView,
    ) -> Result<BoardViewWrite, AppError> {
        let container = on.require(ContainerPermission::ManageProperties, CREATE_VIEW_DENIED)?;
        let (board_id, user_id) = (on.container_id(), on.user_id());
        let write = {
            let _structure = self.collab().changing_structure(self, board_id).await;
            self.store()
                .create_board_view(board_id, new, user_id)
                .await?
        };
        self.tell(view_written(&container.workspace_id, &write));
        Ok(write)
    }

    /// Moves `view_id` on the board `on` reached using server-side fractional indexing,
    /// for who may change its properties.
    ///
    /// # Errors
    ///
    /// Returns [`MOVE_VIEW_DENIED`] as [`Reached::require`] does, 404 if `view_id` does
    /// not exist and 400 if `mv.after` is invalid.
    pub async fn move_board_view(
        &self,
        on: &Reached,
        view_id: &str,
        mv: &BoardViewMove,
    ) -> Result<BoardViewWrite, AppError> {
        let container = on.require(ContainerPermission::ManageProperties, MOVE_VIEW_DENIED)?;
        let (board_id, user_id) = (on.container_id(), on.user_id());
        let write = {
            let _structure = self.collab().changing_structure(self, board_id).await;
            self.store()
                .move_board_view(board_id, view_id, mv, user_id)
                .await?
        };
        self.tell(view_written(&container.workspace_id, &write));
        Ok(write)
    }

    /// Duplicates `view_id` on the board `on` reached using server-side fractional
    /// indexing, for who may change its properties.
    ///
    /// # Errors
    ///
    /// Returns [`DUPLICATE_VIEW_DENIED`] as [`Reached::require`] does, 404 if `view_id`
    /// does not exist and 400 if `dup` is invalid.
    pub async fn duplicate_board_view(
        &self,
        on: &Reached,
        view_id: &str,
        dup: &BoardViewDuplicate,
        accept_language: Option<&str>,
    ) -> Result<BoardViewWrite, AppError> {
        let denial = DUPLICATE_VIEW_DENIED;
        let container = on.require(ContainerPermission::ManageProperties, denial)?;
        let (board_id, user_id) = (on.container_id(), on.user_id());
        let language = self.language_for(user_id, accept_language).await?;
        let titles = titles_in(language, ContainerKind::Board);
        let write = {
            let _structure = self.collab().changing_structure(self, board_id).await;
            self.store()
                .duplicate_board_view(board_id, view_id, dup, titles.copy_of, user_id)
                .await?
        };
        self.tell(view_written(&container.workspace_id, &write));
        Ok(write)
    }

    /// Creates a verified attachment block on `card_id` of the board `on` reached, for
    /// its editors, after locking and verifying that `request.file_id` belongs to the
    /// board.
    ///
    /// # Errors
    ///
    /// Returns [`ATTACH_DENIED`] as [`Reached::require`] does, 404 if `card_id` is absent
    /// and 400 if `request.file_id` does not belong to the board.
    pub async fn create_card_attachment(
        &self,
        on: &Reached,
        card_id: &str,
        request: &NewCardAttachment,
        notification: ChangeNotification,
    ) -> Result<Block, AppError> {
        let container = on.require(ContainerPermission::ManageCards, ATTACH_DENIED)?;
        let (board_id, user_id) = (on.container_id(), on.user_id());
        let attachment = {
            let _structure = self.collab().changing_structure(self, board_id).await;
            self.store_telling(notification)
                .create_card_attachment(board_id, card_id, request, user_id)
                .await?
        };
        log_notification(notification, ChangeAction::Add, &attachment.id);
        self.tell(Changes::in_workspace(&container.workspace_id).changed([attachment.clone()]));
        Ok(attachment)
    }
}

/// What a new board starts with besides what its request names.
#[derive(Debug, Clone, PartialEq)]
pub enum BoardStart {
    /// Nothing: no view, and the properties the request names.
    Empty,
    /// For a blank board that is no template, localized `status`/`assignee`/`due`
    /// properties and a first kanban view (`scaffold`); any other board is made as it is.
    Scaffolded,
    /// This first view (`firstView`).
    WithView(NewBoardView),
}

impl App {
    /// Creates a new board container with what `start` says: its first view, made with
    /// it in one transaction, or the default `status`/`assignee`/`due` properties,
    /// `done_when`, `due_date_property_id` and a first kanban view, or neither. The user
    /// becomes the board's admin.
    ///
    /// # Errors
    ///
    /// Returns a bad request naming `/id` for a board that names its identifier, as
    /// every way of making a board does, one naming `/firstView` for a first view that is
    /// refused, and
    /// the errors of [`App::create_container_with_admin`] or
    /// [`App::create_containers_and_blocks_with_admin`].
    pub async fn create_board(
        &self,
        mut container: Container,
        start: BoardStart,
        user_id: &str,
        accept_language: Option<&str>,
    ) -> Result<Container, AppError> {
        refuse_named_id(&container)?;
        let view = match start {
            BoardStart::Empty => None,
            BoardStart::Scaffolded
                if container.is_template || !container.card_properties.is_empty() =>
            {
                None
            }
            BoardStart::Scaffolded => {
                let language = self.language_for(user_id, accept_language).await?;
                let locale = if language == Language::Korean {
                    BoardLocale::Korean
                } else {
                    BoardLocale::English
                };
                let scaffold = default_blank_board_scaffolding(locale);
                if container.due_date_property_id.is_none() {
                    container.due_date_property_id = Some(scaffold.due.id.clone());
                }
                if container.done_when.is_none() {
                    container.done_when = Some(scaffold.done_when.clone());
                }
                container.card_properties = scaffold.properties();
                container.id = new_id();
                Some(first_view_of_scaffold(&container.id, &scaffold, locale))
            }
            BoardStart::WithView(new) => {
                container.id = new_id();
                let view = first_view(&container.id, new);
                view.validate()
                    .map_err(|refused| AppError::from(refused).under(FIRST_VIEW))?;
                Some(view)
            }
        };
        let Some(view) = view else {
            return self.create_container_with_admin(container, user_id).await;
        };
        let view_id = view.id.clone();
        // The board and its first view are one transaction: no board is left without it.
        let made = self
            .create_containers_and_blocks_with_admin(
                ContainersAndBlocks {
                    containers: vec![container],
                    blocks: vec![view],
                },
                user_id,
            )
            .await
            .map_err(|error| error.of_block(&view_id, FIRST_VIEW))?;
        made.containers
            .into_iter()
            .next()
            .ok_or_else(|| AppError::internal("the board was not made"))
    }
}

/// What a write of a view changed: the views numbered anew for it, then the view.
fn view_written(workspace_id: &str, write: &BoardViewWrite) -> Changes {
    let told = write.companion_views.iter().chain([&write.view]).cloned();
    Changes::in_workspace(workspace_id).changed(told)
}

/// The JSON Pointer to the first view of a board in the request that makes the board.
const FIRST_VIEW: &str = "/firstView";

/// The first view of the board `board_id` as `new` asks for it, first among its views:
/// as `createBoardView` makes a view.
fn first_view(board_id: &str, new: NewBoardView) -> Block {
    let (order, _) = place_view_at(&[], 0);
    let mut fields = new.fields.unwrap_or_default();
    fields.insert(
        "viewType".to_owned(),
        Value::String(new.view_type.as_str().to_owned()),
    );
    fields.insert(PAGE_ORDER_FIELD.to_owned(), Value::String(order));
    Block {
        id: new_id(),
        container_id: board_id.to_owned(),
        block_type: BlockType::View,
        title: new.title,
        fields,
        ..Block::default()
    }
}

/// The first view of a scaffolded board `board_id`: a kanban grouped by its status, of
/// the cards that have one, showing who each is given to and when it is due. Its title
/// is the one the web app gives the first view of a board it makes.
fn first_view_of_scaffold(
    board_id: &str,
    scaffold: &BlankBoardScaffolding,
    locale: BoardLocale,
) -> Block {
    let visible_groups: Vec<&str> = scaffold
        .status
        .options
        .iter()
        .map(|option| option.id.as_str())
        .collect();
    let (order, _) = place_view_at(&[], 0);
    let mut fields = JsonMap::new();
    fields.insert(
        "viewType".to_owned(),
        Value::String(ViewType::Board.as_str().to_owned()),
    );
    fields.insert(
        "groupBy".to_owned(),
        Value::String(scaffold.status.id.clone()),
    );
    fields.insert("visibleGroups".to_owned(), json!(visible_groups));
    fields.insert("hiddenGroups".to_owned(), json!([""]));
    fields.insert(
        "shownProperties".to_owned(),
        json!([scaffold.assignee.id, scaffold.due.id]),
    );
    fields.insert(PAGE_ORDER_FIELD.to_owned(), Value::String(order));
    Block {
        id: new_id(),
        container_id: board_id.to_owned(),
        block_type: BlockType::View,
        title: match locale {
            BoardLocale::Korean => "보드 뷰",
            BoardLocale::English => "Board view",
        }
        .to_owned(),
        fields,
        ..Block::default()
    }
}

/// Gives copied `blocks` the files their copies were written to (`file_map`, from the
/// stored name of the original to that of the copy). An attachment whose file was not
/// copied goes, and out of the `contentOrder` of the block that lists it: a copy never
/// names a file of the card it was made from, nor an empty one.
///
/// # Errors
///
/// Returns a bad request if a body grows past its size limit.
pub(super) fn remap_copied_block_files(
    blocks: &mut Vec<Block>,
    file_map: &HashMap<String, String>,
) -> Result<(), AppError> {
    let mut dropped = HashSet::new();
    blocks.retain(|block| {
        let unmapped = block
            .attachment_file()
            .is_some_and(|file| !file_map.contains_key(file));
        if unmapped {
            dropped.insert(block.id.clone());
        }
        !unmapped
    });
    for block in blocks.iter_mut() {
        if !dropped.is_empty()
            && let Some(Value::Array(order)) = block.fields.get_mut("contentOrder")
        {
            order.retain(|item| item.as_str().is_none_or(|id| !dropped.contains(id)));
        }
        if block
            .body
            .as_ref()
            .is_some_and(|body| !body.file_ids().is_empty())
        {
            block.body = block
                .body
                .take()
                .and_then(|body| body.with_files_renamed(|old| file_map.get(old).cloned()));
            if let Some(body) = &block.body {
                body.check_size()
                    .map_err(|error| AppError::from_body_error("body", error))?;
            }
        }
        if let Some(new_name) = block
            .attachment_file()
            .and_then(|old| file_map.get(old))
            .cloned()
        {
            block
                .fields
                .insert("fileId".to_owned(), Value::String(new_name));
            block.fields.remove("attachmentId");
        }
    }
    Ok(())
}
