//! The writes of a card as a request asks them, for who reached its board
//! ([`Reached`]): the permission, the request's shape, a body sent as Markdown, the
//! files the body names, and then the card's own write (`App::create_card`,
//! `App::patch_card`), which nothing outside the logic layer calls.

use fvoci_model::{
    Block, BlockType, BodyChange, Card, CardError, CardPatch, RefusalReason, Refusals,
};
use fvoci_store::ByAgent;

use super::{App, ChangeNotification, Reached, body_of_markdown};
use crate::auth::now_millis;
use crate::error::AppError;
use crate::permissions::ContainerPermission;

/// What a member of the board who may not edit it is refused making a card with.
pub const CREATE_CARD_DENIED: &str = "access denied to create card";
/// What a member of the board who may not edit it is refused changing a card with.
pub const PATCH_CARD_DENIED: &str = "access denied to patch card";

/// A card as a request makes it: the card, and the body the request sent as `body`
/// ([`BodyChange::Keep`] if it sent none), which a body sent as `bodyMarkdown` may not
/// come with.
#[derive(Debug, Clone, Default)]
pub struct NewCardRequest {
    /// The card the request describes.
    pub card: Card,
    /// The body the request sent as `body`.
    pub document: BodyChange,
}

/// Where a request sent the body it writes: `/bodyMarkdown` if it sent Markdown.
fn body_member(markdown: Option<&String>) -> &'static str {
    if markdown.is_some() {
        "/bodyMarkdown"
    } else {
        "/body"
    }
}

/// Refuses a request that sends a body both as `body` and as `bodyMarkdown`.
fn refuse_both_bodies(body: &BodyChange) -> Result<(), AppError> {
    if body.is_keep() {
        return Ok(());
    }
    let mut refused = Refusals::new();
    refused.refuse(
        "bodyMarkdown",
        RefusalReason::Invalid,
        "a body is sent as body or as bodyMarkdown, not as both",
    );
    Err(refused.into())
}

impl App {
    /// Makes the card `request` describes on the board `on` reached, for its editors:
    /// everything of the request's shape is checked before the store is asked, a body
    /// sent as Markdown is made a body, and the files the body names must be of the
    /// board.
    ///
    /// # Errors
    ///
    /// Returns [`CREATE_CARD_DENIED`] as [`Reached::require`] does, a bad request for
    /// a request that is refused (a `boardId` of another board, a body sent both ways,
    /// a body that names a file the board does not have, at the member the body was
    /// sent as), and what `App::create_card` returns.
    pub async fn create_card_on(
        &self,
        on: &Reached,
        request: NewCardRequest,
        notification: ChangeNotification,
    ) -> Result<Card, AppError> {
        on.require(ContainerPermission::ManageCards, CREATE_CARD_DENIED)?;
        let NewCardRequest { mut card, document } = request;
        let board_id = on.container_id();
        // Everything of the request's shape that is refused, before the store is asked.
        let mut refused: Refusals = card.check_request().err().unwrap_or_default();
        if !card.board_id.is_empty() && card.board_id != board_id {
            refused.refuse(
                "boardId",
                RefusalReason::Invalid,
                CardError::BoardIdMismatch.to_string(),
            );
        }
        refused.into_result()?;
        let body_at = body_member(card.body_markdown.as_ref());
        card.body = match card.body_markdown.take() {
            Some(markdown) => {
                refuse_both_bodies(&document)?;
                body_of_markdown(None, &markdown)?.into_body()
            }
            None => document.into_body(),
        };
        (card.has_complex_blocks, card.complex_blocks) = (None, None);
        self.check_body_files(board_id, card.body.as_ref())
            .await
            .map_err(|error| error.moved("/body", body_at))?;
        board_id.clone_into(&mut card.board_id);
        card.populate(now_millis());
        card.validate().map_err(AppError::bad_request)?;
        self.create_card(card, board_id, on.user_id(), notification)
            .await
    }

    /// Changes `card`, a card of the board `on` reached that the request found by its
    /// identifier, as `patch` says, for its editors, as the agent of `agent` if one
    /// writes: a body sent as Markdown is made a body over the card's as it was, the
    /// patch's shape is checked before the store is asked, and the files the body names
    /// must be of the board.
    ///
    /// # Errors
    ///
    /// Returns [`PATCH_CARD_DENIED`] as [`Reached::require_through`] does, not found for
    /// a block that is no card of the board, a bad request for a patch that is refused
    /// (a body sent both ways, or naming a file the board does not have, at the member
    /// the body was sent as), and what `App::patch_card` returns.
    pub async fn patch_card_on(
        &self,
        on: &Reached,
        card: &Block,
        mut patch: CardPatch,
        notification: ChangeNotification,
        agent: Option<ByAgent<'_>>,
    ) -> Result<Card, AppError> {
        const WHAT: &str = "card";
        on.require_through(ContainerPermission::ManageCards, PATCH_CARD_DENIED, WHAT)?;
        if !on.holds(card) || card.block_type != BlockType::Card {
            return Err(AppError::not_found(WHAT));
        }
        let body_at = body_member(patch.body_markdown.as_ref());
        if let Some(markdown) = patch.body_markdown.take() {
            refuse_both_bodies(&patch.body)?;
            patch.body = body_of_markdown(card.body.as_ref(), &markdown)?;
        }
        // Everything of the request's shape that is refused, before the store is asked.
        patch.check_request()?;
        self.check_body_files(on.container_id(), patch.body.new_body())
            .await
            .map_err(|error| error.moved("/body", body_at))?;
        self.patch_card(&patch, &card.id, on.user_id(), notification, agent)
            .await
    }
}

/// The card's own writes, unchecked, for the fixtures of the integration tests: what
/// is written there is the test's, not a request's.
#[cfg(feature = "test-util")]
impl App {
    /// `App::create_card`, which only the logic layer calls otherwise.
    ///
    /// # Errors
    ///
    /// What `App::create_card` returns.
    #[doc(hidden)]
    pub async fn create_card_unchecked(
        &self,
        card: Card,
        board_id: &str,
        user_id: &str,
        notification: ChangeNotification,
    ) -> Result<Card, AppError> {
        self.create_card(card, board_id, user_id, notification)
            .await
    }

    /// `App::patch_card`, which only the logic layer calls otherwise.
    ///
    /// # Errors
    ///
    /// What `App::patch_card` returns.
    #[doc(hidden)]
    pub async fn patch_card_unchecked(
        &self,
        patch: &CardPatch,
        card_id: &str,
        user_id: &str,
        notification: ChangeNotification,
    ) -> Result<Card, AppError> {
        self.patch_card(patch, card_id, user_id, notification, None)
            .await
    }
}
